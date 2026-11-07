//! The one Agent Chat: feed, states, permission cards, review and composer. It
//! knows nothing about how a reply is produced: Codex and Claude plug in below it.
use super::agents_panel::AgentTab;
use super::app::*;
use super::assets::icon;
use super::theme::{Palette, pal};
use super::widgets::*;
use crate::agents::attachments::{self, Attachment, is_secret_file};
use crate::agents::chat::{self, ActionItem, ActionStatus, ChatPhase};
use crate::agents::protocol::{Decision, ServerRequest};
use crate::agents::snapshot::{
    self, Attribution, RevertOutcome, ReviewedFile, SnapshotStore, TurnObservations,
};
use crate::agents::usage;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub use crate::agents::chat::ChatItem;

pub enum PendingKind {
    Codex(ServerRequest),
}

pub struct PendingRequest {
    pub id: Value,
    pub request: PendingKind,
    /// Answers chosen so far, for a card with several questions.
    pub answers: BTreeMap<String, Vec<String>>,
}

pub struct TurnReview {
    pub before: String,
    pub after: String,
    pub files: Vec<ReviewedFile>,
    pub reverted: Vec<String>,
}

/// A request is being retried. `until` is when, if the source says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryState {
    pub text: String,
    pub until: Option<std::time::Instant>,
}

/// What the Usage popover shows. Empty fields mean the source did not give them.
#[derive(Debug, Clone, Default)]
pub struct UsageState {
    pub loading: bool,
    pub limits: Option<usage::CodexLimits>,
    pub limits_error: Option<String>,
    pub tokens: Option<usage::TokenUsage>,
}

/// A session that could not be opened. The record stays and can be retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionError {
    pub id: Option<String>,
    pub message: String,
}

/// A file dragged from the Explorer.
#[derive(Clone)]
pub struct DraggedFile(pub std::path::PathBuf);

impl DraggedFile {
    pub fn name(&self) -> String {
        self.0
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

pub struct DragPreview(pub String);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(RADIUS))
            .bg(p.surface)
            .border_1()
            .border_color(p.accent)
            .text_size(px(11.5))
            .text_color(p.fg)
            .child(self.0.clone())
    }
}

pub struct ChatState {
    pub agent: AgentTab,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub running: bool,
    pub preparing: bool,
    pub stopping: bool,
    pub opening: bool,
    pub clear_input: bool,
    pub items: Vec<ChatItem>,
    pub attachments: Vec<Attachment>,
    pub input: Entity<TextareaState>,
    pub pending: Vec<PendingRequest>,
    pub review: Option<TurnReview>,
    pub before_tree: Option<String>,
    pub store: Option<SnapshotStore>,
    pub observations: TurnObservations,
    pub scroller: Entity<MessageScrollerState>,
    pub show_history: bool,
    pub session_error: Option<SessionError>,
    pub expanded: HashSet<String>,
    pub models: Vec<usage::ModelOption>,
    /// Why the last file could not be attached. Shown before anything is sent.
    pub attach_notice: Option<String>,
    /// The session being opened. An answer for anything else is out of date: the
    /// user pressed New Chat or picked another chat in the meantime.
    pub pending_open: Option<String>,
    /// What was just sent, to put back into the composer if the turn fails.
    pub last_sent: Option<(String, Vec<Attachment>)>,
    pub restore_input: Option<String>,
    pub usage: UsageState,
    pub show_usage: bool,
    pub retry: Option<RetryState>,
    pub turn_started: Option<std::time::Instant>,
    pub ticking: bool,
    pub _sub: Subscription,
}

impl ChatState {
    pub fn new(agent: AgentTab, window: &mut Window, cx: &mut Context<KernApp>) -> Self {
        let placeholder = format!(
            "Ask {} · {} to send",
            agent.short_name(),
            super::keys::SEND.label()
        );
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder(placeholder)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this: &mut KernApp, _, ev: &InputEvent, _w, cx| {
                if matches!(
                    ev,
                    InputEvent::PressEnter {
                        secondary: true,
                        ..
                    }
                ) {
                    this.agent_send(agent, cx);
                }
            },
        );
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        Self {
            agent,
            session_id: None,
            turn_id: None,
            running: false,
            preparing: false,
            stopping: false,
            opening: false,
            clear_input: false,
            items: vec![],
            attachments: vec![],
            input,
            pending: vec![],
            review: None,
            before_tree: None,
            store: None,
            observations: TurnObservations::default(),
            scroller,
            show_history: false,
            session_error: None,
            expanded: HashSet::new(),
            models: vec![],
            attach_notice: None,
            pending_open: None,
            last_sent: None,
            restore_input: None,
            usage: UsageState::default(),
            show_usage: false,
            retry: None,
            turn_started: None,
            ticking: false,
            _sub: sub,
        }
    }

    /// A failed turn gives its text and attachments back, ready to try again.
    pub fn restore_failed(&mut self) {
        if let Some((text, atts)) = self.last_sent.take() {
            self.restore_input = Some(text);
            self.attachments.splice(0..0, atts);
        }
    }

    pub fn busy(&self) -> bool {
        self.running || self.preparing || self.stopping
    }

    /// The feed is its items plus one footer row (cards and status).
    fn row_count(&self) -> usize {
        self.items.len() + 1
    }

    /// Tell the virtual list about new rows, and that the tail may have grown.
    pub fn sync_rows(&self, cx: &mut App) {
        let n = self.row_count();
        self.scroller.update(cx, |s, cx| {
            let have = s.item_count();
            if n > have {
                s.append(n - have, cx);
            } else if n < have {
                s.reset(n, cx);
            }
            s.remeasure_items(n.saturating_sub(2)..n, cx);
        });
    }

    /// The feed was replaced as a whole, or the user asked for the latest message.
    pub fn reload_rows(&self, cx: &mut App) {
        let n = self.row_count();
        self.scroller.update(cx, |s, cx| {
            s.reset(n, cx);
            s.scroll_to_end(cx);
        });
    }

    pub fn clear_feed(&mut self, cx: &mut App) {
        self.items.clear();
        self.review = None;
        self.pending.clear();
        self.expanded.clear();
        self.session_error = None;
        self.retry = None;
        self.pending_open = None;
        self.opening = false;
        self.usage.tokens = None;
        self.reload_rows(cx);
    }

    pub fn phase(&self, ready: bool, connecting: bool) -> ChatPhase {
        chat::phase(&chat::PhaseInput {
            ready,
            connecting: connecting || self.opening,
            item_count: self.items.len(),
            running: self.running || self.preparing,
            stopping: self.stopping,
            pending: self.pending.len(),
            last_is_error: matches!(self.items.last(), Some(ChatItem::Error(_))),
        })
    }
}

impl KernApp {
    pub fn chat(&self, agent: AgentTab) -> Option<&ChatState> {
        let p = self.project.as_ref()?;
        Some(match agent {
            AgentTab::Codex => &p.codex.chat,
        })
    }

    pub fn chat_mut(&mut self, agent: AgentTab) -> Option<&mut ChatState> {
        let p = self.project.as_mut()?;
        Some(match agent {
            AgentTab::Codex => &mut p.codex.chat,
        })
    }

    fn dirty_paths(&self) -> Vec<String> {
        let Some(p) = &self.project else {
            return vec![];
        };
        p.tabs
            .iter()
            .filter(|t| t.is_dirty())
            .filter_map(|t| t.file_path())
            .map(|path| {
                path.strip_prefix(&p.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string()
            })
            .collect()
    }

    /// Send the composer: record the project state first, so that Review Diff can
    /// show exactly what changed during this turn, then hand over to the agent.
    pub fn agent_send(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        if !self.agent_ready(agent) {
            let why = self.agent_not_ready_reason(agent);
            self.toast(why, false, cx);
            return;
        }
        let dirty = self.dirty_paths();
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        if c.busy() {
            return;
        }
        let text = c.input.read(cx).value().to_string();
        if text.trim().is_empty() && c.attachments.is_empty() {
            return;
        }
        let atts = std::mem::take(&mut c.attachments);
        c.last_sent = Some((text.clone(), atts.clone()));
        c.attach_notice = None;
        let titles: Vec<String> = atts
            .iter()
            .filter(|a| a.image_path().is_none())
            .map(|a| a.title(&root))
            .collect();
        let images: Vec<std::path::PathBuf> = atts
            .iter()
            .filter_map(|a| a.image_path().map(std::path::Path::to_path_buf))
            .collect();
        let message = attachments::compose_message(&text, &atts, &root);
        c.items.push(ChatItem::User {
            text: text.clone(),
            attachments: titles,
            images: images.clone(),
        });
        c.preparing = true;
        c.turn_started = Some(std::time::Instant::now());
        c.retry = None;
        c.review = None;
        c.clear_input = true;
        c.session_error = None;
        c.observations = TurnObservations::default();
        c.observations.dirty_paths.extend(dirty);
        c.sync_rows(cx);
        c.scroller.update(cx, |s, cx| s.scroll_to_end(cx));
        let store = c.store.clone();
        self.agent_start_ticker(agent, cx);
        cx.spawn(async move |this, cx| {
            // Off the UI thread: a large tree takes a moment.
            let r = root.clone();
            let snap = cx
                .background_spawn(async move {
                    let store = match store {
                        Some(s) => s,
                        None => SnapshotStore::open(&r)?,
                    };
                    let tree = store.snapshot()?;
                    anyhow::Ok((store, tree))
                })
                .await;
            let _ = &atts;
            let (store, before) = match snap {
                Ok((s, t)) => (Some(s), Some(t)),
                Err(e) => {
                    log::warn!("snapshot failed: {e}");
                    (None, None)
                }
            };
            this.update(cx, |this, cx| {
                let Some(c) = this.chat_mut(agent) else {
                    return;
                };
                c.preparing = false;
                c.store = store;
                if before.is_none() {
                    c.items.push(ChatItem::Note(
                        "Could not record the project state: Review Diff is unavailable for this turn."
                            .into(),
                    ));
                }
                c.before_tree = before;
                this.codex_start_turn(message, text, images, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Redraw once a second while a turn runs: the timer and retry countdown.
    fn agent_start_ticker(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        if std::mem::replace(&mut c.ticking, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let Some(c) = this.chat_mut(agent) else {
                        return false;
                    };
                    let busy = c.busy();
                    if !busy {
                        c.ticking = false;
                    }
                    cx.notify();
                    busy
                });
                if !matches!(alive, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }

    /// The saved choice for one agent: model and effort, each empty for the default.
    pub fn agent_choice(&self, agent: AgentTab) -> (String, String) {
        match agent {
            AgentTab::Codex => (
                self.settings.codex_chat.model.clone(),
                self.settings.codex_chat.effort.clone(),
            ),
        }
    }

    /// Both connections take a new model in the middle of a chat, so nothing is
    /// lost: the next message goes to the chosen model, in the same session.
    pub fn agent_set_model(&mut self, agent: AgentTab, id: String, cx: &mut Context<Self>) {
        let keep_effort = self
            .chat(agent)
            .and_then(|c| c.models.iter().find(|m| m.id == id))
            .is_some_and(|m| m.efforts.contains(&self.agent_choice(agent).1));
        match agent {
            AgentTab::Codex => {
                self.settings.codex_chat.model = id;
                if !keep_effort {
                    self.settings.codex_chat.effort.clear();
                }
            }
        }
        self.persist();
        cx.notify();
    }

    pub fn agent_set_effort(&mut self, agent: AgentTab, effort: String, cx: &mut Context<Self>) {
        match agent {
            AgentTab::Codex => self.settings.codex_chat.effort = effort,
        }
        self.persist();
        cx.notify();
    }

    pub fn agent_stop(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        match agent {
            AgentTab::Codex => self.codex_stop(cx),
        }
    }

    pub fn agent_new_chat(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        match agent {
            AgentTab::Codex => self.codex_new_chat(cx),
        }
    }

    pub fn agent_open_session(&mut self, agent: AgentTab, id: String, cx: &mut Context<Self>) {
        match agent {
            AgentTab::Codex => self.codex_open_session(id, cx),
        }
    }

    /// Compare the tree with the one recorded before the turn.
    pub fn agent_finish_turn(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        let dirty_now = self.dirty_paths();
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        c.observations.dirty_paths.extend(dirty_now);
        let (Some(store), Some(before)) = (c.store.clone(), c.before_tree.clone()) else {
            return;
        };
        let obs = c.observations.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let after = store.snapshot()?;
                    let changed = store.changed_files(&before, &after)?;
                    let files = snapshot::classify(&changed, &obs);
                    anyhow::Ok(TurnReview {
                        before,
                        after,
                        files,
                        reverted: vec![],
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(c) = this.chat_mut(agent) else {
                    return;
                };
                match result {
                    Ok(review) if review.files.is_empty() => c.review = None,
                    Ok(review) => c.review = Some(review),
                    Err(e) => c
                        .items
                        .push(ChatItem::Note(format!("Review is unavailable: {e}"))),
                }
                c.sync_rows(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn agent_note_user_save(&mut self, rel: String) {
        if let Some(c) = self.chat_mut(AgentTab::Codex)
            && c.running
        {
            c.observations.user_saved_paths.insert(rel);
        }
    }

    /// Open the whole turn as one diff tab.
    pub fn agent_open_review(
        &mut self,
        agent: AgentTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.chat(agent) else { return };
        let (Some(store), Some(review)) = (c.store.clone(), &c.review) else {
            self.toast("No agent changes to review yet", false, cx);
            return;
        };
        let (before, after) = (review.before.clone(), review.after.clone());
        cx.spawn_in(window, async move |this, cx| {
            let patch = cx
                .background_spawn(async move { store.patch(&before, &after, None) })
                .await;
            this.update_in(cx, |this, window, cx| match patch {
                Ok(patch) => this.open_diff_tab(
                    format!("{}:review", agent.id()),
                    "Review Diff".into(),
                    patch,
                    None,
                    window,
                    cx,
                ),
                Err(e) => this.toast(e.to_string(), true, cx),
            })
            .ok();
        })
        .detach();
    }

    fn agent_open_file_diff(
        &mut self,
        agent: AgentTab,
        path: String,
        note: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.chat(agent) else { return };
        let (Some(store), Some(review)) = (c.store.clone(), &c.review) else {
            return;
        };
        let (before, after) = (review.before.clone(), review.after.clone());
        let key = format!("{}:{path}", agent.id());
        let title = format!(
            "{} ({})",
            std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            agent.short_name()
        );
        cx.spawn_in(window, async move |this, cx| {
            let patch = cx
                .background_spawn(async move { store.patch(&before, &after, Some(&path)) })
                .await;
            this.update_in(cx, |this, window, cx| match patch {
                Ok(patch) => this.open_diff_tab(key, title, patch, note, window, cx),
                Err(e) => this.toast(e.to_string(), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Revert one reliably attributed file; refuses if it changed after the turn.
    pub fn agent_revert_file(&mut self, agent: AgentTab, path: String, cx: &mut Context<Self>) {
        let Some(c) = self.chat(agent) else { return };
        let (Some(store), Some(review)) = (c.store.clone(), &c.review) else {
            return;
        };
        let allowed = review
            .files
            .iter()
            .any(|f| f.path == path && f.attribution == Attribution::Agent);
        if !allowed {
            return;
        }
        let (before, after) = (review.before.clone(), review.after.clone());
        cx.spawn(async move |this, cx| {
            let p2 = path.clone();
            let res = cx
                .background_spawn(async move { store.revert_file(&before, &after, &p2) })
                .await;
            this.update(cx, |this, cx| match res {
                Ok(RevertOutcome::Reverted) => {
                    if let Some(c) = this.chat_mut(agent) {
                        if let Some(r) = &mut c.review {
                            r.reverted.push(path.clone());
                        }
                        c.sync_rows(cx);
                    }
                    this.toast(format!("Reverted {path}"), false, cx);
                }
                Ok(RevertOutcome::ChangedSince) => this.toast(
                    format!("{path} changed after the agent finished. Nothing was reverted."),
                    true,
                    cx,
                ),
                Err(e) => this.toast(e.to_string(), true, cx),
            })
            .ok();
        })
        .detach();
    }

    // ---- Context attachments ---------------------------------------------------

    /// What this agent can receive right now, from its schema and selected model.
    pub fn agent_limits(&self, agent: AgentTab) -> attachments::Limits {
        const MB: u64 = 1024 * 1024;
        let (saved, _) = self.agent_choice(agent);
        let images = self.chat(agent).is_none_or(|c| {
            c.models
                .iter()
                .find(|m| {
                    if saved.is_empty() || saved == "default" {
                        m.is_default
                    } else {
                        m.id == saved
                    }
                })
                .is_none_or(|m| m.images)
        });
        match agent {
            // `UserInput` has text, image, localImage, audio: no document.
            AgentTab::Codex => attachments::Limits {
                image_bytes: 20 * MB,
                documents: false,
                document_bytes: 0,
                images,
            },
        }
    }

    /// Attach files. A sensitive one waits for a separate confirmation.
    pub fn agent_attach_paths(
        &mut self,
        agent: AgentTab,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let mut sensitive = None;
        for path in paths {
            if is_secret_file(&path) {
                sensitive = Some(path);
            } else {
                self.agent_attach_path_confirmed(agent, path, cx);
            }
        }
        if let Some(path) = sensitive {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            self.overlay = Some(super::overlays::Overlay::confirm(
                &format!("Send {name} to {}?", agent.short_name()),
                "This file looks like it holds secrets. Its contents would go to the agent's provider.",
                vec![super::overlays::ConfirmChoice::new(
                    "Attach Anyway",
                    super::overlays::ConfirmAction::AttachSensitive { agent, path },
                    false,
                )],
            ));
        }
        cx.notify();
    }

    pub fn agent_attach_path_confirmed(
        &mut self,
        agent: AgentTab,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let limits = self.agent_limits(agent);
        let result = attachments::classify(&path, &limits);
        self.settings.layout.codex_visible = true;
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        match result {
            Ok(a) => {
                c.attach_notice = None;
                let dup = c.attachments.iter().any(|x| x.path() == a.path());
                if !dup {
                    c.attachments.push(a);
                }
            }
            Err(why) => c.attach_notice = Some(why),
        }
        cx.notify();
    }

    pub fn agent_pick_files(
        &mut self,
        agent: AgentTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |t, cx| t.agent_attach_paths(agent, paths, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Cmd+V in the composer. Text keeps going to the input; an image or a copied
    /// file becomes an attachment.
    fn agent_paste(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut image = None;
        let mut files = None;
        let mut text = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(i) => image = Some(i.clone()),
                ClipboardEntry::ExternalPaths(p) => files = Some(p.paths().to_vec()),
                ClipboardEntry::String(s) => text |= !s.text().trim().is_empty(),
            }
        }
        if let Some(paths) = files {
            cx.stop_propagation();
            self.agent_attach_paths(agent, paths, cx);
            return;
        }
        let Some(image) = image.filter(|_| !text) else {
            return;
        };
        cx.stop_propagation();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { save_pasted_image(&image) })
                .await;
            this.update(cx, |t, cx| match saved {
                Ok(path) => t.agent_attach_path_confirmed(agent, path, cx),
                Err(e) => {
                    if let Some(c) = t.chat_mut(agent) {
                        c.attach_notice = Some(format!("Could not paste this image: {e}"));
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn agent_preview(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_async(path, true, None, window, cx);
    }

    fn attach(&mut self, agent: AgentTab, a: Attachment, cx: &mut Context<Self>) {
        self.settings.layout.codex_visible = true;
        self.agent_tab = agent;
        self.agent_connect(agent, cx);
        if let Some(c) = self.chat_mut(agent) {
            c.attachments.push(a);
        }
        cx.notify();
    }

    pub fn agent_attach_active_file(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        let Some(f) = self.active_file() else {
            self.toast("Open a file first", false, cx);
            return;
        };
        let path = f.path.clone();
        if is_secret_file(&path) {
            self.agent_attach_paths(agent, vec![path], cx);
            return;
        }
        let content = f.editor.read(cx).text().to_string();
        self.attach(agent, Attachment::File { path, content }, cx);
    }

    pub fn agent_attach_selection(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        let Some(f) = self.active_file() else {
            self.toast("Open a file first", false, cx);
            return;
        };
        let path = f.path.clone();
        let e = f.editor.read(cx);
        let text = e.selected_value().to_string();
        if text.is_empty() {
            self.toast("Select some code first", false, cx);
            return;
        }
        let line = {
            use gpui_kit::component::input::RopeExt as _;
            e.text().offset_to_point(e.selected_range().start).row as u32 + 1
        };
        if is_secret_file(&path) {
            self.toast(
                format!(
                    "{} looks like a secret file. It is not attached.",
                    path.display()
                ),
                true,
                cx,
            );
            return;
        }
        self.attach(
            agent,
            Attachment::Selection {
                path,
                start_line: line,
                content: text,
            },
            cx,
        );
    }

    /// The visible text of the active terminal, with anything key-like masked.
    pub fn agent_attach_terminal(&mut self, agent: AgentTab, cx: &mut Context<Self>) {
        let Some(view) = self
            .project
            .as_ref()
            .and_then(|p| p.terminals.get(p.active_terminal).cloned())
        else {
            self.toast("Open a terminal first", false, cx);
            return;
        };
        let raw = view.read(cx).screen_text();
        if raw.trim().is_empty() {
            self.toast("The terminal is empty", false, cx);
            return;
        }
        let (content, masked) = attachments::scrub_secrets(raw.trim_end());
        if masked > 0 {
            self.toast(
                format!("{masked} value(s) that look like secrets were masked"),
                false,
                cx,
            );
        }
        self.attach(
            agent,
            Attachment::Output {
                label: "Terminal output".into(),
                content,
                file: None,
            },
            cx,
        );
    }

    // ---- Answers to permission cards -------------------------------------------

    pub fn agent_decide(
        &mut self,
        agent: AgentTab,
        index: usize,
        decision: Decision,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        if index >= c.pending.len() {
            return;
        }
        let req = c.pending.remove(index);
        c.sync_rows(cx);
        match req.request {
            PendingKind::Codex(request) => self.codex_answer_request(req.id, &request, decision),
        }
        cx.notify();
    }

    /// Choose one option of a clarifying question; send once all are answered.
    fn agent_choose_option(
        &mut self,
        agent: AgentTab,
        index: usize,
        question: String,
        option: String,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        let Some(req) = c.pending.get_mut(index) else {
            return;
        };
        let total = CardData::from_request(req).questions.len();
        let picked = req.answers.entry(question).or_default();
        if multi {
            match picked.iter().position(|o| *o == option) {
                Some(i) => {
                    picked.remove(i);
                }
                None => picked.push(option),
            }
        } else {
            *picked = vec![option];
        }
        let done = !multi && req.answers.values().filter(|a| !a.is_empty()).count() == total;
        c.sync_rows(cx);
        if done {
            self.agent_submit_answers(agent, index, cx);
        }
        cx.notify();
    }

    fn agent_submit_answers(&mut self, agent: AgentTab, index: usize, cx: &mut Context<Self>) {
        let Some(c) = self.chat_mut(agent) else {
            return;
        };
        if index >= c.pending.len() {
            return;
        }
        let req = c.pending.remove(index);
        c.sync_rows(cx);
        match req.request {
            PendingKind::Codex(request) => {
                self.codex_answer_questions(req.id, &request, &req.answers)
            }
        }
        cx.notify();
    }

    // ---- The panel body ---------------------------------------------------------

    /// Feed and composer for one agent. The composer is always in place.
    pub fn render_agent_chat(
        &mut self,
        agent: AgentTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let this = cx.entity();
        if let Some(c) = self.chat_mut(agent)
            && std::mem::take(&mut c.clear_input)
        {
            let input = c.input.clone();
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        if let Some(c) = self.chat_mut(agent)
            && let Some(text) = c.restore_input.take()
        {
            let input = c.input.clone();
            if input.read(cx).value().is_empty() {
                input.update(cx, |i, cx| i.set_value(text, window, cx));
            }
        }
        let setup = self.render_agent_setup(agent, window, cx);
        let Some(c) = self.chat(agent) else {
            return div().into_any_element();
        };
        let ready = setup.is_none();
        let show_history = c.show_history;
        let empty = c.items.is_empty() && c.pending.is_empty() && !c.opening;
        let opening = c.opening;
        let session_error = c.session_error.clone();
        let scroller = c.scroller.clone();
        let surface = p.panel;
        let body: AnyElement = if show_history {
            self.render_chat_history(agent, cx)
        } else if let Some(setup) = setup {
            div()
                .id("agent-setup")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(setup)
                .into_any_element()
        } else if opening && empty {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(p.muted)
                .child(Spinner::new().small())
                .child("Opening chat…")
                .into_any_element()
        } else if empty {
            let name = self
                .project
                .as_ref()
                .map(|x| x.name.clone())
                .unwrap_or_default();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_state(
                    agent.icon(),
                    format!("Chat with {} in {name}", agent.short_name()),
                    format!(
                        "Attach a file or selection with +, then press {}.",
                        super::keys::SEND.label()
                    ),
                    p,
                ))
                .into_any_element()
        } else {
            let feed_app = this.clone();
            MessageScroller::new(
                SharedString::from(format!("feed-{}", agent.id())),
                scroller,
                move |ix, window, cx| render_row(&feed_app, agent, ix, window, cx),
            )
            .with_bottom_fade(surface)
            .with_row_style(StyleRefinement::default().pb(px(4.)))
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let composer = self.render_composer(agent, ready, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(p.accent_soft))
            .drag_over::<DraggedFile>(move |s, _, _, _| s.bg(p.accent_soft))
            .on_drop(cx.listener(move |t, paths: &ExternalPaths, _, cx| {
                t.agent_attach_paths(agent, paths.paths().to_vec(), cx)
            }))
            .on_drop(cx.listener(move |t, file: &DraggedFile, _, cx| {
                t.agent_attach_paths(agent, vec![file.0.clone()], cx)
            }))
            .when_some(session_error, |d, e| {
                d.child(self.render_session_error(agent, e, cx))
            })
            .child(body)
            // Nothing can be sent yet, so the setup cards stand alone.
            .when(ready, |d| d.child(composer))
            .into_any_element()
    }

    fn render_session_error(
        &self,
        agent: AgentTab,
        e: SessionError,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let retry = e.id.clone();
        let has_id = e.id.is_some();
        card(p)
            .m_2()
            .flex_shrink_0()
            .border_color(p.danger)
            .child(div().text_size(px(12.)).child(if has_id {
                format!("Could not open this chat: {}", e.message)
            } else {
                e.message.clone()
            }))
            .when(has_id, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .child("The chat stays in History. No new chat was started."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .when_some(retry, |d, id| {
                        d.child(
                            primary_button("sess-retry", "Try Again", p).on_click(cx.listener(
                                move |t, _, _, cx| t.agent_open_session(agent, id.clone(), cx),
                            )),
                        )
                    })
                    .child(button("sess-dismiss", "Dismiss", p).on_click(cx.listener(
                        move |t, _, _, cx| {
                            if let Some(c) = t.chat_mut(agent) {
                                c.session_error = None;
                            }
                            cx.notify();
                        },
                    ))),
            )
            .into_any_element()
    }

    /// The models the connection offers. In a narrow panel it collapses to an icon.
    fn render_model_selector(
        &self,
        agent: AgentTab,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::component::button::{Button, ButtonVariants as _};
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let p = pal(cx);
        let Some(c) = self.chat(agent) else {
            return div().into_any_element();
        };
        let (saved, _) = self.agent_choice(agent);
        let models = c.models.clone();
        let wants_default = saved.is_empty() || saved == "default";
        let current = if wants_default {
            models.iter().find(|m| m.is_default)
        } else {
            models.iter().find(|m| m.id == saved)
        }
        .cloned();
        let missing = !wants_default && !models.is_empty() && current.is_none();
        let name = match (&current, missing) {
            (_, true) => format!("{saved} (unavailable)"),
            (Some(m), _) => m.name.trim_end_matches(" (recommended)").to_string(),
            (None, _) if !wants_default => saved.clone(),
            _ => "Default".to_string(),
        };
        let tooltip = if models.is_empty() {
            "Model list is not available yet".to_string()
        } else if missing {
            format!("{saved} is not offered by this connection. The default model is used.")
        } else {
            format!("Model: {name}")
        };
        let this = cx.entity();
        let trigger = Button::new(SharedString::from(format!("model-{}", agent.id())))
            .ghost()
            .small()
            .compact()
            .h(px(ROW_H))
            .min_w_0()
            .tooltip(tooltip.clone())
            .accessibility_label(tooltip)
            .when(narrow, |b| {
                b.child(icon_el(
                    "cpu",
                    ICON_MD,
                    if missing { p.warning } else { p.muted },
                ))
            })
            .when(!narrow, |b| {
                b.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .text_size(px(11.5))
                        .text_color(if missing { p.warning } else { p.muted })
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(name.clone()),
                        )
                        .child(icon_el("chevron-down", ICON_SM - 2., p.muted)),
                )
            });
        trigger
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, _| {
                let mut m = menu.min_w(px(190.)).max_h(px(420.)).scrollable(true);
                if missing {
                    m = m.item(
                        PopupMenuItem::new(format!("{saved}: not offered here"))
                            .disabled(true)
                            .checked(true),
                    );
                }
                for opt in &models {
                    let id = opt.id.clone();
                    let on = if wants_default {
                        opt.is_default
                    } else {
                        opt.id == saved
                    };
                    m = m.item(PopupMenuItem::new(opt.name.clone()).checked(on).on_click(
                        window.listener_for(&this, move |t: &mut KernApp, _, _, cx| {
                            t.agent_set_model(agent, id.clone(), cx)
                        }),
                    ));
                }
                if models.is_empty() {
                    m = m.item(PopupMenuItem::new("Connect to list models").disabled(true));
                }
                m
            })
            .into_any_element()
    }

    /// The levels of the selected model, and which one applies now.
    fn agent_effort_levels(&self, agent: AgentTab) -> Option<(Vec<String>, usize)> {
        let c = self.chat(agent)?;
        let (saved, saved_effort) = self.agent_choice(agent);
        let model = if saved.is_empty() || saved == "default" {
            c.models.iter().find(|m| m.is_default)
        } else {
            c.models.iter().find(|m| m.id == saved)
        }?;
        // One level is no choice at all: then there is no slider.
        if model.efforts.len() < 2 {
            return None;
        }
        let shown = if model.efforts.contains(&saved_effort) {
            saved_effort
        } else {
            model.default_effort.clone().unwrap_or_default()
        };
        let ix = model.efforts.iter().position(|e| *e == shown).unwrap_or(0);
        Some((model.efforts.clone(), ix))
    }

    /// «Рассуждение»: a small popover with a discrete slider, one stop per level
    /// the selected model reports. Raw level names stay out of the interface.
    fn render_effort_control(
        &mut self,
        agent: AgentTab,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use gpui_kit::component::button::{Button, ButtonVariants as _};
        use gpui_kit::component::popover::Popover;
        use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
        let p = pal(cx);
        let (levels, ix) = self.agent_effort_levels(agent)?;
        let current = usage::effort_label(&levels[ix]);
        let stale = !self
            .effort_slider
            .as_ref()
            .is_some_and(|(a, l, _, _)| *a == agent && *l == levels);
        if stale {
            let n = levels.len();
            let state = cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max((n - 1) as f32)
                    .step(1.)
                    .default_value(ix as f32)
            });
            let stops = levels.clone();
            let sub = cx.subscribe(&state, move |t: &mut KernApp, _, ev: &SliderEvent, cx| {
                if let SliderEvent::Change(v) = ev {
                    let i = (v.end().round() as usize).min(stops.len() - 1);
                    t.agent_set_effort(agent, stops[i].clone(), cx);
                }
            });
            self.effort_slider = Some((agent, levels.clone(), state, sub));
        }
        let state = self.effort_slider.as_ref()?.2.clone();
        // The value can also change from outside: a new model keeps or resets it.
        if (state.read(cx).value().end() - ix as f32).abs() > 0.1 {
            state.update(cx, |s, cx| s.set_value(ix as f32, window, cx));
        }
        let label = if width >= 340. {
            format!("Рассуждение · {current}")
        } else if width >= 300. {
            current.to_string()
        } else {
            String::new()
        };
        let tooltip = format!("Рассуждение: {current}");
        let trigger = Button::new(SharedString::from(format!("effort-{}", agent.id())))
            .ghost()
            .small()
            .compact()
            .h(px(ROW_H))
            .flex_shrink_0()
            .tooltip(tooltip.clone())
            .accessibility_label(tooltip)
            .when(label.is_empty(), |b| {
                b.child(icon_el("brain", ICON_MD, p.muted))
            })
            .when(!label.is_empty(), |b| {
                b.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(p.muted)
                        .whitespace_nowrap()
                        .child(label.clone()),
                )
            });
        let n = levels.len();
        // Words under the track only where they sit under their own stop. With more
        // stops the ends show direction, and the exact value is at the top.
        let edge_labels: Vec<&'static str> = if n <= 3 {
            levels.iter().map(|l| usage::effort_label(l)).collect()
        } else {
            vec!["Быстрее", "Глубже"]
        };
        Some(
            Popover::new(SharedString::from(format!("effort-pop-{}", agent.id())))
                .anchor(Anchor::BottomLeft)
                .trigger(trigger)
                .content(move |_, _, _| {
                    div()
                        .w(px(230.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Рассуждение"),
                                )
                                .child(
                                    div().text_size(px(12.)).text_color(p.accent).child(current),
                                ),
                        )
                        .child(Slider::new(&state).accessibility_label("Рассуждение"))
                        .child(div().flex().justify_between().px(px(5.)).children(
                            (0..n).map(|_| div().size(px(3.)).rounded_full().bg(p.muted)),
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_size(px(10.5))
                                .text_color(p.muted)
                                .children(edge_labels.iter().map(|l| div().child(*l))),
                        )
                })
                .into_any_element(),
        )
    }

    fn render_attachment_chip(
        &self,
        agent: AgentTab,
        i: usize,
        a: &Attachment,
        root: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let remove = icon_button(("att-x", i), "x", "Remove attachment")
            .size(px(16.))
            .on_click(cx.listener(move |t, _, _, cx| {
                cx.stop_propagation();
                if let Some(c) = t.chat_mut(agent)
                    && i < c.attachments.len()
                {
                    c.attachments.remove(i);
                }
                cx.notify();
            }));
        let preview = a.path().map(std::path::Path::to_path_buf);
        let title = root.map(|r| a.title(r)).unwrap_or_default();
        if let Some(path) = a.image_path() {
            let open = path.to_path_buf();
            return div()
                .id(("att-thumb", i))
                .relative()
                .size(px(48.))
                .rounded(px(RADIUS))
                .overflow_hidden()
                .border_1()
                .border_color(p.border)
                .bg(p.bg)
                .cursor_pointer()
                .tooltip({
                    let title = title.clone();
                    move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(title.clone()).build(w, cx)
                    }
                })
                .on_click(cx.listener(move |t, _, w, cx| t.agent_preview(open.clone(), w, cx)))
                .child(
                    img(path.to_path_buf())
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .right(px(1.))
                        .rounded(px(RADIUS))
                        .bg(p.surface)
                        .child(remove),
                )
                .into_any_element();
        }
        let kind = match a {
            Attachment::Selection { .. } => "text-cursor-input",
            Attachment::Output { .. } => "terminal",
            Attachment::Document { .. } => "file-text",
            _ => "file",
        };
        div()
            .id(("att-chip", i))
            .flex()
            .items_center()
            .gap_1()
            .h(px(22.))
            .pl_1()
            .min_w_0()
            .rounded(px(RADIUS))
            .bg(p.accent_soft)
            .text_size(px(11.))
            .when(preview.is_some(), |d| d.cursor_pointer())
            .on_click(cx.listener(move |t, _, w, cx| {
                if let Some(path) = preview.clone() {
                    t.agent_preview(path, w, cx);
                }
            }))
            .child(icon_el(kind, ICON_SM, p.accent))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .max_w(px(170.))
                    .child(title),
            )
            .child(remove)
            .into_any_element()
    }

    fn render_composer(
        &mut self,
        agent: AgentTab,
        ready: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::component::button::{Button, ButtonVariants as _};
        use gpui_kit::component::input::Paste;
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let p = pal(cx);
        let width = self.settings.layout.codex_width;
        let narrow = width < 300.;
        let selector = self.render_model_selector(agent, narrow, cx);
        let effort = self.render_effort_control(agent, width, window, cx);
        let limits = self.agent_limits(agent);
        let Some(c) = self.chat(agent) else {
            return div().into_any_element();
        };
        let root = self.project.as_ref().map(|x| x.root.clone());
        let busy = c.busy();
        let stopping = c.stopping;
        let input = c.input.clone();
        let notice = c.attach_notice.clone();
        let has_content = !input.read(cx).value().trim().is_empty() || !c.attachments.is_empty();
        let atts = c.attachments.clone();
        let chips: Vec<AnyElement> = atts
            .iter()
            .enumerate()
            .map(|(i, a)| self.render_attachment_chip(agent, i, a, root.as_deref(), cx))
            .collect();
        let this = cx.entity();
        let hint = limits.hint();
        let add = icon_button("composer-add", "plus", "Attach files or context")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, _cx| {
                menu.item(
                    PopupMenuItem::new("Attach Files…")
                        .icon(icon("paperclip"))
                        .on_click(window.listener_for(&this, move |t, _, w, cx| {
                            t.agent_pick_files(agent, w, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("Current File")
                        .icon(icon("file"))
                        .on_click(window.listener_for(&this, move |t, _, _, cx| {
                            t.agent_attach_active_file(agent, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("Selection")
                        .icon(icon("text-cursor-input"))
                        .on_click(window.listener_for(&this, move |t, _, _, cx| {
                            t.agent_attach_selection(agent, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("Terminal Output")
                        .icon(icon("terminal"))
                        .on_click(window.listener_for(&this, move |t, _, _, cx| {
                            t.agent_attach_terminal(agent, cx)
                        })),
                )
                .separator()
                .label(hint.clone())
            });
        let can_send = ready && has_content;
        let action = if busy {
            Button::new("composer-stop")
                .primary()
                .icon(icon("square").with_size(px(ICON_SM)))
                .tooltip(if stopping { "Stopping…" } else { "Stop" })
                .accessibility_label("Stop")
                .size(px(26.))
                .rounded(px(13.))
                .flex_shrink_0()
                .on_click(cx.listener(move |t, _, _, cx| t.agent_stop(agent, cx)))
        } else {
            Button::new("composer-send")
                .primary()
                .icon(icon("arrow-up").with_size(px(ICON_MD)))
                .tooltip(format!("Send ({})", super::keys::SEND.label()))
                .accessibility_label("Send")
                .size(px(26.))
                .rounded(px(13.))
                .flex_shrink_0()
                .disabled(!can_send)
                .on_click(cx.listener(move |t, _, _, cx| t.agent_send(agent, cx)))
        };
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .mx_2()
            .mb_2()
            .mt_1()
            .p_1()
            .rounded(px(10.))
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .capture_action(cx.listener(move |t, _: &Paste, _, cx| t.agent_paste(agent, cx)))
            .when_some(notice, |d, text| {
                d.child(
                    div()
                        .px_1()
                        .pt_1()
                        .text_size(px(11.))
                        .text_color(p.warning)
                        .child(text),
                )
            })
            .when(!chips.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .pt_1()
                        .children(chips),
                )
            })
            .child(
                tab_moves_focus(div())
                    .px_1()
                    .child(Textarea::new(&input).appearance(false).text_size(px(12.5))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .min_w_0()
                    .child(add)
                    .child(div().flex_1().min_w_0().flex().child(selector))
                    .children(effort)
                    .child(action),
            )
            .into_any_element()
    }
}

/// Clipboard images go to KERN's own folder: previews and history refer to them.
fn save_pasted_image(image: &Image) -> anyhow::Result<std::path::PathBuf> {
    let dir = crate::core::settings::config_dir().join("attachments");
    std::fs::create_dir_all(&dir)?;
    let id = uuid::Uuid::new_v4();
    let direct = match image.format {
        ImageFormat::Png => Some("png"),
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Gif => Some("gif"),
        ImageFormat::Webp => Some("webp"),
        _ => None,
    };
    if let Some(ext) = direct {
        let path = dir.join(format!("paste-{id}.{ext}"));
        std::fs::write(&path, &image.bytes)?;
        return Ok(path);
    }
    // macOS screenshots can arrive as TIFF, Windows ones as BMP: agents take PNG.
    let path = dir.join(format!("paste-{id}.png"));
    let decoded = image::load_from_memory(&image.bytes)
        .map_err(|_| anyhow::anyhow!("this image format could not be converted to PNG"))?;
    decoded
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("could not save the pasted image: {e}"))?;
    Ok(path)
}

// ---- Feed rows ------------------------------------------------------------------

/// Cloned out of the app, so that drawing a row borrows nothing.
enum RowData {
    Item(ChatItem, bool),
    Footer,
    Missing,
}

fn render_row(
    this: &Entity<KernApp>,
    agent: AgentTab,
    ix: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let data = {
        let app = this.read(cx);
        match app.chat(agent) {
            Some(c) => match c.items.get(ix) {
                Some(item) => {
                    let open = matches!(item, ChatItem::Action(a) if c.expanded.contains(&a.id));
                    RowData::Item(item.clone(), open)
                }
                None if ix == c.items.len() => RowData::Footer,
                None => RowData::Missing,
            },
            None => RowData::Missing,
        }
    };
    let p = pal(cx);
    let row = div().w_full().px_2().py(px(3.));
    match data {
        RowData::Missing => div().into_any_element(),
        RowData::Footer => render_footer(this, agent, window, cx).into_any_element(),
        RowData::Item(item, open) => match item {
            ChatItem::User {
                text,
                attachments,
                images,
            } => row
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.92))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .rounded(px(RADIUS_LG))
                        .bg(p.user_bubble)
                        .text_size(px(12.5))
                        .when(!images.is_empty(), |d| {
                            d.child(div().flex().flex_wrap().gap_1().children(
                                images.into_iter().enumerate().map(|(i, path)| {
                                    let open = path.clone();
                                    div()
                                        .id(SharedString::from(format!("msg-img-{ix}-{i}")))
                                        .size(px(96.))
                                        .rounded(px(RADIUS))
                                        .overflow_hidden()
                                        .cursor_pointer()
                                        .on_click(window.listener_for(
                                            this,
                                            move |t: &mut KernApp, _, w, cx| {
                                                t.open_file_async(open.clone(), true, None, w, cx)
                                            },
                                        ))
                                        .child(img(path).size_full().object_fit(ObjectFit::Contain))
                                }),
                            ))
                        })
                        .children(attachments.into_iter().map(|a| {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(11.))
                                .text_color(p.accent)
                                .child(icon_el("paperclip", ICON_SM - 1., p.accent))
                                .child(a)
                        }))
                        .child(text),
                )
                .into_any_element(),
            ChatItem::Agent { id, text } => row
                .px_3()
                .child(markdown_view(&id, &text, p))
                .into_any_element(),
            ChatItem::Action(a) => row
                .child(render_action(this, agent, ix, &a, open, p, window))
                .into_any_element(),
            ChatItem::Note(n) => row
                .px_3()
                .text_size(px(11.5))
                .text_color(p.muted)
                .child(n)
                .into_any_element(),
            ChatItem::Error(e) => row
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(p.danger)
                        .text_size(px(12.))
                        .child(icon_el("circle-alert", ICON_MD, p.danger))
                        .child(div().flex_1().min_w_0().child(e)),
                )
                .into_any_element(),
        },
    }
}

fn markdown_view(id: &str, text: &str, p: Palette) -> impl IntoElement + use<> {
    let style = TextViewStyle::default().code_block(
        StyleRefinement::default()
            .bg(p.editor)
            .border_1()
            .border_color(p.border)
            .text_size(px(11.5)),
    );
    TextView::markdown(
        SharedString::from(format!("md-{id}")),
        SharedString::from(text.to_string()),
    )
    .style(style)
    .selectable(true)
    .text_size(px(12.5))
    .code_block_actions(|block, _, _| {
        let code = block.code();
        let digest = {
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut h = DefaultHasher::new();
            code.hash(&mut h);
            h.finish()
        };
        Clipboard::new(SharedString::from(format!("copy-{digest:x}")))
            .value(code)
            .tooltip("Copy")
    })
}

fn render_action(
    this: &Entity<KernApp>,
    agent: AgentTab,
    ix: usize,
    a: &ActionItem,
    open: bool,
    p: Palette,
    window: &mut Window,
) -> AnyElement {
    let id = a.id.clone();
    let (row_color, status): (Hsla, AnyElement) = match &a.status {
        ActionStatus::Running => (p.fg, Spinner::new().xsmall().into_any_element()),
        ActionStatus::Done => (
            p.muted,
            icon_el("check", ICON_SM, p.success).into_any_element(),
        ),
        ActionStatus::Declined => (
            p.warning,
            div()
                .text_size(px(10.5))
                .child("declined")
                .into_any_element(),
        ),
        ActionStatus::Failed(code) => (
            p.danger,
            div()
                .text_size(px(10.5))
                .child(code.map_or("failed".to_string(), |c| format!("exit {c}")))
                .into_any_element(),
        ),
    };
    let exit_line = a.exit_code.map(|c| format!("Exit code {c}"));
    let has_detail = !a.detail.is_empty() || !a.output.is_empty();
    div()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .when(open, |d| d.bg(p.surface))
        .child(
            div()
                .id(SharedString::from(format!("act-{id}")))
                .flex()
                .items_center()
                .gap_1()
                .h(px(ROW_H))
                .px_1()
                .rounded(px(RADIUS))
                .hover(move |s| s.bg(p.hover))
                .when(has_detail, |d| d.cursor_pointer())
                .on_click(window.listener_for(this, move |t, _, _, cx| {
                    if let Some(c) = t.chat_mut(agent) {
                        if !c.expanded.remove(&id) {
                            c.expanded.insert(id.clone());
                        }
                        c.scroller.update(cx, |s, cx| {
                            s.remeasure_items(ix..ix + 1, cx);
                        });
                    }
                    cx.notify();
                }))
                .child(icon_el(
                    if open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    ICON_SM,
                    p.muted,
                ))
                .child(icon_el(a.kind.icon(), ICON_SM, row_color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(11.5))
                        .text_color(row_color)
                        .child(a.label()),
                )
                .child(div().flex_shrink_0().text_color(row_color).child(status)),
        )
        .when(open, |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_1()
                    .pb_1()
                    .when(!a.detail.is_empty(), |d| {
                        d.child(code_text(a.detail.clone(), p))
                    })
                    .when(!a.output.is_empty(), |d| {
                        d.child(code_text(a.output.clone(), p).text_color(p.muted))
                    })
                    .when_some(exit_line, |d, line| {
                        d.child(
                            div()
                                .px_1()
                                .text_size(px(10.5))
                                .text_color(p.muted)
                                .child(line),
                        )
                    }),
            )
        })
        .into_any_element()
}

fn render_footer(
    this: &Entity<KernApp>,
    agent: AgentTab,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let p = pal(cx);
    let mut out = div().w_full().flex().flex_col().gap_1().px_2().pb_2();
    let (cards, status, review) = {
        let app = this.read(cx);
        let Some(c) = app.chat(agent) else {
            return out;
        };
        let cards: Vec<CardData> = c.pending.iter().map(CardData::from_request).collect();
        let status = if c.stopping {
            Some(("Stopping…".to_string(), false))
        } else if let Some(r) = &c.retry {
            let wait = r.until.map(|u| {
                u.saturating_duration_since(std::time::Instant::now())
                    .as_secs()
            });
            Some((
                match wait {
                    Some(w) if w > 0 => format!("{}. Retrying in {w}s", r.text),
                    _ => r.text.clone(),
                },
                true,
            ))
        } else if c.preparing {
            Some(("Recording project state…".to_string(), false))
        } else if c.running && c.pending.is_empty() {
            let secs = c.turn_started.map_or(0, |t| t.elapsed().as_secs() as i64);
            Some((format!("Working {}", usage::fmt_span(secs)), false))
        } else {
            None
        };
        let answers: Vec<BTreeMap<String, Vec<String>>> =
            c.pending.iter().map(|r| r.answers.clone()).collect();
        (
            cards.into_iter().zip(answers).collect::<Vec<_>>(),
            status,
            c.review
                .as_ref()
                .map(|r| (r.files.clone(), r.reverted.clone())),
        )
    };
    for (i, (card_data, answers)) in cards.into_iter().enumerate() {
        out = out.child(render_permission(
            this, agent, i, card_data, answers, p, window,
        ));
    }
    if let Some((s, warn)) = status {
        out = out.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_h(px(ROW_H))
                .px_1()
                .text_size(px(11.5))
                .text_color(if warn { p.warning } else { p.muted })
                .child(Spinner::new().xsmall())
                .child(div().flex_1().min_w_0().child(s)),
        );
    }
    if let Some((files, reverted)) = review {
        out = out.child(render_review(this, agent, files, reverted, p, window));
    }
    out
}

/// One question an agent asks, with its options as (label, description).
struct Question {
    question: String,
    options: Vec<(String, String)>,
    multi: bool,
}

struct CardData {
    title: String,
    code: Option<String>,
    lines: Vec<String>,
    questions: Vec<Question>,
    can_remember: bool,
    /// Codex can stop the whole task from here; a plain card just declines.
    can_stop: bool,
}

impl CardData {
    fn from_request(r: &PendingRequest) -> Self {
        match &r.request {
            PendingKind::Codex(s) => codex_card(s),
        }
    }
}

fn codex_card(s: &ServerRequest) -> CardData {
    let plain = |title: String, code: Option<String>, lines: Vec<String>| CardData {
        title,
        code,
        lines,
        questions: vec![],
        can_remember: true,
        can_stop: true,
    };
    match s {
        ServerRequest::CommandApproval {
            command,
            cwd,
            reason,
            network_host,
            ..
        } => {
            let mut lines = vec![];
            if let Some(h) = network_host {
                lines.push(format!("Network access to {h}"));
            }
            if let Some(c) = cwd {
                lines.push(format!("in {c}"));
            }
            if let Some(r) = reason {
                lines.push(r.clone());
            }
            plain(
                "Codex wants to run a command".into(),
                command.clone(),
                lines,
            )
        }
        ServerRequest::FileChangeApproval {
            reason, grant_root, ..
        } => {
            let mut lines = vec![];
            if let Some(r) = reason {
                lines.push(r.clone());
            }
            if let Some(g) = grant_root {
                lines.push(format!("Write access to {g}"));
            }
            plain("Codex wants to change files".into(), None, lines)
        }
        ServerRequest::Permissions {
            summary, reason, ..
        } => plain(
            "Codex asks for extra permissions".into(),
            Some(summary.clone()),
            reason.iter().cloned().collect(),
        ),
        ServerRequest::Elicitation { server, message } => CardData {
            can_remember: false,
            ..plain(
                format!("{server} asks for confirmation"),
                None,
                vec![message.clone()],
            )
        },
        ServerRequest::UserInput { questions, .. } => CardData {
            questions: questions
                .iter()
                .map(|q| Question {
                    question: q.question.clone(),
                    options: q
                        .options
                        .iter()
                        .map(|o| (o.clone(), String::new()))
                        .collect(),
                    multi: false,
                })
                .collect(),
            ..plain("Codex has a question".into(), None, vec![])
        },
        ServerRequest::Unsupported(m) => plain(m.clone(), None, vec![]),
    }
}

fn render_permission(
    this: &Entity<KernApp>,
    agent: AgentTab,
    ix: usize,
    data: CardData,
    answers: BTreeMap<String, Vec<String>>,
    p: Palette,
    window: &mut Window,
) -> AnyElement {
    let base = card(p)
        .border_color(p.accent)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(icon_el("shield-alert", ICON_MD, p.accent))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(data.title.clone()),
                ),
        )
        .when_some(data.code.clone(), |d, code| d.child(code_text(code, p)))
        .children(data.lines.iter().map(|l| {
            div()
                .text_size(px(11.))
                .text_color(p.muted)
                .child(l.clone())
        }));
    if !data.questions.is_empty() {
        let mut out = base;
        let mut any_multi = false;
        for (qi, q) in data.questions.iter().enumerate() {
            any_multi |= q.multi;
            let picked = answers.get(&q.question).cloned().unwrap_or_default();
            let mut options = div().flex().flex_wrap().gap_1();
            for (oi, (label, _)) in q.options.iter().enumerate() {
                let on = picked.contains(label);
                let (question, option, multi) = (q.question.clone(), label.clone(), q.multi);
                let b = if on {
                    primary_button(
                        SharedString::from(format!("q{ix}-{qi}-{oi}")),
                        label.clone(),
                        p,
                    )
                } else {
                    button(
                        SharedString::from(format!("q{ix}-{qi}-{oi}")),
                        label.clone(),
                        p,
                    )
                };
                options =
                    options.child(b.on_click(window.listener_for(this, move |t, _, _, cx| {
                        t.agent_choose_option(
                            agent,
                            ix,
                            question.clone(),
                            option.clone(),
                            multi,
                            cx,
                        )
                    })));
            }
            out = out
                .child(div().text_size(px(12.)).child(q.question.clone()))
                .child(options);
        }
        return out
            .child(
                div()
                    .flex()
                    .gap_1()
                    .when(any_multi, |d| {
                        d.child(
                            primary_button(SharedString::from(format!("q{ix}-send")), "Answer", p)
                                .on_click(window.listener_for(this, move |t, _, _, cx| {
                                    t.agent_submit_answers(agent, ix, cx)
                                })),
                        )
                    })
                    .child(
                        ghost_button(SharedString::from(format!("q{ix}-skip")), "Skip", p)
                            .on_click(window.listener_for(this, move |t, _, _, cx| {
                                t.agent_decide(agent, ix, Decision::Decline, cx)
                            })),
                    ),
            )
            .into_any_element();
    }
    base.child(
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(
                primary_button(SharedString::from(format!("ap{ix}-ok")), "Allow", p).on_click(
                    window.listener_for(this, move |t, _, _, cx| {
                        t.agent_decide(agent, ix, Decision::Accept, cx)
                    }),
                ),
            )
            .when(data.can_remember, |d| {
                d.child(
                    button(
                        SharedString::from(format!("ap{ix}-sess")),
                        "Always Allow",
                        p,
                    )
                    .on_click(window.listener_for(this, move |t, _, _, cx| {
                        t.agent_decide(agent, ix, Decision::AcceptForSession, cx)
                    })),
                )
            })
            .child(
                button(SharedString::from(format!("ap{ix}-no")), "Decline", p).on_click(
                    window.listener_for(this, move |t, _, _, cx| {
                        t.agent_decide(agent, ix, Decision::Decline, cx)
                    }),
                ),
            )
            .when(data.can_stop, |d| {
                d.child(
                    ghost_button(SharedString::from(format!("ap{ix}-stop")), "Stop Task", p)
                        .on_click(window.listener_for(this, move |t, _, _, cx| {
                            t.agent_decide(agent, ix, Decision::Cancel, cx)
                        })),
                )
            }),
    )
    .into_any_element()
}

fn render_review(
    this: &Entity<KernApp>,
    agent: AgentTab,
    files: Vec<ReviewedFile>,
    reverted: Vec<String>,
    p: Palette,
    window: &mut Window,
) -> AnyElement {
    let count = files.len();
    let rows: Vec<AnyElement> = files
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            let path = f.path.clone();
            let path2 = f.path.clone();
            let was_reverted = reverted.contains(&f.path);
            let (tag, color, can_revert, note) = match &f.attribution {
                Attribution::Agent => ("agent", p.muted, !was_reverted, None),
                Attribution::Conflict(why) => (
                    "conflict",
                    p.danger,
                    false,
                    Some(format!(
                        "Conflict: {why}. Automatic revert is off; review by hand."
                    )),
                ),
                Attribution::Unknown => (
                    "diff only",
                    p.warning,
                    false,
                    Some(
                        "Changed during the task but not reported as an agent edit. Automatic revert is off."
                            .to_string(),
                    ),
                ),
            };
            list_row(("rv", i), false, p)
                .on_click(window.listener_for(this, move |t, _, w, cx| {
                    t.agent_open_file_diff(agent, path.clone(), note.clone(), w, cx)
                }))
                .child(
                    div()
                        .w(px(12.))
                        .text_size(px(11.))
                        .text_color(p.accent)
                        .child(f.status.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .child(f.path.clone()),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(color)
                        .child(if was_reverted { "reverted" } else { tag }),
                )
                .when(can_revert, |d| {
                    d.child(
                        icon_button(("rv-undo", i), "undo-2", "Revert this file").on_click(
                            window.listener_for(this, move |t, ev: &ClickEvent, _, cx| {
                                let _ = ev;
                                cx.stop_propagation();
                                t.agent_revert_file(agent, path2.clone(), cx);
                            }),
                        ),
                    )
                })
                .into_any_element()
        })
        .collect();
    card(p)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!(
                            "{count} file{} changed",
                            if count == 1 { "" } else { "s" }
                        )),
                )
                .child(primary_button("chat-review", "Review Diff", p).on_click(
                    window.listener_for(this, move |t, _, w, cx| t.agent_open_review(agent, w, cx)),
                )),
        )
        .child(div().flex().flex_col().children(rows))
        .into_any_element()
}

// ---- Usage -----------------------------------------------------------------------

/// The exact words the product uses when a source gives no limits.
pub const NO_LIMITS: &str = "Лимиты недоступны для этого подключения";

fn usage_section(title: &'static str, p: Palette) -> Div {
    div()
        .mt_2()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.muted)
        .child(title)
}

fn usage_line(left: String, right: String, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(11.5))
        .child(div().flex_1().min_w_0().text_color(p.muted).child(left))
        .child(div().flex_shrink_0().child(right))
}

fn usage_window(w: &usage::RateWindow, now: i64, p: Palette) -> Div {
    let used = w.used_percent.clamp(0, 100);
    let color = if used >= 100 {
        p.danger
    } else if used >= 80 {
        p.warning
    } else {
        p.accent
    };
    let reset = match w.resets_at {
        Some(at) => format!(
            "Resets in {} ({})",
            usage::fmt_span(at - now),
            usage::fmt_clock(at, now)
        ),
        None => "Reset time unknown".to_string(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(usage_line(
            w.label(),
            format!("{used}% used · {}% left", 100 - used),
            p,
        ))
        .child(
            div().h(px(4.)).w_full().rounded_full().bg(p.active).child(
                div()
                    .h_full()
                    .rounded_full()
                    .bg(color)
                    .w(relative(used as f32 / 100.)),
            ),
        )
        .child(div().text_size(px(10.5)).text_color(p.muted).child(reset))
}

fn usage_tokens(t: &usage::TokenUsage, p: Palette) -> Vec<AnyElement> {
    let mut rows = vec![
        usage_line("Input".into(), usage::fmt_tokens(t.input), p),
        usage_line("Output".into(), usage::fmt_tokens(t.output), p),
    ];
    if t.cached > 0 {
        rows.push(usage_line(
            "Cache read".into(),
            usage::fmt_tokens(t.cached),
            p,
        ));
    }
    if t.cache_write > 0 {
        rows.push(usage_line(
            "Cache write".into(),
            usage::fmt_tokens(t.cache_write),
            p,
        ));
    }
    if let (Some(used), Some(window)) = (t.context_used, t.context_window)
        && window > 0
    {
        rows.push(usage_line(
            "Context".into(),
            format!(
                "{} of {} ({}%)",
                usage::fmt_tokens(used),
                usage::fmt_tokens(window),
                used * 100 / window
            ),
            p,
        ));
    }
    if let Some(cost) = t.cost_usd {
        rows.push(usage_line(
            "Estimated cost".into(),
            format!("${cost:.4}"),
            p,
        ));
    }
    rows.into_iter()
        .map(IntoElement::into_any_element)
        .collect()
}

/// The popover body. Tokens, API limits and subscription limits stay separate, and
/// nothing is shown that the connection did not report.
pub fn render_usage(
    this: &Entity<KernApp>,
    agent: AgentTab,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let p = pal(cx);
    let now = crate::agents::sessions::now();
    let app = this.read(cx);
    let Some(c) = app.chat(agent) else {
        return div().into_any_element();
    };
    let u = c.usage.clone();
    let chatgpt = matches!(
        app.project.as_ref().map(|x| &x.codex.conn),
        Some(super::codex_panel::Conn::Connected { description }) if description.starts_with("ChatGPT")
    );
    let unavailable = |p: Palette| {
        div()
            .text_size(px(11.5))
            .text_color(p.muted)
            .child(NO_LIMITS)
            .into_any_element()
    };
    let not_used = |text: &'static str, p: Palette| {
        div()
            .text_size(px(11.5))
            .text_color(p.muted)
            .child(text)
            .into_any_element()
    };
    let tokens: Vec<AnyElement> = match &u.tokens {
        Some(t) => usage_tokens(t, p),
        None => vec![not_used("Nothing reported for this chat yet.", p)],
    };
    let mut subscription: Vec<AnyElement> = vec![];
    let mut updated: Option<i64> = None;
    let api: AnyElement;
    match agent {
        AgentTab::Codex => {
            api = if chatgpt {
                not_used("Not used: this connection is a ChatGPT sign-in.", p)
            } else {
                unavailable(p)
            };
            match &u.limits {
                Some(l) if !l.buckets.is_empty() => {
                    updated = Some(l.updated_at);
                    for b in &l.buckets {
                        if l.buckets.len() > 1 {
                            subscription.push(
                                div()
                                    .text_size(px(11.))
                                    .child(b.name.clone())
                                    .into_any_element(),
                            );
                        }
                        for w in &b.windows {
                            subscription.push(usage_window(w, now, p).into_any_element());
                        }
                    }
                    if let Some(credits) = &l.credits {
                        subscription.push(
                            usage_line("Credits".into(), credits.clone(), p).into_any_element(),
                        );
                    }
                    if l.allowed == Some(false) {
                        subscription.push(
                            div()
                                .text_size(px(11.5))
                                .text_color(p.danger)
                                .child("Included usage is not available right now.")
                                .into_any_element(),
                        );
                    }
                }
                _ => subscription.push(unavailable(p)),
            }
        }
    }
    let refresh = (agent == AgentTab::Codex).then(|| {
        icon_button("usage-refresh", "refresh-cw", "Refresh").on_click(
            window.listener_for(this, |t: &mut KernApp, _, _, cx| t.codex_refresh_limits(cx)),
        )
    });
    div()
        .id("usage-popover")
        .w(px(260.))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{} usage", agent.short_name())),
                )
                .children(refresh),
        )
        .child(usage_section("TOKENS IN THIS CHAT", p))
        .children(tokens)
        .child(usage_section("SUBSCRIPTION LIMITS", p))
        .children(subscription)
        .child(usage_section("API LIMITS", p))
        .child(api)
        .when_some(updated, |d, at| {
            d.child(
                div()
                    .mt_1()
                    .text_size(px(10.5))
                    .text_color(p.muted)
                    .child(format!(
                        "Updated {}",
                        match crate::agents::sessions::relative_time(at, now).as_str() {
                            "now" => "just now".to_string(),
                            other => format!("{other} ago"),
                        }
                    )),
            )
        })
        .into_any_element()
}
