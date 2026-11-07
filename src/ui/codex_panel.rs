//! Codex behind the Agent Chat: official App Server, real streaming, approvals,
//! Stop, history and diff. The sign-in stays with the Codex CLI.
use super::agent_chat::{ChatState, PendingKind, PendingRequest, SessionError};
use super::agents_panel::AgentTab;
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use crate::agents::chat::{ActionItem, ActionKind, ActionStatus, ChatItem};
use crate::agents::codex::{CodexClient, CodexEvent, cli_status};
use crate::agents::protocol::{
    self, AccountState, Decision, Item, Notification, ServerRequest, TurnStatus,
};
use crate::agents::sessions;
use crate::agents::snapshot;
use crate::agents::usage;
use crate::platform::tools::{self, ToolStatus};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    NotStarted,
    CliMissing { hint: String },
    Starting,
    LoginRequired,
    WaitingForLogin { url: String },
    Connected { description: String },
    Error(String),
}

pub struct CodexState {
    pub conn: Conn,
    pub client: Option<Arc<CodexClient>>,
    pub chat: ChatState,
}

impl std::ops::Deref for CodexState {
    type Target = ChatState;
    fn deref(&self) -> &ChatState {
        &self.chat
    }
}

impl std::ops::DerefMut for CodexState {
    fn deref_mut(&mut self) -> &mut ChatState {
        &mut self.chat
    }
}

impl CodexState {
    pub fn new(window: &mut Window, cx: &mut Context<KernApp>) -> Self {
        Self {
            conn: Conn::NotStarted,
            client: None,
            chat: ChatState::new(AgentTab::Codex, window, cx),
        }
    }
}

/// Draw an item loaded from a resumed thread.
pub fn apply_history_item(c: &mut ChatState, item: Item, root: &std::path::Path) {
    match item {
        Item::UserMessage { text, images, .. } => c.items.push(ChatItem::User {
            text: protocol::display_user_text(&text),
            attachments: vec![],
            images: images.into_iter().map(Into::into).collect(),
        }),
        other => apply_item(c, other, root),
    }
}

fn status_of(status: &str, exit_code: Option<i64>) -> ActionStatus {
    match status {
        "inProgress" => ActionStatus::Running,
        "declined" => ActionStatus::Declined,
        "failed" => ActionStatus::Failed(exit_code),
        _ if exit_code.is_some_and(|c| c != 0) => ActionStatus::Failed(exit_code),
        _ => ActionStatus::Done,
    }
}

fn upsert_action(c: &mut ChatState, action: ActionItem) {
    let found = c
        .items
        .iter()
        .position(|i| matches!(i, ChatItem::Action(a) if a.id == action.id));
    match found {
        Some(ix) => c.items[ix] = ChatItem::Action(action),
        None => c.items.push(ChatItem::Action(action)),
    }
}

fn apply_item(c: &mut ChatState, item: Item, root: &std::path::Path) {
    match item {
        Item::AgentMessage { id, text } => {
            let found = c
                .items
                .iter()
                .position(|i| matches!(i, ChatItem::Agent { id: have, .. } if *have == id));
            if !text.is_empty() {
                match found {
                    Some(ix) => c.items[ix] = ChatItem::Agent { id, text },
                    None => c.items.push(ChatItem::Agent { id, text }),
                }
            }
        }
        Item::Command {
            id,
            command,
            status,
            exit_code,
            output,
        } => upsert_action(
            c,
            ActionItem {
                id,
                kind: ActionKind::Command,
                title: crate::agents::chat::display_command(&command),
                detail: command,
                output,
                status: status_of(&status, exit_code),
                paths: vec![],
                exit_code,
            },
        ),
        Item::FileChange {
            id,
            status,
            changes,
        } => {
            let paths: Vec<String> = changes
                .iter()
                .map(|ch| snapshot::relative_agent_path(&ch.path, root))
                .collect();
            c.observations.agent_paths.extend(paths.iter().cloned());
            upsert_action(
                c,
                ActionItem {
                    id,
                    kind: ActionKind::Edit,
                    title: match paths.as_slice() {
                        [one] => one.clone(),
                        many => format!("{} files", many.len()),
                    },
                    detail: paths.join("\n"),
                    output: String::new(),
                    status: status_of(&status, None),
                    paths,
                    exit_code: None,
                },
            );
        }
        Item::Tool { id, name, status } => upsert_action(
            c,
            ActionItem {
                id,
                kind: ActionKind::Tool,
                title: name,
                detail: String::new(),
                output: String::new(),
                status: status_of(&status, None),
                paths: vec![],
                exit_code: None,
            },
        ),
        _ => {}
    }
}

impl KernApp {
    fn codex(&mut self) -> Option<&mut CodexState> {
        self.project.as_mut().map(|p| &mut p.codex)
    }

    /// Starts the App Server only on demand, when the panel is actually used.
    pub fn codex_connect(&mut self, cx: &mut Context<Self>) {
        if !self.settings.extension_enabled("codex") {
            return;
        }
        let Some(p) = &mut self.project else { return };
        if p.codex.client.is_some() || matches!(p.codex.conn, Conn::Starting) {
            return;
        }
        match cli_status() {
            ToolStatus::Found(_) => {}
            ToolStatus::Broken(path) => {
                p.codex.conn = Conn::CliMissing {
                    hint: format!(
                        "{} is a broken link. Reinstall: {}",
                        path.display(),
                        tools::install_hint("codex")
                    ),
                };
                cx.notify();
                return;
            }
            ToolStatus::Missing => {
                p.codex.conn = Conn::CliMissing {
                    hint: format!("Install: {}", tools::install_hint("codex")),
                };
                cx.notify();
                return;
            }
        }
        p.codex.conn = Conn::Starting;
        let root = p.root.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let (ready_tx, ready_rx) = futures::channel::oneshot::channel();
        std::thread::Builder::new()
            .name("codex-start".into())
            .spawn(move || {
                let _ = ready_tx.send(CodexClient::start(&root, tx));
            })
            .ok();
        let project_root = p.root.clone();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let started = ready_rx.await;
            let client = match started {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    this.update(cx, |this, cx| {
                        if let Some(c) = this.codex() {
                            c.conn = Conn::Error(format!("{e:#}"));
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            let account = client
                .call("account/read", json!({"refreshToken": false}))
                .await;
            this.update(cx, |this, cx| {
                let Some(c) = this.codex() else { return };
                c.client = Some(client);
                c.conn = match account {
                    Ok(res) => match protocol::parse_account(&res) {
                        AccountState::Connected { description } => Conn::Connected { description },
                        AccountState::LoginRequired => Conn::LoginRequired,
                    },
                    Err(e) => Conn::Error(e.message),
                };
                if matches!(c.conn, Conn::Connected { .. }) {
                    this.codex_refresh_models(cx);
                    this.codex_refresh_limits(cx);
                    this.codex_refresh_history(cx);
                    this.codex_restore_last(cx);
                }
                cx.notify();
            })
            .ok();
            while let Some(ev) = rx.next().await {
                let ok = this.update(cx, |this, cx| {
                    if this
                        .project
                        .as_ref()
                        .is_some_and(|p| p.root == project_root)
                    {
                        this.on_codex_event(ev, cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub fn codex_login(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.codex() else { return };
        let Some(client) = c.client.clone() else {
            return;
        };
        let call = client.call("account/login/start", json!({"type": "chatgpt"}));
        cx.spawn(async move |this, cx| {
            let res = call.await;
            this.update(cx, |this, cx| {
                let Some(c) = this.codex() else { return };
                match res {
                    Ok(v) => {
                        let url = v
                            .get("authUrl")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        c.conn = Conn::WaitingForLogin { url: url.clone() };
                        if !url.is_empty() {
                            cx.open_url(&url);
                        }
                    }
                    Err(e) => c.conn = Conn::Error(e.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn codex_refresh_account(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.codex() else { return };
        let Some(client) = c.client.clone() else {
            return;
        };
        let call = client.call("account/read", json!({"refreshToken": false}));
        cx.spawn(async move |this, cx| {
            let res = call.await;
            this.update(cx, |this, cx| {
                if let Some(c) = this.codex()
                    && let Ok(v) = res
                {
                    c.conn = match protocol::parse_account(&v) {
                        AccountState::Connected { description } => Conn::Connected { description },
                        AccountState::LoginRequired => Conn::LoginRequired,
                    };
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_codex_event(&mut self, ev: CodexEvent, cx: &mut Context<Self>) {
        match ev {
            CodexEvent::Closed(reason) => {
                if let Some(c) = self.codex() {
                    c.client = None;
                    c.session_id = None;
                    c.turn_id = None;
                    c.running = false;
                    c.stopping = false;
                    c.pending.clear();
                    c.conn = Conn::Error(reason);
                }
            }
            CodexEvent::Request { id, request } => {
                let Some(c) = self.codex() else { return };
                if let ServerRequest::Unsupported(method) = &request {
                    // Answer so that the server never waits on something KERN cannot do.
                    let msg = format!("{method} is not supported by this client");
                    if let Some(client) = &c.client {
                        client.respond_error(id, -32601, &msg);
                    }
                } else {
                    c.pending.push(PendingRequest {
                        id,
                        request: PendingKind::Codex(request),
                        answers: BTreeMap::new(),
                    });
                }
            }
            CodexEvent::Notification(n) => self.on_codex_notification(n, cx),
        }
        if let Some(c) = self.codex() {
            c.sync_rows(cx);
        }
        cx.notify();
    }

    fn on_codex_notification(&mut self, n: Notification, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        let Some(c) = self.codex() else { return };
        match n {
            Notification::TurnStarted { turn_id } => {
                c.turn_id = Some(turn_id);
                c.running = true;
            }
            Notification::RateLimitsUpdated(snap) => {
                let now = sessions::now();
                match &mut c.usage.limits {
                    Some(l) => usage::merge_codex_update(l, &snap, now),
                    None => self.codex_refresh_limits(cx),
                }
            }
            Notification::TokenUsage(params) => {
                c.usage.tokens = usage::parse_codex_tokens(&params);
            }
            Notification::ModelRerouted { from, to } => {
                c.items.push(ChatItem::Note(format!(
                    "Codex moved this turn from {from} to {to}."
                )));
            }
            Notification::AgentDelta { item_id, delta } => match c
                .items
                .iter_mut()
                .rev()
                .find(|i| matches!(i, ChatItem::Agent { id, .. } if *id == item_id))
            {
                Some(ChatItem::Agent { text, .. }) => {
                    text.push_str(&delta);
                    c.retry = None;
                }
                _ => {
                    c.retry = None;
                    c.items.push(ChatItem::Agent {
                        id: item_id,
                        text: delta,
                    })
                }
            },
            Notification::ItemStarted(item) | Notification::ItemCompleted(item) => {
                c.retry = None;
                apply_item(c, item, &root)
            }
            Notification::TurnCompleted { status, error, .. } => {
                c.running = false;
                c.stopping = false;
                c.turn_id = None;
                c.retry = None;
                c.pending.clear();
                match status {
                    TurnStatus::Failed => {
                        let mut message = error.unwrap_or_else(|| "The turn failed".into());
                        // When a limit is what stopped it, say when it comes back.
                        if let Some(at) = c.usage.limits.as_ref().and_then(|l| l.reached_reset()) {
                            message.push_str(&format!(
                                " Resets at {}.",
                                usage::fmt_clock(at, sessions::now())
                            ));
                        }
                        c.items.push(ChatItem::Error(message));
                        c.chat.restore_failed();
                    }
                    TurnStatus::Interrupted => c.items.push(ChatItem::Note("Stopped".into())),
                    _ => {}
                }
                self.agent_finish_turn(AgentTab::Codex, cx);
                self.codex_refresh_limits(cx);
                if let Some(id) = self
                    .project
                    .as_ref()
                    .and_then(|p| p.codex.session_id.clone())
                {
                    self.sessions
                        .upsert("codex", &root, &id, None, Some(sessions::now()));
                    self.sessions.save();
                }
                self.codex_refresh_history(cx);
            }
            Notification::Error {
                message,
                will_retry,
            } => {
                if will_retry {
                    c.retry = Some(super::agent_chat::RetryState {
                        text: format!("Reconnecting: {message}"),
                        until: None,
                    });
                } else {
                    c.items.push(ChatItem::Error(message));
                }
            }
            Notification::Warning(w) => c.items.push(ChatItem::Note(w)),
            Notification::LoginCompleted { success, error, .. } => {
                if success {
                    self.codex_refresh_account(cx);
                } else if let Some(c) = self.codex() {
                    c.conn = Conn::Error(error.unwrap_or_else(|| "Sign-in failed".into()));
                }
            }
            Notification::AccountUpdated { .. } => self.codex_refresh_account(cx),
            _ => {}
        }
    }

    /// Start or continue a thread, then a turn. The feed follows notifications.
    pub fn codex_start_turn(
        &mut self,
        message: String,
        title: String,
        images: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        let saved = self.settings.codex_chat.clone();
        let Some(c) = self.codex() else { return };
        let Some(client) = c.client.clone() else {
            c.items
                .push(ChatItem::Error("Codex is not connected".into()));
            c.sync_rows(cx);
            return;
        };
        c.running = true;
        let thread = c.session_id.clone();
        let is_new = thread.is_none();
        let choice = usage::effective(&c.chat.models, &saved.model, &saved.effort);
        if let Some(gone) = &choice.unavailable {
            c.items.push(ChatItem::Note(format!(
                "{gone} is not offered by this connection: using the default model."
            )));
        }
        let index_root = root.clone();
        cx.spawn(async move |this, cx| {
            let cwd = root.to_string_lossy().to_string();
            let thread_id = match thread {
                Some(t) => Ok(t),
                None => client
                    .call("thread/start", protocol::thread_start_params(&cwd))
                    .await
                    .map(|v| {
                        v.get("thread")
                            .and_then(|t| t.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    }),
            };
            let turn = match &thread_id {
                Ok(t) => client
                    .call(
                        "turn/start",
                        protocol::turn_start_params(
                            t,
                            &message,
                            &images,
                            &cwd,
                            choice.model.as_deref(),
                            choice.effort.as_deref(),
                        ),
                    )
                    .await
                    .map(|v| {
                        v.get("turn")
                            .and_then(|t| t.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    }),
                Err(e) => Err(e.clone()),
            };
            this.update(cx, |this, cx| {
                if let Ok(t) = thread_id {
                    if let Some(c) = this.codex() {
                        c.session_id = Some(t.clone());
                    }
                    // The first message names a chat; later ones must not rename it.
                    let title = sessions::title_from_message(&title);
                    this.sessions.upsert(
                        "codex",
                        &index_root,
                        &t,
                        is_new.then_some(title.as_str()),
                        Some(sessions::now()),
                    );
                    this.sessions.set_last_open("codex", &index_root, &t);
                    this.sessions.save();
                }
                let Some(c) = this.codex() else { return };
                match turn {
                    Ok(id) => {
                        if !id.is_empty() {
                            c.turn_id = Some(id);
                        }
                    }
                    Err(e) => {
                        c.running = false;
                        c.items.push(ChatItem::Error(e.message));
                        c.chat.restore_failed();
                    }
                }
                c.sync_rows(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What this sign-in can select, from the official `model/list`.
    pub fn codex_refresh_models(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.codex().and_then(|c| c.client.clone()) else {
            return;
        };
        let call = client.call("model/list", json!({"limit": 100}));
        cx.spawn(async move |this, cx| {
            let res = call.await;
            this.update(cx, |this, cx| {
                if let Some(c) = this.codex() {
                    match res {
                        Ok(v) => c.models = usage::parse_codex_models(&v),
                        Err(e) => log::warn!("model/list: {}", e.message),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Limits from the official `account/rateLimits/read`.
    pub fn codex_refresh_limits(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.codex() else { return };
        let Some(client) = c.client.clone() else {
            return;
        };
        c.usage.loading = true;
        let call = client.call("account/rateLimits/read", Value::Null);
        cx.spawn(async move |this, cx| {
            let res = call.await;
            this.update(cx, |this, cx| {
                if let Some(c) = this.codex() {
                    c.usage.loading = false;
                    match res {
                        Ok(v) => {
                            c.usage.limits = Some(usage::parse_codex_limits(&v, sessions::now()));
                            c.usage.limits_error = None;
                        }
                        Err(e) => c.usage.limits_error = Some(e.message),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn codex_stop(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.codex() else { return };
        let (Some(client), Some(thread), Some(turn)) =
            (c.client.clone(), c.session_id.clone(), c.turn_id.clone())
        else {
            return;
        };
        c.stopping = true;
        c.sync_rows(cx);
        let call = client.call(
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
        );
        cx.spawn(async move |this, cx| {
            if let Err(e) = call.await {
                this.update(cx, |this, cx| {
                    if let Some(c) = this.codex() {
                        c.stopping = false;
                        c.items.push(ChatItem::Error(e.message));
                        c.sync_rows(cx);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    pub fn codex_answer_request(&mut self, id: Value, request: &ServerRequest, d: Decision) {
        if let Some(client) = self.codex().and_then(|c| c.client.clone()) {
            client.respond(id, protocol::decision_response(request, d));
        }
    }

    pub fn codex_answer_questions(
        &mut self,
        id: Value,
        request: &ServerRequest,
        answers: &BTreeMap<String, Vec<String>>,
    ) {
        let ServerRequest::UserInput { questions, .. } = request else {
            return;
        };
        let pairs: Vec<(String, String)> = questions
            .iter()
            .filter_map(|q| {
                let a = answers.get(&q.question)?.first()?;
                Some((q.id.clone(), a.clone()))
            })
            .collect();
        if let Some(client) = self.codex().and_then(|c| c.client.clone()) {
            client.respond(id, protocol::user_input_response(&pairs));
        }
    }

    /// Connection states of Codex. `None` once it is connected.
    pub fn render_codex_setup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = pal(cx);
        let conn = self.project.as_ref()?.codex.conn.clone();
        let base = |title: &'static str, detail: String| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .p_3()
                .child(empty_state(AgentTab::Codex.icon(), title, detail, p))
        };
        Some(match conn {
            Conn::Connected { .. } => return None,
            Conn::NotStarted => base("Codex", "Runs through the official CLI.".into())
                .child(
                    primary_button("cx-connect", "Connect Codex", p)
                        .on_click(cx.listener(|t, _, _, cx| t.codex_connect(cx))),
                )
                .into_any_element(),
            Conn::CliMissing { hint } => base("Codex CLI not found", String::new())
                .child(code_text(hint, p))
                .child(
                    button("cx-retry", "Check Again", p).on_click(cx.listener(|t, _, _, cx| {
                        if let Some(c) = t.codex() {
                            c.conn = Conn::NotStarted;
                        }
                        t.codex_connect(cx);
                    })),
                )
                .into_any_element(),
            Conn::Starting => base("Connecting to Codex…", String::new()).into_any_element(),
            Conn::LoginRequired => base(
                "Sign in to Codex",
                "Opens the official ChatGPT sign-in in your browser.".into(),
            )
            .child(
                primary_button("cx-login", "Sign In with ChatGPT", p)
                    .on_click(cx.listener(|t, _, _, cx| t.codex_login(cx))),
            )
            .into_any_element(),
            Conn::WaitingForLogin { url } => {
                base("Waiting for sign-in in the browser…", String::new())
                    .child(
                        button("cx-open-url", "Open Sign-In Page Again", p)
                            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                    )
                    .into_any_element()
            }
            Conn::Error(msg) => base("Codex is not available", msg)
                .child(
                    primary_button("cx-reconnect", "Reconnect", p).on_click(cx.listener(
                        |t, _, _, cx| {
                            if let Some(c) = t.codex() {
                                c.client = None;
                                c.conn = Conn::NotStarted;
                            }
                            t.codex_connect(cx);
                        },
                    )),
                )
                .into_any_element(),
        })
    }

    // ---- History ---------------------------------------------------------------

    pub fn codex_shutdown(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.project {
            let c = &mut p.codex;
            if let (Some(client), Some(thread), Some(turn)) =
                (&c.client, &c.chat.session_id, &c.chat.turn_id)
            {
                drop(client.call(
                    "turn/interrupt",
                    json!({"threadId": thread, "turnId": turn}),
                ));
            }
            c.client = None;
            c.chat.running = false;
            c.chat.preparing = false;
            c.chat.stopping = false;
            c.chat.turn_id = None;
            c.chat.pending.clear();
            c.conn = Conn::NotStarted;
        }
        cx.notify();
    }

    /// Merge the server's thread list for this project into the local index.
    pub fn codex_refresh_history(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        let Some(client) = self.project.as_ref().and_then(|p| p.codex.client.clone()) else {
            return;
        };
        let call = client.call(
            "thread/list",
            json!({"cwd": root.to_string_lossy(), "limit": 100}),
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(v) => {
                        for t in v
                            .get("data")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            let Some(id) = t.get("id").and_then(Value::as_str) else {
                                continue;
                            };
                            let name = t
                                .get("name")
                                .and_then(Value::as_str)
                                .filter(|n| !n.is_empty());
                            let preview = t
                                .get("preview")
                                .and_then(Value::as_str)
                                .map(protocol::display_user_text);
                            let title = name
                                .map(str::to_string)
                                .or_else(|| preview.map(|p| sessions::title_from_message(&p)));
                            let at = t.get("updatedAt").and_then(Value::as_i64);
                            this.sessions
                                .upsert("codex", &root, id, title.as_deref(), at);
                        }
                        this.sessions.save();
                    }
                    Err(e) => log::warn!("thread/list: {}", e.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reopen the chat that was open before the restart. Failure says why and
    /// keeps the record; it does not pretend the old chat continues.
    pub fn codex_restore_last(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        if let Some(id) = self.sessions.get_last_open("codex", &root).cloned() {
            self.codex_open_session(id, cx);
        }
    }

    pub fn codex_new_chat(&mut self, cx: &mut Context<Self>) {
        let root = self.project.as_ref().map(|p| p.root.clone());
        let Some(c) = self.codex() else { return };
        if c.busy() {
            self.toast("Stop the running task first", true, cx);
            return;
        }
        c.session_id = None;
        c.show_history = false;
        c.chat.clear_feed(cx);
        if let Some(root) = root {
            self.sessions.clear_last_open("codex", &root);
            self.sessions.save();
        }
        cx.notify();
    }

    /// Resume through the documented `thread/resume`, then show its real turns.
    pub fn codex_open_session(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        let Some(c) = self.codex() else { return };
        if c.busy() {
            self.toast("Stop the running task first", true, cx);
            return;
        }
        let Some(client) = c.client.clone() else {
            self.toast("Connect Codex first", true, cx);
            return;
        };
        c.session_error = None;
        c.opening = true;
        c.pending_open = Some(id.clone());
        let call = client.call(
            "thread/resume",
            json!({"threadId": id, "cwd": root.to_string_lossy()}),
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                let Some(root) = this.project.as_ref().map(|p| p.root.clone()) else {
                    return;
                };
                let Some(c) = this.codex() else { return };
                if c.pending_open.as_deref() != Some(id.as_str()) {
                    return;
                }
                c.pending_open = None;
                c.opening = false;
                match result {
                    Ok(v) => {
                        let thread = v.get("thread").cloned().unwrap_or(Value::Null);
                        c.session_id = Some(id.clone());
                        c.items.clear();
                        c.review = None;
                        c.show_history = false;
                        c.expanded.clear();
                        c.usage.tokens = None;
                        for item in protocol::thread_items(&thread) {
                            apply_history_item(&mut c.chat, item, &root);
                        }
                        c.chat.reload_rows(cx);
                        this.sessions.upsert("codex", &root, &id, None, None);
                        this.sessions.set_last_open("codex", &root, &id);
                        this.sessions.save();
                    }
                    // Show why and keep whatever was open. A new chat is the user's choice.
                    Err(e) => {
                        c.session_error = Some(SessionError {
                            id: Some(id.clone()),
                            message: e.message,
                        })
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl KernApp {
    pub fn agent_ready(&self, _agent: AgentTab) -> bool {
        self.project
            .as_ref()
            .is_some_and(|p| matches!(p.codex.conn, Conn::Connected { .. }))
    }

    pub fn agent_not_ready_reason(&self, _agent: AgentTab) -> &'static str {
        "Connect Codex first"
    }

    pub fn agent_connect(&mut self, _agent: AgentTab, cx: &mut Context<Self>) {
        self.codex_connect(cx)
    }

    pub fn render_agent_setup(
        &mut self,
        _agent: AgentTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.render_codex_setup(cx)
    }
}
