//! A supervised child process in its own process group, so that Stop also ends
//! the children of a dev server and its port is released.
use super::detect::RunConfig;
use super::output::Stream;
use crate::platform::{self, tools};
use futures::channel::mpsc;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    Output(Stream, String),
    /// Exit code, `None` when killed by a signal.
    Exited(Option<i32>),
    SpawnFailed(String),
}

pub struct RunningProcess {
    pgid: u32,
    stopping: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    // Kept open: some dev servers exit when stdin reaches EOF.
    _stdin: Option<std::process::ChildStdin>,
}

impl RunningProcess {
    pub fn spawn(config: &RunConfig, events: mpsc::UnboundedSender<RunEvent>) -> Option<Self> {
        tools::wait_for_environment();
        let program = if config.program.contains('/') {
            config.program.clone().into()
        } else {
            tools::find(&config.program).unwrap_or_else(|| config.program.clone().into())
        };
        let mut cmd = Command::new(program);
        cmd.args(&config.args)
            .current_dir(&config.cwd)
            .env_clear()
            .envs(tools::child_env())
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::new_process_group(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let hint = tools::install_hint(&config.program);
                let hint = if hint.is_empty() {
                    String::new()
                } else {
                    format!("\nInstall: {hint}")
                };
                let _ = events.unbounded_send(RunEvent::SpawnFailed(format!(
                    "Cannot start {}: {e}{hint}",
                    config.program
                )));
                return None;
            }
        };
        let pgid = child.id();
        let stdin = child.stdin.take();
        let mut readers = vec![];
        if let Some(out) = child.stdout.take() {
            readers.push(pump(out, Stream::Stdout, events.clone()));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(pump(err, Stream::Stderr, events.clone()));
        }
        let finished = Arc::new(AtomicBool::new(false));
        let fin = finished.clone();
        std::thread::Builder::new()
            .name("run-wait".into())
            .spawn(move || {
                let status = child.wait();
                // A surviving grandchild may hold the pipes open forever; stop waiting
                // for them shortly after the main process is gone.
                let deadline = std::time::Instant::now() + Duration::from_millis(400);
                for r in readers {
                    while !r.is_finished() && std::time::Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                fin.store(true, Ordering::SeqCst);
                let code = status.ok().and_then(|s| s.code());
                let _ = events.unbounded_send(RunEvent::Exited(code));
            })
            .ok();
        Some(Self {
            pgid,
            stopping: Arc::new(AtomicBool::new(false)),
            finished,
            _stdin: stdin,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Stop the whole process group: SIGTERM, then SIGKILL after a grace period.
    /// Never blocks the caller.
    pub fn stop(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        let pgid = self.pgid;
        std::thread::spawn(move || platform::terminate_group(pgid, Duration::from_secs(2)));
    }

    /// Stop, waiting until the group is gone. For application exit.
    pub fn stop_blocking(&self) {
        platform::terminate_group(self.pgid, Duration::from_millis(800));
    }

    pub fn pgid(&self) -> u32 {
        self.pgid
    }
}

fn pump(
    mut stream: impl Read + Send + 'static,
    kind: Stream,
    events: mpsc::UnboundedSender<RunEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        let mut pending: Vec<u8> = vec![];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // Keep an incomplete UTF-8 sequence for the next read.
                    let valid = match std::str::from_utf8(&pending) {
                        Ok(_) => pending.len(),
                        Err(e) if e.error_len().is_none() => e.valid_up_to(),
                        Err(_) => pending.len(),
                    };
                    let text = String::from_utf8_lossy(&pending[..valid]).to_string();
                    pending.drain(..valid);
                    if !text.is_empty()
                        && events.unbounded_send(RunEvent::Output(kind, text)).is_err()
                    {
                        break;
                    }
                }
            }
        }
    })
}

// These tests drive real Unix processes; Windows is checked on Windows itself.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::path::PathBuf;

    fn cfg(script: &str, cwd: PathBuf) -> RunConfig {
        RunConfig {
            id: "t".into(),
            label: "t".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd,
        }
    }

    #[test]
    fn captures_streams_and_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded();
        let _p = RunningProcess::spawn(
            &cfg("echo out; echo err 1>&2; pwd; exit 3", d.path().into()),
            tx,
        )
        .unwrap();
        let mut out = String::new();
        let mut err = String::new();
        let mut code = None;
        futures::executor::block_on(async {
            while let Some(ev) = rx.next().await {
                match ev {
                    RunEvent::Output(Stream::Stdout, s) => out.push_str(&s),
                    RunEvent::Output(Stream::Stderr, s) => err.push_str(&s),
                    RunEvent::Exited(c) => {
                        code = c;
                        break;
                    }
                    _ => {}
                }
            }
        });
        assert!(out.contains("out"));
        assert!(err.contains("err"));
        assert_eq!(code, Some(3));
    }

    #[test]
    fn stop_kills_children_too() {
        let d = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded();
        // Parent ignores TERM so that the KILL path is exercised for it; child sleeps.
        let p =
            RunningProcess::spawn(&cfg("sleep 300 & echo $!; wait", d.path().into()), tx).unwrap();
        let pid: i32 = futures::executor::block_on(async {
            loop {
                if let Some(RunEvent::Output(Stream::Stdout, s)) = rx.next().await
                    && let Ok(n) = s.trim().parse()
                {
                    return n;
                }
            }
        });
        assert!(unsafe { libc::kill(pid, 0) } == 0);
        p.stop();
        let start = std::time::Instant::now();
        while unsafe { libc::kill(pid, 0) } == 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(unsafe { libc::kill(pid, 0) } != 0, "child survived");
    }

    #[test]
    fn missing_program_reports_failure() {
        let (tx, mut rx) = mpsc::unbounded();
        let mut c = cfg("", std::env::temp_dir());
        c.program = "/nonexistent/prog".into();
        assert!(RunningProcess::spawn(&c, tx).is_none());
        let ev = futures::executor::block_on(rx.next()).unwrap();
        assert!(matches!(ev, RunEvent::SpawnFailed(_)));
    }

    #[test]
    fn utf8_split_across_reads_is_not_mangled() {
        let d = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded();
        // printf the two bytes of "я" separately with a delay in between.
        let _p = RunningProcess::spawn(
            &cfg(
                r"printf '\321'; sleep 0.2; printf '\217\n'",
                d.path().into(),
            ),
            tx,
        )
        .unwrap();
        let mut out = String::new();
        futures::executor::block_on(async {
            while let Some(ev) = rx.next().await {
                match ev {
                    RunEvent::Output(_, s) => out.push_str(&s),
                    RunEvent::Exited(_) => break,
                    _ => {}
                }
            }
        });
        assert_eq!(out, "я\n");
    }
}
