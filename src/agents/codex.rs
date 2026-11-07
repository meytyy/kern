//! The Codex App Server process. Started only when the panel needs it. Message
//! bodies are never logged: they may contain code or account information.
use super::protocol::{self, Incoming, Notification, RpcError, ServerRequest};
use crate::platform::{self, tools};
use anyhow::{Result, anyhow};
use futures::channel::{mpsc, oneshot};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum CodexEvent {
    Notification(Notification),
    Request { id: Value, request: ServerRequest },
    Closed(String),
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub struct CodexClient {
    out: crossbeam_channel::Sender<String>,
    pending: Pending,
    next_id: AtomicI64,
    pgid: u32,
}

impl Drop for CodexClient {
    fn drop(&mut self) {
        let pgid = self.pgid;
        std::thread::spawn(move || platform::terminate_group(pgid, Duration::from_secs(1)));
    }
}

/// Availability of the CLI, for the "not found" state of the panel.
pub fn cli_status() -> tools::ToolStatus {
    tools::locate("codex")
}

impl CodexClient {
    /// Start `codex app-server` and perform the initialize handshake (blocking:
    /// call off the UI thread).
    pub fn start(cwd: &Path, events: mpsc::UnboundedSender<CodexEvent>) -> Result<Arc<Self>> {
        tools::wait_for_environment();
        let program = tools::find("codex").ok_or_else(|| anyhow!("Codex CLI not found"))?;
        let mut cmd = Command::new(program);
        cmd.arg("app-server");
        // Test builds can tighten the approval policy to exercise real approvals.
        #[cfg(feature = "autotest")]
        if let Ok(extra) = std::env::var("KERN_CODEX_CONFIG") {
            for item in extra.split(';') {
                cmd.args(["-c", item]);
            }
        }
        cmd.current_dir(cwd)
            .env_clear()
            .envs(tools::child_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::new_process_group(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("cannot start Codex: {e}"))?;
        let pgid = child.id();
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

        let (out_tx, out_rx) = crossbeam_channel::unbounded::<String>();
        std::thread::Builder::new()
            .name("codex-writer".into())
            .spawn(move || {
                while let Ok(line) = out_rx.recv() {
                    if stdin.write_all(line.as_bytes()).is_err()
                        || stdin.write_all(b"\n").is_err()
                        || stdin.flush().is_err()
                    {
                        break;
                    }
                }
            })?;
        let tail: Arc<Mutex<Vec<String>>> = Default::default();
        let tail2 = tail.clone();
        let tail_for_start = tail.clone();
        std::thread::Builder::new()
            .name("codex-stderr".into())
            .spawn(move || {
                // Kept only in memory, for a readable message if the process dies.
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut t = tail2.lock();
                    t.push(line);
                    if t.len() > 20 {
                        t.remove(0);
                    }
                }
            })?;

        let pending: Pending = Default::default();
        let client = Arc::new(Self {
            out: out_tx,
            pending: pending.clone(),
            next_id: AtomicI64::new(1),
            pgid,
        });

        let ev = events;
        std::thread::Builder::new()
            .name("codex-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    match protocol::parse_line(&line) {
                        Incoming::Response { id, result } => {
                            if let Some(tx) = id.as_i64().and_then(|i| pending.lock().remove(&i)) {
                                let _ = tx.send(result);
                            }
                        }
                        Incoming::Notification(n) => {
                            let _ = ev.unbounded_send(CodexEvent::Notification(n));
                        }
                        Incoming::Request { id, request } => {
                            let _ = ev.unbounded_send(CodexEvent::Request { id, request });
                        }
                        Incoming::Invalid(why) => log::warn!("codex: invalid message: {why}"),
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code());
                for (_, tx) in pending.lock().drain() {
                    let _ = tx.send(Err(RpcError {
                        code: -1,
                        message: "Codex process ended".into(),
                    }));
                }
                let detail = tail.lock().last().cloned().unwrap_or_default();
                let _ = ev.unbounded_send(CodexEvent::Closed(match code {
                    Some(0) | None => "Codex process ended".to_string(),
                    Some(c) => format!("Codex exited with code {c}. {detail}"),
                }));
            })?;

        futures::executor::block_on(client.call(
            "initialize",
            protocol::initialize_params(env!("CARGO_PKG_VERSION")),
        ))
        .map_err(|e| {
            // Give the stderr reader a moment: its last line says what went wrong.
            std::thread::sleep(Duration::from_millis(150));
            // A backtrace may follow: its frames say nothing, the error line does.
            let detail = {
                let t = tail_for_start.lock();
                t.iter()
                    .find(|l| l.starts_with("Error"))
                    .or(t.last())
                    .cloned()
                    .unwrap_or(e.message)
            };
            anyhow!("Codex could not start: {detail}")
        })?;
        client.send(&json!({"method": "initialized"}));
        Ok(client)
    }

    fn send(&self, msg: &Value) {
        let _ = self.out.send(msg.to_string());
    }

    pub fn call(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<Value, RpcError>> + use<> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        self.send(&json!({"id": id, "method": method, "params": params}));
        async move {
            rx.await.unwrap_or_else(|_| {
                Err(RpcError {
                    code: -1,
                    message: "Codex process ended".into(),
                })
            })
        }
    }

    pub fn respond(&self, id: Value, result: Value) {
        self.send(&json!({"id": id, "result": result}));
    }

    /// Tell the server this client cannot serve a request.
    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.send(&json!({"id": id, "error": {"code": code, "message": message}}));
    }
}
