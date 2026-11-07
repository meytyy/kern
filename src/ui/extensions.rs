//! Extensions: KERN's own built-in catalog. Only these integrations exist; there
//! is no loading of third-party code. Install and Uninstall change KERN's state
//! only. External CLIs, sign-ins and chat history are never touched.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use super::*;
use crate::core::settings::ExtensionState;
use crate::platform::tools::Probe;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct ExtensionDef {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
    pub cli: &'static str,
    pub install: &'static str,
    pub docs: &'static str,
    pub adds: &'static str,
}

pub const CATALOG: &[ExtensionDef] = &[
    ExtensionDef {
        id: "codex",
        name: "Codex",
        icon: "agent-codex",
        description: "Chat in the Agents panel through the official App Server.",
        cli: "codex",
        install: "brew install --cask codex",
        docs: "https://developers.openai.com/codex/cli",
        adds: "The Codex panel, Ask Agent in Run Output, attach commands",
    },
    ExtensionDef {
        id: "claude",
        name: "Claude Code",
        icon: "agent-claude",
        description: "The official claude CLI in its own tab of the bottom panel.",
        cli: "claude",
        install: "curl -fsSL https://claude.ai/install.sh | bash",
        docs: "https://code.claude.com/docs/en/quickstart",
        adds: "A Claude Code tab in the bottom panel, Claude Code: Open in the palette",
    },
];

pub fn display_name(id: &str) -> String {
    CATALOG
        .iter()
        .find(|e| e.id == id)
        .map_or(id, |e| e.name)
        .to_string()
}

/// Facts about an extension, gathered off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct ExtensionFacts {
    pub signed_in: Option<bool>,
    pub detail: String,
    pub checking: bool,
}

impl KernApp {
    pub fn after_probes(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn cli_probe(&self, def: &ExtensionDef) -> Option<&Probe> {
        self.settings_form.probes.get(def.cli)
    }

    pub fn open_extension_page(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The editor that had focus is about to leave the screen, and shortcuts need a target.
        window.focus(&self.focus, cx);
        if let Some(p) = &mut self.project {
            let ix = p
                .tabs
                .iter()
                .position(|t| matches!(t, editor_area::Tab::Extension(x) if x == id));
            let ix = ix.unwrap_or_else(|| {
                p.tabs.push(editor_area::Tab::Extension(id.to_string()));
                p.tabs.len() - 1
            });
            p.active = Some(ix);
        }
        self.check_extension(id, cx);
    }

    /// Check Again: runs the CLI and asks it for the sign-in state.
    pub fn check_extension(&mut self, id: &str, cx: &mut Context<Self>) {
        self.run_tool_probes(cx);
        let id = id.to_string();
        self.extension_facts.entry(id.clone()).or_default().checking = true;
        let cwd = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir);
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let facts = cx
                .background_spawn(async move { sign_in_state(&key, &cwd) })
                .await;
            this.update(cx, |this, cx| {
                this.extension_facts.insert(id, facts);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn install_extension(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.set_extension(
            id,
            ExtensionState {
                installed: true,
                enabled: true,
            },
        );
        self.persist();
        self.extensions_changed(window, cx);
        self.check_extension(id, cx);
    }

    pub fn enable_extension(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.set_extension(
            id,
            ExtensionState {
                installed: true,
                enabled: true,
            },
        );
        self.persist();
        self.extensions_changed(window, cx);
    }

    fn extension_busy(&self, id: &str, cx: &App) -> Option<String> {
        let p = self.project.as_ref()?;
        match id {
            "codex" if p.codex.running || p.codex.preparing => {
                Some("Codex is working on a task.".to_string())
            }
            "claude" => p
                .claude_code
                .as_ref()
                .filter(|v| v.read(cx).exited.is_none())
                .map(|_| "A Claude Code session is open in the bottom panel.".to_string()),
            _ => None,
        }
    }

    /// Disable or Uninstall. An active task needs a decision first.
    pub fn turn_off_extension(
        &mut self,
        id: &str,
        uninstall: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(reason) = self.extension_busy(id, cx) {
            let verb = if uninstall { "uninstall" } else { "disable" };
            self.overlay = Some(overlays::Overlay::confirm(
                &format!("{} is in use", display_name(id)),
                format!(
                    "{reason} Stop it and {verb} the extension? The CLI, your sign-in and chat history stay."
                ),
                vec![overlays::ConfirmChoice::new(
                    "Stop and Continue",
                    overlays::ConfirmAction::ExtensionOff {
                        id: id.to_string(),
                        uninstall,
                    },
                    true,
                )],
            ));
            cx.notify();
            return;
        }
        self.force_extension_off(id, uninstall, window, cx);
    }

    pub fn force_extension_off(
        &mut self,
        id: &str,
        uninstall: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "codex" => self.codex_shutdown(cx),
            "claude" => {
                if let Some(p) = &mut self.project {
                    p.claude_code = None;
                }
            }
            _ => {}
        }
        self.settings.set_extension(
            id,
            ExtensionState {
                installed: !uninstall,
                enabled: false,
            },
        );
        self.persist();
        self.extensions_changed(window, cx);
    }

    /// Commands and panels follow from the settings; only stale selections remain.
    fn extensions_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let claude_on = self.settings.extension_enabled("claude");
        if !self.settings.extension_enabled("codex") {
            self.settings.layout.codex_visible = false;
        }
        if let Some(p) = &mut self.project
            && !claude_on
            && p.bottom_tab == BottomTab::Claude
        {
            p.bottom_tab = BottomTab::Terminal;
        }
        self.persist();
        cx.notify();
    }

    fn state_chips(&self, def: &ExtensionDef, cx: &App) -> Div {
        let p = pal(cx);
        let st = self.settings.extension(def.id);
        let facts = self.extension_facts.get(def.id);
        let chip = |label: String, color: Hsla| {
            div()
                .px_1()
                .rounded(px(3.))
                .border_1()
                .border_color(color)
                .text_size(px(10.))
                .text_color(color)
                .child(label)
        };
        let cli = match self.cli_probe(def) {
            Some(Probe::Ready { .. }) => chip("CLI Ready".into(), p.success),
            Some(Probe::Broken { .. }) => chip("CLI Broken".into(), p.danger),
            Some(Probe::Missing) => chip("CLI Missing".into(), p.warning),
            None => chip("CLI ?".into(), p.muted),
        };
        let signed = match facts.map(|f| (f.checking, f.signed_in)) {
            Some((true, _)) => chip("Checking…".into(), p.muted),
            Some((_, Some(true))) => chip("Signed In".into(), p.success),
            Some((_, Some(false))) => chip("Not Signed In".into(), p.warning),
            _ => chip("Sign-in unknown".into(), p.muted),
        };
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(chip(
                if st.installed {
                    "Installed"
                } else {
                    "Not Installed"
                }
                .into(),
                if st.installed { p.success } else { p.muted },
            ))
            .when(st.installed, |d| {
                d.child(chip(
                    if st.enabled { "Enabled" } else { "Disabled" }.into(),
                    if st.enabled { p.success } else { p.muted },
                ))
            })
            .child(cli)
            .child(signed)
    }

    /// One short status for a card. Diagnostics live on the details page.
    fn card_status(&self, def: &ExtensionDef, p: super::theme::Palette) -> (String, Hsla) {
        use super::codex_panel::Conn as C;
        let st = self.settings.extension(def.id);
        if !st.installed {
            return ("Not installed".into(), p.muted);
        }
        if !st.enabled {
            return ("Disabled".into(), p.muted);
        }
        let project = self.project.as_ref();
        match def.id {
            "codex" => match project.map(|x| &x.codex.conn) {
                Some(C::Connected { .. }) => ("Connected".into(), p.success),
                Some(C::LoginRequired | C::WaitingForLogin { .. }) => {
                    ("Sign-in required".into(), p.warning)
                }
                Some(C::CliMissing { .. }) => ("CLI not found".into(), p.warning),
                Some(C::Error(_)) => ("Error".into(), p.danger),
                _ => ("Enabled".into(), p.success),
            },
            _ => match crate::platform::tools::locate("claude") {
                crate::platform::tools::ToolStatus::Found(_) => ("Ready".into(), p.success),
                _ => ("CLI not found".into(), p.warning),
            },
        }
    }

    /// Open: Codex in the right panel, Claude Code in its terminal session.
    pub fn open_agent_from_extension(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id == "claude" {
            self.open_claude(window, cx);
            return;
        }
        let agent = super::agents_panel::AgentTab::Codex;
        self.settings.layout.codex_visible = true;
        self.agent_connect(agent, cx);
        self.persist();
        cx.notify();
    }

    fn extension_card(
        &self,
        i: usize,
        def: &'static ExtensionDef,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
        let p = pal(cx);
        let id = def.id;
        let st = self.settings.extension(id);
        let (status, color) = self.card_status(def, p);
        let this = cx.entity();
        let main = if !st.installed {
            primary_button(("ext-main", i), "Install", p)
                .on_click(cx.listener(move |t, _, w, cx| t.install_extension(id, w, cx)))
        } else if !st.enabled {
            primary_button(("ext-main", i), "Enable", p)
                .on_click(cx.listener(move |t, _, w, cx| t.enable_extension(id, w, cx)))
        } else {
            primary_button(("ext-main", i), "Open", p)
                .on_click(cx.listener(move |t, _, w, cx| t.open_agent_from_extension(id, w, cx)))
        };
        let menu = icon_button(("ext-menu", i), "ellipsis", "More actions")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, _| {
                let mut m = menu.item(PopupMenuItem::new("Details and Diagnostics").on_click(
                    window.listener_for(&this, move |t: &mut KernApp, _, w, cx| {
                        t.open_extension_page(id, w, cx)
                    }),
                ));
                if id == "claude" {
                    m =
                        m.item(PopupMenuItem::new("Open Official Claude Code").on_click(
                            window.listener_for(&this, |t: &mut KernApp, _, w, cx| {
                                t.open_claude(w, cx)
                            }),
                        ));
                }
                m = m.item(PopupMenuItem::new("Check Again").on_click(
                    window.listener_for(&this, move |t: &mut KernApp, _, _, cx| {
                        t.check_extension(id, cx)
                    }),
                ));
                m = m.item(
                    PopupMenuItem::new("Official Docs")
                        .on_click(move |_, _, cx: &mut App| cx.open_url(def.docs)),
                );
                if st.installed {
                    m = m.separator();
                    if st.enabled {
                        m = m.item(PopupMenuItem::new("Disable").on_click(
                            window.listener_for(&this, move |t: &mut KernApp, _, w, cx| {
                                t.turn_off_extension(id, false, w, cx)
                            }),
                        ));
                    }
                    m = m.item(PopupMenuItem::new("Uninstall").on_click(
                        window.listener_for(&this, move |t: &mut KernApp, _, w, cx| {
                            t.turn_off_extension(id, true, w, cx)
                        }),
                    ));
                }
                m
            });
        card(p)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(icon_el(def.icon, 20., p.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(def.name),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(p.muted)
                                    .child(def.description),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(chip(status, color))
                    .child(div().flex_1())
                    .child(main)
                    .child(menu),
            )
            .into_any_element()
    }

    pub fn render_extensions_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let q = self.extension_query.read(cx).value().to_lowercase();
        let input = self.extension_query.clone();
        let cards: Vec<AnyElement> = CATALOG
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                q.is_empty()
                    || e.name.to_lowercase().contains(q.as_ref() as &str)
                    || e.description.to_lowercase().contains(q.as_ref() as &str)
            })
            .map(|(i, def)| self.extension_card(i, def, cx))
            .collect();
        let nothing = cards.is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(panel_header("Extensions", p))
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .flex_shrink_0()
                    .child(Input::new(&input)),
            )
            .child(
                div()
                    .id("ext-cards")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_2()
                    .children(cards)
                    .when(nothing, |d| {
                        d.child(empty_state(
                            "search",
                            "Nothing found",
                            "Try another name.",
                            p,
                        ))
                    }),
            )
    }

    pub fn render_extension_page(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some(def) = CATALOG.iter().find(|e| e.id == id) else {
            return div().into_any_element();
        };
        let st = self.settings.extension(id);
        let chips = self.state_chips(def, cx);
        let probe = self.cli_probe(def).cloned();
        let detail = self
            .extension_facts
            .get(id)
            .map(|f| f.detail.clone())
            .unwrap_or_default();
        let cli_line: AnyElement = match &probe {
            Some(Probe::Ready { path, version }) => div().text_size(px(12.)).child(format!("Using {} {version}", path.display())).into_any_element(),
            Some(Probe::Broken { path, reason }) => div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .child(div().text_color(p.danger).child(format!("{} cannot run: {reason}", path.display())))
                .child(div().text_color(p.muted).child("KERN does not bypass macOS warnings or run unknown copies. Repair it with the official installer:"))
                .child(div().font_family("Menlo").text_color(p.accent).child(def.install))
                .into_any_element(),
            Some(Probe::Missing) => div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .child(div().text_color(p.warning).child(format!("The {} CLI is not installed. Install it with the official method:", def.cli)))
                .child(div().font_family("Menlo").text_color(p.accent).child(def.install))
                .into_any_element(),
            None => div().text_size(px(12.)).text_color(p.muted).child("Checking the CLI…").into_any_element(),
        };
        let (a, b) = (id.to_string(), id.to_string());
        let (c, d2) = (id.to_string(), id.to_string());
        let e = id.to_string();
        let docs = def.docs;
        div()
            .id("ext-page")
            .size_full()
            .overflow_y_scroll()
            .bg(p.editor)
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(icon_el(def.icon, 40., p.accent))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(def.name))
                            .child(div().text_size(px(11.)).text_color(p.muted).child(format!("Built into KERN · requires the official {} CLI", def.cli)))
                            .child(chips),
                    ),
            )
            .child(div().text_size(px(13.)).child(def.description))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .when(!st.installed, |x| x.child(primary_button("ext-install", "Install", p).on_click(cx.listener(move |t, _, w, cx| t.install_extension(&a, w, cx)))))
                    .when(st.installed && !st.enabled, |x| x.child(primary_button("ext-enable", "Enable", p).on_click(cx.listener(move |t, _, w, cx| t.enable_extension(&b, w, cx)))))
                    .when(st.installed && st.enabled, |x| x.child(button("ext-disable", "Disable", p).on_click(cx.listener(move |t, _, w, cx| t.turn_off_extension(&c, false, w, cx)))))
                    .when(st.installed, |x| x.child(button("ext-uninstall", "Uninstall", p).on_click(cx.listener(move |t, _, w, cx| t.turn_off_extension(&d2, true, w, cx)))))
                    .child(button("ext-check", "Check Again", p).on_click(cx.listener(move |t, _, _, cx| t.check_extension(&e, cx))))
                    .child(button("ext-docs", "Official Docs", p).on_click(cx.listener(move |_, _, _, cx| cx.open_url(docs)))),
            )
            .child(hsep(p))
            .child(cli_line)
            .when(!detail.is_empty(), |x| x.child(div().text_size(px(12.)).text_color(p.muted).child(detail)))
            .child(div().text_size(px(11.)).text_color(p.muted).child(format!("Adds: {}.", def.adds)))
            .child(div().text_size(px(11.)).text_color(p.muted).child("Disable and Uninstall remove these from KERN only. The external CLI, your sign-in and chat history are kept."))
            .into_any_element()
    }

    pub fn extensions_settings_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = pal(cx);
        CATALOG
            .iter()
            .enumerate()
            .map(|(i, def)| {
                let id = def.id;
                let chips = self.state_chips(def, cx);
                div()
                    .id(("ext-set", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(div().w(px(120.)).text_size(px(13.)).child(def.name))
                    .child(div().flex_1().child(chips))
                    .child(
                        button(("ext-open", i), "Open", p).on_click(
                            cx.listener(move |t, _, w, cx| t.open_extension_page(id, w, cx)),
                        ),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

/// Ask the official client. Tokens are never read: only the reported state.
fn sign_in_state(id: &str, cwd: &std::path::Path) -> ExtensionFacts {
    use crate::platform::tools;
    tools::wait_for_environment();
    match id {
        "claude" => {
            if tools::find("claude").is_none() {
                return ExtensionFacts::default();
            }
            let Ok(out) = tools::command("claude").args(["auth", "status"]).output() else {
                return ExtensionFacts {
                    detail: "Could not ask claude for its sign-in state.".into(),
                    ..Default::default()
                };
            };
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            ExtensionFacts {
                signed_in: v.get("loggedIn").and_then(|l| l.as_bool()),
                detail: if v.get("loggedIn").and_then(|l| l.as_bool()) == Some(false) {
                    "Run claude and sign in there: Claude Code handles its own login.".into()
                } else {
                    String::new()
                },
                checking: false,
            }
        }
        "codex" => {
            if tools::find("codex").is_none() {
                return ExtensionFacts::default();
            }
            let (tx, _rx) = futures::channel::mpsc::unbounded();
            match crate::agents::codex::CodexClient::start(cwd, tx) {
                Ok(client) => {
                    let call =
                        client.call("account/read", serde_json::json!({"refreshToken": false}));
                    match futures::executor::block_on(call) {
                        Ok(res) => match crate::agents::protocol::parse_account(&res) {
                            crate::agents::protocol::AccountState::Connected { description } => {
                                ExtensionFacts {
                                    signed_in: Some(true),
                                    detail: description,
                                    checking: false,
                                }
                            }
                            crate::agents::protocol::AccountState::LoginRequired => {
                                ExtensionFacts {
                                    signed_in: Some(false),
                                    detail: "Use Sign In with ChatGPT in the Agents panel.".into(),
                                    checking: false,
                                }
                            }
                        },
                        Err(e) => ExtensionFacts {
                            detail: e.message,
                            ..Default::default()
                        },
                    }
                }
                Err(e) => ExtensionFacts {
                    detail: format!("{e:#}"),
                    ..Default::default()
                },
            }
        }
        _ => ExtensionFacts::default(),
    }
}
