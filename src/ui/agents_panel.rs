//! The Codex panel on the right: header, chat history and the shared Agent Chat.
//! Claude Code runs as the official `claude` CLI in its own tab of the bottom
//! panel, one live session per project. KERN never reads its terminal output or
//! its credentials; sign-in and history stay with Claude Code.
use super::agent_chat::render_usage as chat_render_usage;
use super::app::*;
use super::assets::icon;
use super::terminal_view::TerminalView;
use super::theme::{Palette, pal};
use super::widgets::*;
use super::*;
use crate::agents::chat::ChatPhase;
use crate::agents::sessions::{self, SessionEntry};
use crate::platform::tools::{self, ToolStatus};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentTab {
    Codex,
}

impl AgentTab {
    pub fn id(self) -> &'static str {
        "codex"
    }

    pub fn sessions_key(self) -> &'static str {
        "codex"
    }

    pub fn name(self) -> &'static str {
        "Codex"
    }

    pub fn short_name(self) -> &'static str {
        "Codex"
    }

    pub fn icon(self) -> &'static str {
        "agent-codex"
    }
}

impl KernApp {
    fn project_root(&self) -> Option<std::path::PathBuf> {
        self.project.as_ref().map(|p| p.root.clone())
    }

    pub fn rename_session(&mut self, agent: &str, id: &str, title: &str, cx: &mut Context<Self>) {
        let Some(root) = self.project_root() else {
            return;
        };
        self.sessions.rename(agent, &root, id, title);
        self.sessions.save();
        if agent == "codex"
            && let Some(client) = self.project.as_ref().and_then(|p| p.codex.client.clone())
        {
            let call = client.call(
                "thread/name/set",
                serde_json::json!({"threadId": id, "name": title}),
            );
            cx.background_spawn(async move {
                if let Err(e) = call.await {
                    log::warn!("thread/name/set: {}", e.message);
                }
            })
            .detach();
        }
        cx.notify();
    }

    // ---- Claude Code: the official CLI in the bottom panel -------------------

    /// The live Claude Code session of this project, if its process still runs.
    fn claude_live(&self, cx: &App) -> Option<Entity<TerminalView>> {
        self.project
            .as_ref()?
            .claude_code
            .clone()
            .filter(|v| v.read(cx).exited.is_none())
    }

    /// Open Claude Code: focus the project's live session, or start `claude` in the
    /// project folder. Never a second process for the same project.
    pub fn open_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        self.settings.layout.bottom_visible = true;
        if let Some(view) = self.claude_live(cx) {
            if let Some(p) = &mut self.project {
                p.bottom_tab = BottomTab::Claude;
            }
            view.update(cx, |v, cx| v.focus(window, cx));
            cx.notify();
            return;
        }
        let Some(p) = &mut self.project else { return };
        p.bottom_tab = BottomTab::Claude;
        match tools::locate("claude") {
            ToolStatus::Found(program) => {
                p.claude_message = None;
                let root = p.root.clone();
                let view = cx.new(|cx| {
                    TerminalView::new(
                        "Claude Code",
                        Some((program.to_string_lossy().to_string(), vec![])),
                        &root,
                        None,
                        cx,
                    )
                });
                cx.observe(&view, |_, _, cx| cx.notify()).detach();
                view.update(cx, |v, cx| v.focus(window, cx));
                p.claude_code = Some(view);
            }
            ToolStatus::Broken(path) => {
                p.claude_message = Some(format!(
                    "The claude command at {} cannot be run. Reinstall it:",
                    path.display()
                ));
            }
            ToolStatus::Missing => {
                p.claude_message = Some("Claude Code is not installed. Install it:".to_string())
            }
        }
        cx.notify();
    }

    /// End the session's process. Its sign-in and history stay with Claude Code.
    pub fn claude_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.project {
            p.claude_code = None;
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn render_claude_bottom(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some(project) = &self.project else {
            return div().into_any_element();
        };
        if let Some(msg) = project.claude_message.clone() {
            return div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(12.))
                .child(msg)
                .child(code_text(tools::install_hint("claude"), p))
                .child(
                    div()
                        .text_color(p.muted)
                        .child("Sign-in happens in Claude Code itself, in the browser."),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            primary_button("claude-recheck", "Check Again", p)
                                .on_click(cx.listener(|t, _, w, cx| t.open_claude(w, cx))),
                        )
                        .child(button("claude-docs", "Open Install Guide", p).on_click(
                            cx.listener(|_, _, _, cx| cx.open_url(bottom::CLAUDE_INSTALL_URL)),
                        )),
                )
                .into_any_element();
        }
        let Some(view) = project.claude_code.clone() else {
            return div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(12.))
                .child(div().text_color(p.muted).child(
                    "The official claude CLI in this project. Sign-in, subscription and history stay with Claude Code.",
                ))
                .child(div().flex().child(
                    primary_button("claude-open", "Start Claude Code", p)
                        .on_click(cx.listener(|t, _, w, cx| t.open_claude(w, cx))),
                ))
                .into_any_element();
        };
        let ended = view.read(cx).exited;
        div()
            .size_full()
            .flex()
            .flex_col()
            .when_some(ended, |d, code| {
                d.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(p.hover)
                        .border_b_1()
                        .border_color(p.border)
                        .child(div().flex_1().text_size(px(12.)).child(match code {
                            Some(c) => format!("Claude Code exited with code {c}."),
                            None => "Claude Code has ended.".to_string(),
                        }))
                        .child(
                            primary_button("claude-again", "Start Again", p)
                                .on_click(cx.listener(|t, _, w, cx| t.open_claude(w, cx))),
                        ),
                )
            })
            .child(div().flex_1().min_h_0().w_full().child(view))
            .into_any_element()
    }

    // ---- The panel -----------------------------------------------------------

    /// Current chat state as one small dot in the header.
    fn agent_status(&self, agent: AgentTab, p: Palette) -> (Hsla, String) {
        use super::codex_panel::Conn as C;
        let Some(project) = &self.project else {
            return (p.muted, String::new());
        };
        let (ready, connecting, description) = match agent {
            AgentTab::Codex => match &project.codex.conn {
                C::Connected { description } => (true, false, description.clone()),
                C::Starting => (false, true, "Connecting".to_string()),
                C::LoginRequired | C::WaitingForLogin { .. } => {
                    (false, false, "Sign-in required".to_string())
                }
                C::CliMissing { .. } => (false, false, "CLI not found".to_string()),
                C::Error(e) => (false, false, e.clone()),
                C::NotStarted => (false, false, "Not connected".to_string()),
            },
        };
        let Some(chat) = self.chat(agent) else {
            return (p.muted, description);
        };
        match chat.phase(ready, connecting) {
            ChatPhase::Setup => (p.warning, description),
            ChatPhase::Connecting => (p.muted, description),
            ChatPhase::Responding => (p.accent, format!("Responding · {description}")),
            ChatPhase::AwaitingPermission => (p.warning, "Waiting for your decision".to_string()),
            ChatPhase::Stopping => (p.warning, "Stopping".to_string()),
            ChatPhase::Failed => (p.danger, format!("Last turn failed · {description}")),
            ChatPhase::Empty | ChatPhase::Idle => (p.success, description),
        }
    }

    /// The current chat's title, from the session index.
    fn agent_chat_title(&self, agent: AgentTab) -> String {
        let (Some(root), Some(id)) = (
            self.project_root(),
            self.chat(agent).and_then(|c| c.session_id.clone()),
        ) else {
            return "New Chat".to_string();
        };
        self.sessions
            .list(agent.sessions_key(), &root, "")
            .into_iter()
            .find(|e| e.id == id)
            .map_or_else(|| "Chat".to_string(), |e| e.title)
    }

    fn render_agents_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::component::popover::Popover;
        let p = pal(cx);
        let tab = AgentTab::Codex;
        let (dot, status) = self.agent_status(tab, p);
        let this = cx.entity();
        let enabled = self.settings.extension_enabled(tab.id());
        let history_on = self.chat(tab).is_some_and(|c| c.show_history);
        let usage_open = self.chat(tab).is_some_and(|c| c.show_usage);
        let title = self.agent_chat_title(tab);
        let codex_needs_login = matches!(
            self.project.as_ref().map(|x| &x.codex.conn),
            Some(codex_panel::Conn::LoginRequired | codex_panel::Conn::WaitingForLogin { .. })
        );
        let menu_app = this.clone();
        let menu = icon_button("ag-menu", "ellipsis", "More actions").dropdown_menu_with_anchor(
            Anchor::TopRight,
            move |menu, window, _cx| {
                let this = &menu_app;
                let mut m = menu;
                if codex_needs_login {
                    m = m.item(
                        PopupMenuItem::new("Sign In with ChatGPT")
                            .icon(icon("log-in"))
                            .on_click(
                                window.listener_for(this, |t: &mut KernApp, _, _, cx| {
                                    t.codex_login(cx)
                                }),
                            ),
                    );
                }
                m.item(
                    PopupMenuItem::new("Review Diff")
                        .icon(icon("git-compare"))
                        .on_click(window.listener_for(this, move |t: &mut KernApp, _, w, cx| {
                            t.agent_open_review(tab, w, cx)
                        })),
                )
                .item(
                    PopupMenuItem::new("Account and Connection")
                        .icon(icon("settings"))
                        .on_click(window.listener_for(this, |t: &mut KernApp, _, w, cx| {
                            t.settings_form.category = settings_panel::Category::Extensions;
                            t.open_settings_tab(w, cx);
                        })),
                )
            },
        );
        let usage_app = this.clone();
        let change_app = this.clone();
        let usage = Popover::new("ag-usage")
            .anchor(Anchor::TopRight)
            .open(usage_open)
            .on_open_change(move |open, _, cx| {
                let open = *open;
                change_app.update(cx, |t, cx| {
                    if let Some(c) = t.chat_mut(tab) {
                        c.show_usage = open;
                    }
                    if open {
                        t.codex_refresh_limits(cx);
                    }
                    cx.notify();
                });
            })
            .trigger(icon_button("ag-usage-btn", "gauge", "Usage and limits"))
            .content(move |_, window, cx| chat_render_usage(&usage_app, tab, window, cx));
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(p.border)
            .child(
                panel_header("Codex", p)
                    .child(
                        div()
                            .id("ag-status")
                            .size(px(7.))
                            .mx_1()
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(dot)
                            .aria_label(status.clone())
                            .tooltip({
                                let status = status.clone();
                                move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(status.clone())
                                        .build(w, cx)
                                }
                            }),
                    )
                    .when(enabled, |d| d.child(usage))
                    .child(menu),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .h(px(28.))
                    .pl_2()
                    .pr_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .child(title),
                    )
                    .when(enabled, |d| {
                        d.child(
                            icon_toggle("ag-history", "history", "History", history_on, p)
                                .on_click(cx.listener(move |t, _, _, cx| {
                                    if let Some(c) = t.chat_mut(tab) {
                                        c.show_history = !c.show_history;
                                    }
                                    t.codex_refresh_history(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            icon_button("ag-new", "square-pen", "New Chat").on_click(
                                cx.listener(move |t, _, _, cx| t.agent_new_chat(tab, cx)),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn session_rows(
        &self,
        agent: AgentTab,
        entries: Vec<SessionEntry>,
        current: Option<String>,
        project: String,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = pal(cx);
        let now = sessions::now();
        entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                let id = e.id.clone();
                let rename_id = e.id.clone();
                let rename_title = e.title.clone();
                let on = current.as_deref() == Some(e.id.as_str());
                let group: SharedString = format!("sess-{i}").into();
                list_row(
                    SharedString::from(format!("sess-{}-{i}", agent.id())),
                    on,
                    p,
                )
                .group(group.clone())
                .py_1()
                .on_click(
                    cx.listener(move |t, _, _, cx| t.agent_open_session(agent, id.clone(), cx)),
                )
                .child(session_text(&e.title, &project, e.last_active, now, p))
                .child(
                    div().invisible().group_hover(group, |s| s.visible()).child(
                        icon_button(
                            SharedString::from(format!("sess-ren-{}-{i}", agent.id())),
                            "pencil",
                            "Rename",
                        )
                        .on_click(cx.listener(move |t, _, w, cx| {
                            cx.stop_propagation();
                            t.overlay = Some(overlays::Overlay::prompt(
                                "Rename Chat",
                                "Shown in this chat list.",
                                &rename_title,
                                overlays::PromptKind::RenameSession {
                                    agent: agent.sessions_key().to_string(),
                                    id: rename_id.clone(),
                                },
                                w,
                                cx,
                            ));
                            cx.notify();
                        })),
                    ),
                )
                .into_any_element()
            })
            .collect()
    }

    /// Search and the chats of this project.
    pub fn render_chat_history(&mut self, agent: AgentTab, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some(root) = self.project_root() else {
            return div().into_any_element();
        };
        let project = self
            .project
            .as_ref()
            .map(|x| x.name.clone())
            .unwrap_or_default();
        let query = self.agents_search.read(cx).value().to_string();
        let chats = self.sessions.list(agent.sessions_key(), &root, &query);
        let nothing = chats.is_empty();
        let current = self.chat(agent).and_then(|c| c.session_id.clone());
        let rows = self.session_rows(agent, chats, current, project, cx);
        let input = self.agents_search.clone();
        div()
            .id("chat-history")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .gap(px(1.))
            .p_2()
            .child(div().pb_1().child(Input::new(&input).small()))
            .children(rows)
            .when(nothing, |d| {
                d.child(empty_state(
                    "history",
                    "No chats here yet",
                    format!(
                        "Chats with {} in this project appear here.",
                        agent.short_name()
                    ),
                    p,
                ))
            })
            .into_any_element()
    }

    pub fn render_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let codex_on = self.settings.extension_enabled("codex");
        let tab = AgentTab::Codex;
        // The panel is in use once it is on screen: connect, then show models and limits.
        let not_started = self
            .project
            .as_ref()
            .is_some_and(|x| matches!(x.codex.conn, codex_panel::Conn::NotStarted));
        if not_started && codex_on {
            cx.defer_in(window, move |t, _, cx| t.agent_connect(tab, cx));
        }
        let header = self.render_agents_header(cx);
        let body: AnyElement =
            if !codex_on {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .p_3()
                    .gap_2()
                    .child(empty_state(
                        "blocks",
                        "Codex is not enabled",
                        "Enable Codex in Extensions.",
                        p,
                    ))
                    .child(primary_button("agents-ext", "Open Extensions", p).on_click(
                        cx.listener(|t, _, w, cx| {
                            t.side = SideView::SourceControl;
                            t.show_side(SideView::Extensions, w, cx);
                        }),
                    ))
                    .into_any_element()
            } else {
                self.render_agent_chat(tab, window, cx)
            };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.panel)
            .child(header)
            .child(div().flex_1().min_h_0().w_full().child(body))
            .into_any_element()
    }
}

fn session_text(title: &str, project: &str, at: i64, now: i64, p: Palette) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(10.5))
                .text_color(p.muted)
                .child(format!("{project} · {}", sessions::relative_time(at, now))),
        )
}
