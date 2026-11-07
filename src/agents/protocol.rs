//! Codex App Server wire messages (`codex app-server generate-json-schema`,
//! verified against codex-cli 0.167). JSON-RPC 2.0 lite over JSONL, without
//! the `jsonrpc` header. Parsing here is pure, so that it can be tested
//! exhaustively; unknown methods are tolerated, never fatal.
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
    Notification(Notification),
    Request {
        id: Value,
        request: ServerRequest,
    },
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Notification {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted {
        turn_id: String,
    },
    TurnCompleted {
        turn_id: String,
        status: TurnStatus,
        error: Option<String>,
    },
    AgentDelta {
        item_id: String,
        delta: String,
    },
    ItemStarted(Item),
    ItemCompleted(Item),
    CommandOutputDelta {
        item_id: String,
        delta: String,
    },
    TurnDiffUpdated {
        diff: String,
    },
    Error {
        message: String,
        will_retry: bool,
    },
    Warning(String),
    LoginCompleted {
        login_id: Option<String>,
        success: bool,
        error: Option<String>,
    },
    AccountUpdated {
        auth_mode: Option<String>,
        plan: Option<String>,
    },
    ServerRequestResolved,
    RateLimitsUpdated(Value),
    TokenUsage(Value),
    ModelRerouted {
        from: String,
        to: String,
    },
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeEntry {
    pub path: String,
    pub kind: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    AgentMessage {
        id: String,
        text: String,
    },
    UserMessage {
        id: String,
        text: String,
        images: Vec<String>,
    },
    Reasoning {
        id: String,
        summary: String,
    },
    Command {
        id: String,
        command: String,
        status: String,
        exit_code: Option<i64>,
        output: String,
    },
    FileChange {
        id: String,
        status: String,
        changes: Vec<FileChangeEntry>,
    },
    Plan {
        id: String,
        text: String,
    },
    Tool {
        id: String,
        name: String,
        status: String,
    },
    Other {
        id: String,
        kind: String,
    },
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::AgentMessage { id, .. }
            | Item::UserMessage { id, .. }
            | Item::Reasoning { id, .. }
            | Item::Command { id, .. }
            | Item::FileChange { id, .. }
            | Item::Plan { id, .. }
            | Item::Tool { id, .. }
            | Item::Other { id, .. } => id,
        }
    }
}

/// A request the server is waiting on.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerRequest {
    CommandApproval {
        item_id: String,
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
        network_host: Option<String>,
        offers_session: bool,
    },
    FileChangeApproval {
        item_id: String,
        reason: Option<String>,
        grant_root: Option<String>,
    },
    Permissions {
        summary: String,
        reason: Option<String>,
        permissions: Value,
    },
    UserInput {
        item_id: String,
        questions: Vec<Question>,
    },
    Elicitation {
        server: String,
        message: String,
    },
    /// Not handled by this client; answered with an error so that the server does not hang.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<String>,
    pub secret: bool,
}

/// A decision on an approval request. Maps to the documented decision strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

impl Decision {
    pub fn wire(self) -> &'static str {
        match self {
            Decision::Accept => "accept",
            Decision::AcceptForSession => "acceptForSession",
            Decision::Decline => "decline",
            Decision::Cancel => "cancel",
        }
    }
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

pub fn parse_line(line: &str) -> Incoming {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Incoming::Invalid("not JSON".to_string());
    };
    parse_message(v)
}

pub fn parse_message(v: Value) -> Incoming {
    let method = v.get("method").and_then(Value::as_str).map(str::to_string);
    let id = v.get("id").filter(|i| !i.is_null()).cloned();
    match (id, method) {
        (Some(id), Some(method)) => {
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            Incoming::Request {
                id,
                request: parse_request(&method, &params),
            }
        }
        (Some(id), None) => {
            let result = match v.get("error") {
                Some(e) => Err(RpcError {
                    code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: s(e, "message").unwrap_or_else(|| "error".into()),
                }),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            Incoming::Response { id, result }
        }
        (None, Some(method)) => {
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            Incoming::Notification(parse_notification(&method, &params))
        }
        (None, None) => Incoming::Invalid("message without id or method".to_string()),
    }
}

pub fn parse_notification(method: &str, p: &Value) -> Notification {
    match method {
        "thread/started" => Notification::ThreadStarted {
            thread_id: p.get("thread").and_then(|t| s(t, "id")).unwrap_or_default(),
        },
        "turn/started" => Notification::TurnStarted {
            turn_id: p.get("turn").and_then(|t| s(t, "id")).unwrap_or_default(),
        },
        "turn/completed" => {
            let turn = p.get("turn").cloned().unwrap_or(Value::Null);
            Notification::TurnCompleted {
                turn_id: s(&turn, "id").unwrap_or_default(),
                status: match turn.get("status").and_then(Value::as_str) {
                    Some("completed") => TurnStatus::Completed,
                    Some("interrupted") => TurnStatus::Interrupted,
                    Some("inProgress") => TurnStatus::InProgress,
                    _ => TurnStatus::Failed,
                },
                error: turn.get("error").and_then(|e| s(e, "message")),
            }
        }
        "item/agentMessage/delta" => Notification::AgentDelta {
            item_id: s(p, "itemId").unwrap_or_default(),
            delta: s(p, "delta").unwrap_or_default(),
        },
        "item/started" => {
            Notification::ItemStarted(parse_item(p.get("item").unwrap_or(&Value::Null)))
        }
        "item/completed" => {
            Notification::ItemCompleted(parse_item(p.get("item").unwrap_or(&Value::Null)))
        }
        "item/commandExecution/outputDelta" => Notification::CommandOutputDelta {
            item_id: s(p, "itemId").unwrap_or_default(),
            delta: s(p, "delta").unwrap_or_default(),
        },
        "turn/diff/updated" => Notification::TurnDiffUpdated {
            diff: s(p, "diff").unwrap_or_default(),
        },
        "error" => Notification::Error {
            message: p
                .get("error")
                .and_then(|e| s(e, "message"))
                .unwrap_or_else(|| "Codex error".into()),
            will_retry: p.get("willRetry").and_then(Value::as_bool).unwrap_or(false),
        },
        // Addressed to client developers: it means nothing to someone reading a chat.
        "deprecationNotice" => {
            log::info!("codex: {}", s(p, "summary").unwrap_or_default());
            Notification::Other(method.to_string())
        }
        "warning" | "configWarning" | "guardianWarning" => Notification::Warning(
            s(p, "message")
                .or_else(|| s(p, "summary"))
                .unwrap_or_else(|| method.to_string()),
        ),
        "account/rateLimits/updated" => {
            Notification::RateLimitsUpdated(p.get("rateLimits").cloned().unwrap_or(Value::Null))
        }
        "thread/tokenUsage/updated" => Notification::TokenUsage(p.clone()),
        "model/rerouted" => Notification::ModelRerouted {
            from: s(p, "fromModel").unwrap_or_default(),
            to: s(p, "toModel").unwrap_or_default(),
        },
        "account/login/completed" => Notification::LoginCompleted {
            login_id: s(p, "loginId"),
            success: p.get("success").and_then(Value::as_bool).unwrap_or(false),
            error: s(p, "error"),
        },
        "account/updated" => Notification::AccountUpdated {
            auth_mode: s(p, "authMode"),
            plan: s(p, "planType"),
        },
        "serverRequest/resolved" => Notification::ServerRequestResolved,
        other => Notification::Other(other.to_string()),
    }
}

pub fn parse_item(v: &Value) -> Item {
    let id = s(v, "id").unwrap_or_default();
    let kind = s(v, "type").unwrap_or_default();
    let status = s(v, "status").unwrap_or_default();
    match kind.as_str() {
        "agentMessage" => Item::AgentMessage {
            id,
            text: s(v, "text").unwrap_or_default(),
        },
        "userMessage" => Item::UserMessage {
            id,
            images: v
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|c| s(c, "type").as_deref() == Some("localImage"))
                .filter_map(|c| s(c, "path"))
                .collect(),
            text: v
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|c| s(c, "text"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        },
        "reasoning" => Item::Reasoning {
            id,
            summary: v
                .get("summary")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        },
        "commandExecution" => Item::Command {
            id,
            command: s(v, "command").unwrap_or_default(),
            status,
            exit_code: v.get("exitCode").and_then(Value::as_i64),
            output: s(v, "aggregatedOutput").unwrap_or_default(),
        },
        "fileChange" => Item::FileChange {
            id,
            status,
            changes: v
                .get("changes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|c| FileChangeEntry {
                            path: s(c, "path").unwrap_or_default(),
                            kind: c
                                .get("kind")
                                .and_then(|k| {
                                    s(k, "type").or_else(|| k.as_str().map(str::to_string))
                                })
                                .unwrap_or_default(),
                            diff: s(c, "diff").unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "plan" => Item::Plan {
            id,
            text: s(v, "text").unwrap_or_default(),
        },
        "mcpToolCall" | "dynamicToolCall" => Item::Tool {
            id,
            name: s(v, "tool").unwrap_or_default(),
            status,
        },
        "webSearch" => Item::Tool {
            id,
            name: format!("web search: {}", s(v, "query").unwrap_or_default()),
            status,
        },
        _ => Item::Other { id, kind },
    }
}

pub fn parse_request(method: &str, p: &Value) -> ServerRequest {
    match method {
        "item/commandExecution/requestApproval" => ServerRequest::CommandApproval {
            item_id: s(p, "itemId").unwrap_or_default(),
            command: s(p, "command"),
            cwd: s(p, "cwd"),
            reason: s(p, "reason"),
            network_host: p.get("networkApprovalContext").and_then(|n| s(n, "host")),
            offers_session: true,
        },
        "item/fileChange/requestApproval" => ServerRequest::FileChangeApproval {
            item_id: s(p, "itemId").unwrap_or_default(),
            reason: s(p, "reason"),
            grant_root: s(p, "grantRoot"),
        },
        "item/permissions/requestApproval" => {
            let perms = p.get("permissions").cloned().unwrap_or(Value::Null);
            ServerRequest::Permissions {
                summary: serde_json::to_string_pretty(&perms).unwrap_or_default(),
                reason: s(p, "reason"),
                permissions: perms,
            }
        }
        "item/tool/requestUserInput" => ServerRequest::UserInput {
            item_id: s(p, "itemId").unwrap_or_default(),
            questions: p
                .get("questions")
                .and_then(Value::as_array)
                .map(|qs| {
                    qs.iter()
                        .map(|q| Question {
                            id: s(q, "id").unwrap_or_default(),
                            header: s(q, "header").unwrap_or_default(),
                            question: s(q, "question").unwrap_or_default(),
                            options: q
                                .get("options")
                                .and_then(Value::as_array)
                                .map(|o| o.iter().filter_map(|x| s(x, "label")).collect())
                                .unwrap_or_default(),
                            secret: q.get("isSecret").and_then(Value::as_bool).unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "mcpServer/elicitation/request" => ServerRequest::Elicitation {
            server: s(p, "serverName").unwrap_or_default(),
            message: s(p, "message").unwrap_or_default(),
        },
        other => ServerRequest::Unsupported(other.to_string()),
    }
}

/// Reply payload for a decision on a pending request.
pub fn decision_response(request: &ServerRequest, decision: Decision) -> Value {
    match request {
        ServerRequest::CommandApproval { .. } | ServerRequest::FileChangeApproval { .. } => {
            json!({"decision": decision.wire()})
        }
        ServerRequest::Permissions { permissions, .. } => match decision {
            Decision::Accept => json!({"permissions": permissions, "scope": "turn"}),
            Decision::AcceptForSession => json!({"permissions": permissions, "scope": "session"}),
            // An empty grant is the documented refusal.
            Decision::Decline | Decision::Cancel => json!({"permissions": {}, "scope": "turn"}),
        },
        ServerRequest::Elicitation { .. } => match decision {
            Decision::Accept | Decision::AcceptForSession => {
                json!({"action": "accept", "content": null})
            }
            Decision::Decline => json!({"action": "decline", "content": null}),
            Decision::Cancel => json!({"action": "cancel", "content": null}),
        },
        ServerRequest::UserInput { .. } | ServerRequest::Unsupported(_) => json!({}),
    }
}

/// Answers for `item/tool/requestUserInput`.
pub fn user_input_response(answers: &[(String, String)]) -> Value {
    let map: serde_json::Map<String, Value> = answers
        .iter()
        .map(|(id, a)| (id.clone(), json!({"answers": [a]})))
        .collect();
    json!({"answers": map})
}

/// Whether the login state requires action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountState {
    /// The active provider needs no OpenAI credentials, or a session exists.
    Connected {
        description: String,
    },
    LoginRequired,
}

pub fn parse_account(result: &Value) -> AccountState {
    let account = result.get("account").filter(|a| !a.is_null());
    let requires = result
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    match account {
        Some(a) => {
            let kind = s(a, "type").unwrap_or_default();
            let description = match kind.as_str() {
                "chatgpt" => {
                    let plan = s(a, "planType").unwrap_or_default();
                    match s(a, "email") {
                        Some(e) => format!("ChatGPT {plan} · {e}"),
                        None => format!("ChatGPT {plan}"),
                    }
                }
                "apiKey" => "API key".to_string(),
                other => other.to_string(),
            };
            AccountState::Connected { description }
        }
        None if !requires => AccountState::Connected {
            description: "no sign-in required by the active provider".into(),
        },
        None => AccountState::LoginRequired,
    }
}

/// All items of a `thread/resume` or `thread/read` result, in order.
pub fn thread_items(thread: &Value) -> Vec<Item> {
    thread
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|t| {
            t.get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .map(parse_item)
        .collect()
}

/// What the user typed: KERN sends attachments as tagged blocks before the text.
pub fn display_user_text(text: &str) -> String {
    let mut rest = text;
    for close in ["</file>\n\n", "</selection>\n\n", "</output>\n\n"] {
        if let Some(ix) = rest.rfind(close) {
            rest = &rest[ix + close.len()..];
        }
    }
    rest.trim().to_string()
}

pub fn initialize_params(version: &str) -> Value {
    json!({
        "clientInfo": {"name": "kern_ide", "title": "KERN", "version": version},
        "capabilities": {"experimentalApi": false}
    })
}

/// No `approvalPolicy` / `sandbox`: Codex's own configured limits stay in force.
pub fn thread_start_params(cwd: &str) -> Value {
    json!({"cwd": cwd, "serviceName": "kern_ide"})
}

/// `model` and `effort` apply to this turn and later ones. `None` keeps what is set.
pub fn turn_start_params(
    thread_id: &str,
    text: &str,
    images: &[std::path::PathBuf],
    cwd: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let mut input = vec![json!({"type": "text", "text": text})];
    for path in images {
        // A real image for the model: the schema's `localImage`, not a path in text.
        input.push(json!({"type": "localImage", "path": path.to_string_lossy()}));
    }
    let mut params = json!({"threadId": thread_id, "cwd": cwd, "input": input});
    if let Some(m) = model {
        params["model"] = json!(m);
    }
    if let Some(e) = effort {
        params["effort"] = json!(e);
    }
    params
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    #[test]
    fn images_go_as_local_image_inputs() {
        let p = turn_start_params(
            "t",
            "what is this?",
            &["/tmp/a.png".into()],
            "/p",
            Some("m"),
            Some("high"),
        );
        assert_eq!(p["input"][0]["text"], "what is this?");
        assert_eq!(p["input"][1]["type"], "localImage");
        assert_eq!(p["input"][1]["path"], "/tmp/a.png");
        assert_eq!(p["model"], "m");
        assert_eq!(p["effort"], "high");
    }

    #[test]
    fn stored_turns_keep_their_images() {
        let items = thread_items(&json!({"turns":[{"items":[{"id":"u","type":"userMessage",
            "content":[{"type":"text","text":"q"},{"type":"localImage","path":"/a.png"}]}]}]}));
        assert!(
            matches!(&items[0], Item::UserMessage { images, .. } if images == &vec!["/a.png".to_string()])
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_messages_by_shape() {
        assert!(matches!(
            parse_line(r#"{"id":1,"result":{"x":1}}"#),
            Incoming::Response { result: Ok(_), .. }
        ));
        assert!(matches!(
            parse_line(r#"{"id":"a","error":{"code":-1,"message":"no"}}"#),
            Incoming::Response {
                result: Err(RpcError { code: -1, .. }),
                ..
            }
        ));
        assert!(matches!(
            parse_line(r#"{"method":"turn/started","params":{"turn":{"id":"t"}}}"#),
            Incoming::Notification(Notification::TurnStarted { .. })
        ));
        assert!(matches!(
            parse_line(
                r#"{"id":5,"method":"item/fileChange/requestApproval","params":{"itemId":"i"}}"#
            ),
            Incoming::Request { .. }
        ));
        assert!(matches!(parse_line("garbage"), Incoming::Invalid(_)));
        assert!(matches!(parse_line("{}"), Incoming::Invalid(_)));
    }

    #[test]
    fn streaming_and_turn_lifecycle() {
        let Incoming::Notification(Notification::AgentDelta { item_id, delta }) = parse_line(
            r#"{"method":"item/agentMessage/delta","params":{"delta":"При","itemId":"i1","threadId":"t","turnId":"u"}}"#,
        ) else {
            panic!()
        };
        assert_eq!((item_id.as_str(), delta.as_str()), ("i1", "При"));

        let n = parse_notification(
            "turn/completed",
            &json!({"threadId":"t","turn":{"id":"u","status":"failed","error":{"message":"quota"},"items":[]}}),
        );
        assert_eq!(
            n,
            Notification::TurnCompleted {
                turn_id: "u".into(),
                status: TurnStatus::Failed,
                error: Some("quota".into())
            }
        );
        let n = parse_notification(
            "turn/completed",
            &json!({"turn":{"id":"u","status":"interrupted","items":[]}}),
        );
        assert!(matches!(
            n,
            Notification::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            }
        ));
    }

    #[test]
    fn unknown_notifications_are_tolerated() {
        assert_eq!(
            parse_notification("thread/realtime/sdp", &json!({})),
            Notification::Other("thread/realtime/sdp".into())
        );
    }

    #[test]
    fn items() {
        let item = parse_item(
            &json!({"type":"fileChange","id":"f","status":"completed","changes":[{"path":"a.ts","kind":{"type":"update"},"diff":"@@"}]}),
        );
        let Item::FileChange { changes, .. } = item else {
            panic!()
        };
        assert_eq!(
            changes[0],
            FileChangeEntry {
                path: "a.ts".into(),
                kind: "update".into(),
                diff: "@@".into()
            }
        );
        let item = parse_item(
            &json!({"type":"commandExecution","id":"c","command":"ls","status":"completed","exitCode":0,"aggregatedOutput":"x","commandActions":[],"cwd":"/"}),
        );
        assert!(matches!(
            item,
            Item::Command {
                exit_code: Some(0),
                ..
            }
        ));
        assert!(matches!(
            parse_item(&json!({"type":"somethingNew","id":"n"})),
            Item::Other { .. }
        ));
    }

    #[test]
    fn approvals_roundtrip_with_documented_decisions() {
        let Incoming::Request { id, request } = parse_line(
            r#"{"id":9,"method":"item/commandExecution/requestApproval","params":{"itemId":"i","threadId":"t","turnId":"u","startedAtMs":1,"command":"rm -rf x","cwd":"/p","reason":"need"}}"#,
        ) else {
            panic!()
        };
        assert_eq!(id, json!(9));
        let ServerRequest::CommandApproval {
            command,
            cwd,
            reason,
            ..
        } = &request
        else {
            panic!()
        };
        assert_eq!(command.as_deref(), Some("rm -rf x"));
        assert_eq!(cwd.as_deref(), Some("/p"));
        assert_eq!(reason.as_deref(), Some("need"));
        assert_eq!(
            decision_response(&request, Decision::Accept),
            json!({"decision":"accept"})
        );
        assert_eq!(
            decision_response(&request, Decision::AcceptForSession),
            json!({"decision":"acceptForSession"})
        );
        assert_eq!(
            decision_response(&request, Decision::Decline),
            json!({"decision":"decline"})
        );
        assert_eq!(
            decision_response(&request, Decision::Cancel),
            json!({"decision":"cancel"})
        );

        let net = parse_request(
            "item/commandExecution/requestApproval",
            &json!({"networkApprovalContext":{"host":"x.dev","protocol":"https"}}),
        );
        assert!(
            matches!(net, ServerRequest::CommandApproval { network_host: Some(h), .. } if h == "x.dev")
        );

        let perm = parse_request(
            "item/permissions/requestApproval",
            &json!({"permissions":{"network":{"enabled":true}},"reason":"r"}),
        );
        assert_eq!(
            decision_response(&perm, Decision::Accept)["permissions"]["network"]["enabled"],
            true
        );
        assert_eq!(
            decision_response(&perm, Decision::Decline)["permissions"],
            json!({})
        );
    }

    #[test]
    fn unknown_server_requests_stay_unsupported() {
        assert!(matches!(
            parse_request("account/chatgptAuthTokens/refresh", &json!({})),
            ServerRequest::Unsupported(_)
        ));
    }

    #[test]
    fn user_input() {
        let r = parse_request(
            "item/tool/requestUserInput",
            &json!({"itemId":"i","questions":[{"id":"q","header":"H","question":"Why?","isSecret":true,"options":[{"label":"A","description":""}]}]}),
        );
        let ServerRequest::UserInput { questions, .. } = r else {
            panic!()
        };
        assert_eq!(questions[0].options, vec!["A"]);
        assert!(questions[0].secret);
        assert_eq!(
            user_input_response(&[("q".into(), "A".into())]),
            json!({"answers":{"q":{"answers":["A"]}}})
        );
    }

    #[test]
    fn history_items_and_user_text() {
        let thread = json!({"turns":[
            {"items":[{"type":"userMessage","id":"u","content":[{"type":"text","text":"<file path=\"a\">\nx\n</file>\n\nпочини"}]},{"type":"agentMessage","id":"a","text":"готово"}]},
            {"items":[{"type":"commandExecution","id":"c","command":"ls","status":"completed"}]}
        ]});
        let items = thread_items(&thread);
        assert_eq!(items.len(), 3);
        let Item::UserMessage { text, .. } = &items[0] else {
            panic!()
        };
        assert_eq!(display_user_text(text), "почини");
        assert!(matches!(&items[1], Item::AgentMessage { text, .. } if text == "готово"));
        assert!(thread_items(&json!({})).is_empty());
        assert_eq!(display_user_text("просто текст"), "просто текст");
    }

    #[test]
    fn account_states() {
        assert_eq!(
            parse_account(&json!({"account":null,"requiresOpenaiAuth":true})),
            AccountState::LoginRequired
        );
        assert!(matches!(
            parse_account(&json!({"account":null,"requiresOpenaiAuth":false})),
            AccountState::Connected { .. }
        ));
        let AccountState::Connected { description } = parse_account(
            &json!({"account":{"type":"chatgpt","email":"a@b","planType":"plus"},"requiresOpenaiAuth":true}),
        ) else {
            panic!()
        };
        assert_eq!(description, "ChatGPT plus · a@b");
    }

    #[test]
    fn thread_start_does_not_loosen_codex_limits() {
        let p = thread_start_params("/p");
        assert!(p.get("approvalPolicy").is_none());
        assert!(p.get("sandbox").is_none());
    }
}
