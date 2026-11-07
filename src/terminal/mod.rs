//! Real interactive Unix PTY sessions: ANSI parsing by `alacritty_terminal`.
pub mod input;
#[cfg(unix)]
mod zsh_startup;

use crate::platform::tools;
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use anyhow::{Context as _, Result};
use futures::channel::mpsc;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Screen contents changed.
    Wakeup,
    Title(String),
    Bell,
    /// OSC 52 clipboard store requested by the program.
    ClipboardStore(String),
    Exited(Option<i32>),
}

/// Whether the last applied theme is light.
pub static LIGHT_THEME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// History length for new sessions, from settings.
pub static SCROLLBACK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(10_000);

#[derive(Clone)]
pub struct Listener {
    raw: crossbeam_channel::Sender<TermEvent>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.raw.send(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines as usize
    }
    fn screen_lines(&self) -> usize {
        self.lines as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

impl From<TermSize> for WindowSize {
    fn from(s: TermSize) -> Self {
        WindowSize {
            num_lines: s.lines,
            num_cols: s.cols,
            cell_width: s.cell_width,
            cell_height: s.cell_height,
        }
    }
}

pub struct TerminalSession {
    pub term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: parking_lot::Mutex<TermSize>,
    pub cwd: PathBuf,
}

impl TerminalSession {
    /// Spawn `program` (the configured shell when `None`) on a new PTY.
    pub fn spawn(
        program: Option<(String, Vec<String>)>,
        cwd: &Path,
        shell_setting: Option<&str>,
        size: TermSize,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Result<Self> {
        let (program, args) = match program {
            Some(p) => p,
            None => {
                let shell = crate::platform::user_shell(shell_setting);
                let args = crate::platform::shell_args(&shell);
                (shell, args)
            }
        };
        let mut env = tools::child_env();
        #[cfg(unix)]
        if is_login_zsh(&program, &args) {
            zsh_startup::apply(&mut env);
        }
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "KERN".into());
        env.insert(
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        );
        // GUI launches on macOS often have no locale, which breaks non-ASCII in shells.
        if !env.contains_key("LANG") && !env.contains_key("LC_ALL") && !env.contains_key("LC_CTYPE")
        {
            env.insert("LANG".into(), "en_US.UTF-8".into());
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let id = next_id();
        let pty = tty::new(&options, size.into(), id).context("cannot open PTY")?;

        let (raw_tx, raw_rx) = crossbeam_channel::unbounded();
        let listener = Listener { raw: raw_tx };
        let config = Config {
            scrolling_history: SCROLLBACK.load(std::sync::atomic::Ordering::Relaxed),
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)
            .context("cannot start terminal loop")?;
        let sender = event_loop.channel();
        let _thread = event_loop.spawn();

        // Answers to terminal queries must not wait for the UI thread.
        let pump_sender = sender.clone();
        let pump_term = term.clone();
        std::thread::Builder::new()
            .name("term-events".into())
            .spawn(move || {
                while let Ok(ev) = raw_rx.recv() {
                    let forward = match ev {
                        TermEvent::Wakeup
                        | TermEvent::MouseCursorDirty
                        | TermEvent::CursorBlinkingChange => Some(SessionEvent::Wakeup),
                        TermEvent::Title(t) => Some(SessionEvent::Title(t)),
                        TermEvent::ResetTitle => Some(SessionEvent::Title(String::new())),
                        TermEvent::Bell => Some(SessionEvent::Bell),
                        TermEvent::ClipboardStore(_, s) => Some(SessionEvent::ClipboardStore(s)),
                        // Programs must not be able to read the user's clipboard silently.
                        TermEvent::ClipboardLoad(_, _) => None,
                        TermEvent::PtyWrite(s) => {
                            let _ = pump_sender.send(Msg::Input(Cow::Owned(s.into_bytes())));
                            None
                        }
                        TermEvent::TextAreaSizeRequest(f) => {
                            let (cols, lines) = {
                                let t = pump_term.lock();
                                (t.columns() as u16, t.screen_lines() as u16)
                            };
                            let reply = f(WindowSize {
                                num_lines: lines,
                                num_cols: cols,
                                cell_width: 8,
                                cell_height: 16,
                            });
                            let _ = pump_sender.send(Msg::Input(Cow::Owned(reply.into_bytes())));
                            None
                        }
                        TermEvent::ColorRequest(index, f) => {
                            let color = pump_term.lock().colors()[index]
                                .unwrap_or_else(|| default_color(index));
                            let _ = pump_sender.send(Msg::Input(Cow::Owned(f(color).into_bytes())));
                            None
                        }
                        TermEvent::ChildExit(status) => Some(SessionEvent::Exited(status.code())),
                        TermEvent::Exit => None,
                    };
                    if let Some(f) = forward
                        && events.unbounded_send(f).is_err()
                    {
                        break;
                    }
                }
            })?;

        Ok(Self {
            term,
            sender,
            size: parking_lot::Mutex::new(size),
            cwd: cwd.to_path_buf(),
        })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    pub fn resize(&self, size: TermSize) {
        let mut cur = self.size.lock();
        if *cur == size || size.cols == 0 || size.lines == 0 {
            return;
        }
        *cur = size;
        self.term.lock().resize(size);
        let _ = self.sender.send(Msg::Resize(size.into()));
    }

    pub fn size(&self) -> TermSize {
        *self.size.lock()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(unix)]
fn is_login_zsh(program: &str, args: &[String]) -> bool {
    args == ["-l"] && Path::new(program).file_name().is_some_and(|n| n == "zsh")
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn default_color(index: usize) -> alacritty_terminal::vte::ansi::Rgb {
    use alacritty_terminal::vte::ansi::Rgb;
    match index {
        // Background
        257 => Rgb {
            r: 0x17,
            g: 0x17,
            b: 0x1b,
        },
        // Foreground and everything else
        _ => Rgb {
            r: 0xe4,
            g: 0xe4,
            b: 0xe8,
        },
    }
}

// These tests drive real Unix processes; Windows is checked on Windows itself.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};
    use futures::StreamExt;
    use std::time::{Duration, Instant};

    fn screen(session: &TerminalSession) -> String {
        let term = session.term.lock();
        let mut out = String::new();
        for l in 0..term.screen_lines() {
            for c in 0..term.columns() {
                out.push(term.grid()[Line(l as i32)][Column(c)].c);
            }
            out.push('\n');
        }
        out
    }

    fn wait_for(session: &TerminalSession, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if screen(session).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    }

    const SIZE: TermSize = TermSize {
        cols: 80,
        lines: 24,
        cell_width: 8,
        cell_height: 16,
    };

    fn sh(
        script: &str,
        cwd: &Path,
        size: TermSize,
    ) -> (TerminalSession, mpsc::UnboundedReceiver<SessionEvent>) {
        let (tx, rx) = mpsc::unbounded();
        let s = TerminalSession::spawn(
            Some(("/bin/sh".into(), vec!["-c".into(), script.into()])),
            cwd,
            None,
            size,
            tx,
        )
        .unwrap();
        (s, rx)
    }

    #[test]
    fn real_pty_cwd_unicode_and_isatty() {
        let d = tempfile::Builder::new()
            .prefix("терминал ")
            .tempdir()
            .unwrap();
        let (s, _rx) = sh(
            "test -t 0 && test -t 1 && echo TTY_OK; pwd; echo Привет; sleep 5",
            d.path(),
            SIZE,
        );
        assert!(wait_for(&s, "TTY_OK"), "{}", screen(&s));
        assert!(wait_for(&s, "терминал"), "{}", screen(&s));
        assert!(wait_for(&s, "Привет"), "{}", screen(&s));
    }

    #[test]
    fn ctrl_c_interrupts_foreground_program() {
        let d = tempfile::tempdir().unwrap();
        let (s, _rx) = sh(
            "trap 'echo GOT_INT; exit 0' INT; echo READY; sleep 30 & wait",
            d.path(),
            SIZE,
        );
        assert!(wait_for(&s, "READY"));
        s.write(vec![0x03]);
        assert!(wait_for(&s, "GOT_INT"), "{}", screen(&s));
    }

    #[test]
    fn resize_reaches_the_program() {
        let d = tempfile::tempdir().unwrap();
        let (s, _rx) = sh("echo READY; read x; stty size; sleep 5", d.path(), SIZE);
        assert!(wait_for(&s, "READY"));
        s.resize(TermSize {
            cols: 100,
            lines: 30,
            cell_width: 8,
            cell_height: 16,
        });
        std::thread::sleep(Duration::from_millis(100));
        s.write(b"\n".to_vec());
        assert!(wait_for(&s, "30 100"), "{}", screen(&s));
    }

    #[test]
    fn ansi_cursor_addressing_and_query_replies() {
        let d = tempfile::tempdir().unwrap();
        // Cursor position query is answered by the terminal: needed by TUI programs.
        let script = r#"printf '\033[2;5HMARK'; stty -echo raw; printf '\033[6n'; dd bs=1 count=6 2>/dev/null | od -c | head -1 > "$0.out"; stty sane; echo DONE; sleep 3"#;
        let (s, _rx) = {
            let (tx, rx) = mpsc::unbounded();
            let path = d.path().join("q");
            let s = TerminalSession::spawn(
                Some((
                    "/bin/sh".into(),
                    vec![
                        "-c".into(),
                        script.into(),
                        path.to_string_lossy().to_string(),
                    ],
                )),
                d.path(),
                None,
                SIZE,
                tx,
            )
            .unwrap();
            (s, rx)
        };
        assert!(wait_for(&s, "DONE"), "{}", screen(&s));
        assert_eq!(&screen(&s).lines().nth(1).unwrap()[4..8], "MARK");
        let reply = std::fs::read_to_string(d.path().join("q.out")).unwrap();
        assert!(reply.contains('['), "{reply}");
    }

    #[test]
    fn exit_status_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let (_s, mut rx) = sh("exit 7", d.path(), SIZE);
        let code = futures::executor::block_on(async {
            while let Some(ev) = rx.next().await {
                if let SessionEvent::Exited(c) = ev {
                    return c;
                }
            }
            None
        });
        assert_eq!(code, Some(7));
    }
}
