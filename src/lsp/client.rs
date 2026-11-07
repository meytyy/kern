//! A running language server. All I/O happens on background threads; callers
//! get results through futures and events through a channel.
use super::position::Encoding;
use super::transport::{read_message, write_message};
use crate::platform::{self, tools};
use anyhow::{Result, anyhow};
use futures::channel::{mpsc, oneshot};
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    Starting,
    Ready,
    Missing { hint: String },
    Failed(String),
    Exited,
}

#[derive(Debug, Clone)]
pub enum LspEvent {
    Status {
        server: String,
        status: ServerStatus,
    },
    Diagnostics {
        uri: String,
        diagnostics: Value,
    },
}

const SERVER_EXITED: &str = "language server exited";

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct LspClient {
    pub server_id: String,
    next_id: AtomicI64,
    pending: Pending,
    out: crossbeam_channel::Sender<Vec<u8>>,
    capabilities: Arc<RwLock<Value>>,
    encoding: Arc<RwLock<Encoding>>,
    initialized: Arc<AtomicBool>,
    held: Arc<Mutex<Vec<Vec<u8>>>>,
    pgid: u32,
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let pgid = self.pgid;
        let out = self.out.clone();
        // Polite shutdown is best-effort; the process group is killed regardless.
        let _ = out.send(
            serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"exit"})).unwrap_or_default(),
        );
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            platform::terminate_group(pgid, Duration::from_millis(500));
        });
    }
}

fn client_capabilities() -> Value {
    json!({
        "general": {"positionEncodings": ["utf-8", "utf-16"]},
        "window": {"workDoneProgress": false, "showMessage": {}},
        "workspace": {"configuration": true, "workspaceFolders": true, "didChangeConfiguration": {"dynamicRegistration": false}},
        "textDocument": {
            "synchronization": {"didSave": true, "dynamicRegistration": false},
            "completion": {
                "contextSupport": true,
                "completionItem": {
                    "snippetSupport": false,
                    "documentationFormat": ["markdown", "plaintext"],
                    "labelDetailsSupport": true,
                    "resolveSupport": {"properties": ["documentation", "detail"]}
                }
            },
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "definition": {"linkSupport": true},
            "publishDiagnostics": {"relatedInformation": false, "versionSupport": true},
            "diagnostic": {"dynamicRegistration": false, "relatedDocumentSupport": false}
        }
    })
}

/// Answer `workspace/configuration` from the server's settings object.
pub fn configuration_response(settings: &Value, params: &Value) -> Value {
    let items = params
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Value::Array(
        items
            .iter()
            .map(|item| {
                let Some(section) = item.get("section").and_then(Value::as_str) else {
                    return settings.clone();
                };
                let mut cur = settings;
                for part in section.split('.') {
                    match cur.get(part) {
                        Some(v) => cur = v,
                        None => return Value::Null,
                    }
                }
                cur.clone()
            })
            .collect(),
    )
}

pub struct StartOptions<'a> {
    pub server_id: &'a str,
    pub program: &'a Path,
    pub args: &'a [String],
    pub root: &'a Path,
    pub init_options: Value,
    pub settings: Value,
    pub install_hint: &'a str,
}

impl LspClient {
    pub fn start(
        opts: StartOptions<'_>,
        events: mpsc::UnboundedSender<LspEvent>,
    ) -> Result<Arc<Self>> {
        let mut cmd = Command::new(opts.program);
        cmd.args(opts.args)
            .current_dir(opts.root)
            .env_clear()
            .envs(tools::child_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::new_process_group(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("cannot start {}: {e}", opts.program.display()))?;
        let pgid = child.id();
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

        let (out_tx, out_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
        std::thread::Builder::new()
            .name("lsp-writer".into())
            .spawn(move || {
                while let Ok(msg) = out_rx.recv() {
                    if write_message(&mut stdin, &msg).is_err() {
                        break;
                    }
                }
            })?;

        let server_id = opts.server_id.to_string();
        let sid = server_id.clone();
        let stderr_tail: Arc<Mutex<String>> = Default::default();
        let tail = stderr_tail.clone();
        std::thread::Builder::new()
            .name("lsp-stderr".into())
            .spawn(move || {
                let mut r = stderr;
                let mut buf = [0u8; 4096];
                while let Ok(n) = r.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&buf[..n]);
                    log::debug!("{sid}: {text}");
                    let mut t = tail.lock();
                    t.push_str(&text);
                    if t.len() > 2000 {
                        let cut = t.len() - 2000;
                        let cut = (cut..t.len())
                            .find(|i| t.is_char_boundary(*i))
                            .unwrap_or(cut);
                        t.drain(..cut);
                    }
                }
            })?;

        let client = Arc::new(Self {
            server_id: server_id.clone(),
            next_id: AtomicI64::new(1),
            pending: Default::default(),
            out: out_tx,
            capabilities: Default::default(),
            encoding: Default::default(),
            initialized: Default::default(),
            held: Default::default(),
            pgid,
        });

        let pending = client.pending.clone();
        let reply = client.out.clone();
        let ev = events.clone();
        let sid = server_id.clone();
        let settings = opts.settings.clone();
        let root_uri = super::path_to_uri(opts.root);
        let was_initialized = client.initialized.clone();
        let hint = opts.install_hint.to_string();
        std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    match read_message(&mut reader) {
                        Ok(Some(bytes)) => {
                            let Ok(msg) = serde_json::from_slice::<Value>(&bytes) else {
                                log::warn!("{sid}: invalid JSON from server");
                                continue;
                            };
                            dispatch(msg, &pending, &reply, &ev, &settings, &root_uri);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            log::warn!("{sid}: {e}");
                            break;
                        }
                    }
                }
                for (_, tx) in pending.lock().drain() {
                    let _ = tx.send(Err(SERVER_EXITED.into()));
                }
                let status = child.wait().ok();
                let started = was_initialized.load(Ordering::SeqCst);
                // A server that dies during startup is usually not really installed.
                std::thread::sleep(Duration::from_millis(50));
                let detail = stderr_tail
                    .lock()
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let _ = ev.unbounded_send(LspEvent::Status {
                    server: sid.clone(),
                    status: match status {
                        _ if !started => {
                            ServerStatus::Failed(format!("{detail} · Install: {hint}"))
                        }
                        Some(s) if !s.success() => {
                            ServerStatus::Failed(format!("exited with {s}. {detail}"))
                        }
                        _ => ServerStatus::Exited,
                    },
                });
            })?;

        // Initialize asynchronously; messages sent meanwhile are held back.
        let init_rx = client.send_request_now(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "KERN", "version": env!("CARGO_PKG_VERSION")},
                "rootUri": super::path_to_uri(opts.root),
                "rootPath": opts.root,
                "workspaceFolders": [{"uri": super::path_to_uri(opts.root), "name": opts.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()}],
                "capabilities": client_capabilities(),
                "initializationOptions": opts.init_options,
            }),
        );
        let weak = Arc::downgrade(&client);
        let sid = server_id.clone();
        let settings = opts.settings;
        std::thread::Builder::new().name("lsp-init".into()).spawn(move || {
            let result = futures::executor::block_on(init_rx);
            let Some(client) = weak.upgrade() else { return };
            match result {
                Ok(Ok(res)) => {
                    let caps = res.get("capabilities").cloned().unwrap_or(Value::Null);
                    *client.encoding.write() = Encoding::from_capabilities(&caps);
                    *client.capabilities.write() = caps;
                    client.send_raw(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
                    if !settings.is_null() {
                        client.send_raw(json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{"settings":settings}}));
                    }
                    {
                        let mut held = client.held.lock();
                        client.initialized.store(true, Ordering::SeqCst);
                        for m in held.drain(..) {
                            let _ = client.out.send(m);
                        }
                    }
                    let _ = events.unbounded_send(LspEvent::Status {
                        server: sid,
                        status: ServerStatus::Ready,
                    });
                }
                // A dead process is reported by the reader, with its stderr and install hint.
                Ok(Err(e)) if e == SERVER_EXITED => {}
                Ok(Err(e)) => {
                    let _ = events.unbounded_send(LspEvent::Status {
                        server: sid,
                        status: ServerStatus::Failed(e),
                    });
                }
                Err(_) => {}
            }
        })?;
        Ok(client)
    }

    pub fn capabilities(&self) -> Value {
        self.capabilities.read().clone()
    }

    pub fn encoding(&self) -> Encoding {
        *self.encoding.read()
    }

    pub fn supports(&self, capability: &str) -> bool {
        let caps = self.capabilities.read();
        // Before initialization finished, allow; requests are held back until then.
        if caps.is_null() {
            return true;
        }
        caps.get(capability)
            .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
    }

    fn send_raw(&self, msg: Value) {
        if let Ok(bytes) = serde_json::to_vec(&msg) {
            let _ = self.out.send(bytes);
        }
    }

    fn enqueue(&self, msg: Value) {
        let Ok(bytes) = serde_json::to_vec(&msg) else {
            return;
        };
        let mut held = self.held.lock();
        if self.initialized.load(Ordering::SeqCst) {
            let _ = self.out.send(bytes);
        } else {
            held.push(bytes);
        }
    }

    fn send_request_now(
        &self,
        method: &str,
        params: Value,
    ) -> oneshot::Receiver<Result<Value, String>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        self.send_raw(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        rx
    }

    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Result<Value, String>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        self.enqueue(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        rx
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.enqueue(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    pub fn did_open(&self, uri: &str, language_id: &str, version: i32, text: String) {
        self.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":language_id,"version":version,"text":text}}));
    }

    /// Full-text change: valid for every sync kind other than None.
    pub fn did_change(&self, uri: &str, version: i32, text: String) {
        self.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]}),
        );
    }

    pub fn did_save(&self, uri: &str) {
        self.notify("textDocument/didSave", json!({"textDocument":{"uri":uri}}));
    }

    pub fn did_close(&self, uri: &str) {
        self.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
    }
}

fn dispatch(
    msg: Value,
    pending: &Pending,
    reply: &crossbeam_channel::Sender<Vec<u8>>,
    events: &mpsc::UnboundedSender<LspEvent>,
    settings: &Value,
    root_uri: &str,
) {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str);
    match (id, method) {
        // Response to one of our requests.
        (Some(id), None) => {
            let Some(id) = id.as_i64() else { return };
            if let Some(tx) = pending.lock().remove(&id) {
                let res = match msg.get("error") {
                    Some(err) => Err(err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(res);
            }
        }
        // Request from the server: always answer, or the server may stall.
        (Some(id), Some(method)) => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let response = server_request_result(method, &params, settings, root_uri);
            let body = match response {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(message) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":message}})
                }
            };
            if let Ok(bytes) = serde_json::to_vec(&body) {
                let _ = reply.send(bytes);
            }
        }
        (None, Some("textDocument/publishDiagnostics")) => {
            if let Some(params) = msg.get("params") {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let diagnostics = params.get("diagnostics").cloned().unwrap_or(json!([]));
                let _ = events.unbounded_send(LspEvent::Diagnostics { uri, diagnostics });
            }
        }
        _ => {}
    }
}

pub fn server_request_result(
    method: &str,
    params: &Value,
    settings: &Value,
    root_uri: &str,
) -> Result<Value, String> {
    match method {
        "workspace/configuration" => Ok(configuration_response(settings, params)),
        "workspace/workspaceFolders" => Ok(json!([{"uri": root_uri, "name": "root"}])),
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "window/showMessageRequest" => Ok(Value::Null),
        "workspace/applyEdit" => {
            Ok(json!({"applied": false, "failureReason": "not supported by KERN"}))
        }
        _ => Err(format!("unsupported request: {method}")),
    }
}

// These tests drive real Unix processes; Windows is checked on Windows itself.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn configuration_sections() {
        let settings = json!({"python":{"pythonPath":"/v/bin/python","analysis":{"x":1}}});
        let res = configuration_response(
            &settings,
            &json!({"items":[{"section":"python.analysis"},{"section":"python"},{"section":"nope"},{}]}),
        );
        assert_eq!(res[0], json!({"x":1}));
        assert_eq!(res[1]["pythonPath"], "/v/bin/python");
        assert_eq!(res[2], Value::Null);
        assert_eq!(res[3], settings);
    }

    #[test]
    fn unknown_server_requests_get_error_and_edits_are_refused() {
        assert!(server_request_result("foo/bar", &Value::Null, &Value::Null, "").is_err());
        let r =
            server_request_result("workspace/applyEdit", &Value::Null, &Value::Null, "").unwrap();
        assert_eq!(r["applied"], false);
        assert_eq!(
            server_request_result(
                "window/workDoneProgress/create",
                &Value::Null,
                &Value::Null,
                ""
            )
            .unwrap(),
            Value::Null
        );
    }

    #[test]
    fn startup_crash_reports_stderr_and_install_hint() {
        use futures::StreamExt;
        let (tx, mut rx) = mpsc::unbounded();
        let _c = LspClient::start(
            StartOptions {
                server_id: "fake",
                program: Path::new("/bin/sh"),
                args: &[
                    "-c".to_string(),
                    "echo Unknown binary 1>&2; exit 1".to_string(),
                ],
                root: Path::new("/tmp"),
                init_options: Value::Null,
                settings: Value::Null,
                install_hint: "install it",
            },
            tx,
        )
        .unwrap();
        let msg = futures::executor::block_on(async {
            while let Some(ev) = rx.next().await {
                if let LspEvent::Status {
                    status: ServerStatus::Failed(m),
                    ..
                } = ev
                {
                    return m;
                }
            }
            String::new()
        });
        assert!(
            msg.contains("Unknown binary") && msg.contains("install it"),
            "{msg}"
        );
    }

    /// A real full exchange against a scripted server: handshake, held-back
    /// messages, a server request, diagnostics, and a response.
    #[test]
    fn protocol_exchange_with_scripted_server() {
        use futures::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"
import json, sys
def read():
    n = 0
    while True:
        l = sys.stdin.buffer.readline().decode()
        if l in ("\r\n", ""): break
        if l.lower().startswith("content-length"): n = int(l.split(":")[1])
    return json.loads(sys.stdin.buffer.read(n))
def send(o):
    b = json.dumps(o).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); sys.stdout.buffer.flush()
while True:
    m = read()
    meth = m.get("method")
    if meth == "initialize":
        send({"jsonrpc":"2.0","id":m["id"],"result":{"capabilities":{"positionEncoding":"utf-8","hoverProvider":True}}})
    elif meth == "textDocument/didOpen":
        send({"jsonrpc":"2.0","id":"cfg","method":"workspace/configuration","params":{"items":[{"section":"a"}]}})
    elif m.get("id") == "cfg":
        send({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///x","diagnostics":[{"seen":m["result"][0]}]}})
    elif meth == "textDocument/hover":
        send({"jsonrpc":"2.0","id":m["id"],"result":{"echo":m["params"]}})
    elif meth == "exit":
        sys.exit(0)
"#,
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded();
        let args = vec!["-I".to_string(), script.to_string_lossy().to_string()];
        let c = LspClient::start(
            StartOptions {
                server_id: "py",
                program: Path::new("/usr/bin/python3"),
                args: &args,
                root: dir.path(),
                init_options: Value::Null,
                settings: json!({"a": 42}),
                install_hint: "",
            },
            tx,
        )
        .unwrap();
        // Sent before initialize finished: must be held back, then delivered in order.
        c.did_open("file:///x", "t", 1, "text".into());
        let hover = c.request("textDocument/hover", json!({"p": 1}));
        let reply = futures::executor::block_on(hover).unwrap().unwrap();
        assert_eq!(reply["echo"]["p"], 1);
        assert_eq!(c.encoding(), Encoding::Utf8);
        assert!(c.supports("hoverProvider") && !c.supports("completionProvider"));
        let seen = futures::executor::block_on(async {
            while let Some(ev) = rx.next().await {
                if let LspEvent::Diagnostics { diagnostics, .. } = ev {
                    return diagnostics[0]["seen"].clone();
                }
            }
            Value::Null
        });
        assert_eq!(seen, 42);
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        let (tx, _rx) = mpsc::unbounded();
        let res = LspClient::start(
            StartOptions {
                server_id: "x",
                program: Path::new("/nonexistent/ls"),
                args: &[],
                root: Path::new("/tmp"),
                init_options: Value::Null,
                settings: Value::Null,
                install_hint: "",
            },
            tx,
        );
        assert!(res.is_err());
    }
}
