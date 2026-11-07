//! Modal surfaces: quick open, command palette, prompts, confirmations, GitHub.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use super::*;
use crate::github::{self, AuthState, Repo};
use crate::workspace::search;
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq)]
pub enum ConfirmAction {
    SaveAndClose(usize),
    DiscardAndClose(usize),
    SaveAllAndQuit,
    DiscardAndQuit,
    ExtensionOff {
        id: String,
        uninstall: bool,
    },
    AttachSensitive {
        agent: agents_panel::AgentTab,
        path: std::path::PathBuf,
    },
}

#[derive(Clone)]
pub struct ConfirmChoice {
    pub label: String,
    pub action: ConfirmAction,
    pub primary: bool,
}

impl ConfirmChoice {
    pub fn new(label: &str, action: ConfirmAction, primary: bool) -> Self {
        Self {
            label: label.into(),
            action,
            primary,
        }
    }
}

#[derive(Clone)]
pub enum PromptKind {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    CloneUrl,
    NewBranch,
    RenameSession { agent: String, id: String },
}

#[derive(Clone, Copy)]
pub enum Command {
    OpenFolder,
    NewFile,
    Save,
    CloseTab,
    ToggleSidebar,
    ToggleBottom,
    ToggleCodex,
    ToggleTheme,
    ToggleWordWrap,
    ChangeLanguage,
    Extensions,
    CodexNewChat,
    ClaudeEnd,
    NewTerminal,
    OpenClaude,
    Run,
    Stop,
    SearchProject,
    SourceControl,
    Settings,
    CloneRepo,
    CloneGithub,
    GitPull,
    GitPush,
    OpenPr,
    AttachFile,
    AttachSelection,
    TrustFolder,
    GoToDefinition,
    ReviewDiff,
    ToggleTerminal,
    KillTerminal,
}

const COMMANDS: &[(&str, &str, Command)] = &[
    ("Codex: New Chat", "", Command::CodexNewChat),
    ("View: Toggle Word Wrap", "⌥Z", Command::ToggleWordWrap),
    ("Editor: Change Language Mode", "", Command::ChangeLanguage),
    ("Extensions: Show", "⇧⌘X", Command::Extensions),
    ("File: Open Folder…", "⌘O", Command::OpenFolder),
    ("File: New File", "⌘N", Command::NewFile),
    ("File: Save", "⌘S", Command::Save),
    ("File: Close Tab", "⌘W", Command::CloseTab),
    ("Editor: Go to Definition", "F12", Command::GoToDefinition),
    ("View: Toggle Sidebar", "⌘B", Command::ToggleSidebar),
    ("View: Toggle Bottom Panel", "⌘J", Command::ToggleBottom),
    ("View: Toggle Codex Panel", "⌥⌘C", Command::ToggleCodex),
    (
        "View: Toggle Light/Graphite Theme",
        "",
        Command::ToggleTheme,
    ),
    ("Search: Find in Project", "⇧⌘F", Command::SearchProject),
    ("Terminal: Toggle Terminal", "⌃`", Command::ToggleTerminal),
    ("Terminal: Kill Terminal", "", Command::KillTerminal),
    ("Terminal: New Terminal", "", Command::NewTerminal),
    ("Claude Code: Open", "", Command::OpenClaude),
    ("Claude Code: End Session", "", Command::ClaudeEnd),
    ("Run: Start", "⌘R", Command::Run),
    ("Run: Stop", "⌘.", Command::Stop),
    ("Git: Source Control", "", Command::SourceControl),
    ("Git: Clone Repository…", "", Command::CloneRepo),
    ("GitHub: Clone from GitHub…", "", Command::CloneGithub),
    ("Git: Pull", "", Command::GitPull),
    ("Git: Push", "", Command::GitPush),
    ("GitHub: Open Pull Request", "", Command::OpenPr),
    ("Codex: Attach Current File", "", Command::AttachFile),
    ("Codex: Attach Selection", "", Command::AttachSelection),
    ("Codex: Review Diff", "", Command::ReviewDiff),
    ("Project: Trust This Folder", "", Command::TrustFolder),
    ("Preferences: Settings", "⌘,", Command::Settings),
];

pub enum GithubView {
    Checking,
    CliMissing,
    SignedOut,
    Loading(String),
    Repos { login: String, repos: Vec<Repo> },
    Error(String),
}

pub enum Overlay {
    Language {
        input: Entity<InputState>,
        selected: usize,
        remember: bool,
        _sub: Subscription,
    },
    QuickOpen {
        input: Entity<InputState>,
        selected: usize,
        _sub: Subscription,
    },
    Palette {
        input: Entity<InputState>,
        selected: usize,
        _sub: Subscription,
    },
    Prompt {
        title: String,
        description: String,
        input: Entity<InputState>,
        kind: PromptKind,
        _sub: Subscription,
    },
    Confirm {
        title: String,
        body: String,
        choices: Vec<ConfirmChoice>,
        focused: bool,
    },
    Github {
        view: GithubView,
        filter: Entity<InputState>,
        _sub: Subscription,
    },
}

impl Overlay {
    pub fn confirm(title: &str, body: impl Into<String>, choices: Vec<ConfirmChoice>) -> Self {
        Overlay::Confirm {
            title: title.to_string(),
            body: body.into(),
            choices,
            focused: false,
        }
    }

    pub fn prompt(
        title: &str,
        description: impl Into<String>,
        initial: &str,
        kind: PromptKind,
        window: &mut Window,
        cx: &mut Context<KernApp>,
    ) -> Self {
        let init = initial.to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(init));
        input.update(cx, |i, cx| i.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut KernApp, _, ev: &InputEvent, w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.submit_prompt(w, cx);
                }
            },
        );
        Overlay::Prompt {
            title: title.to_string(),
            description: description.into(),
            input,
            kind,
            _sub: sub,
        }
    }
}

impl KernApp {
    /// True once per confirmation: it has no input, so nothing took focus when it
    /// opened, and keys would still reach whatever lies behind the scrim.
    fn overlay_needs_focus(&mut self) -> bool {
        match &mut self.overlay {
            Some(Overlay::Confirm { focused, .. }) => !std::mem::replace(focused, true),
            _ => false,
        }
    }

    pub fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        if let Some(previous) = self.overlay_return.take() {
            window.focus(&previous, cx);
        } else if let Some(f) = self.active_file() {
            let e = f.editor.clone();
            e.update(cx, |e, cx| e.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    pub fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        self.refresh_file_index(cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to file"));
        input.update(cx, |i, cx| i.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut KernApp, _, ev: &InputEvent, w, cx| match ev {
                InputEvent::Change => {
                    if let Some(Overlay::QuickOpen { selected, .. }) = &mut this.overlay {
                        *selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.accept_overlay(w, cx),
                _ => {}
            },
        );
        self.overlay = Some(Overlay::QuickOpen {
            input,
            selected: 0,
            _sub: sub,
        });
        cx.notify();
    }

    pub fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command"));
        input.update(cx, |i, cx| i.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut KernApp, _, ev: &InputEvent, w, cx| match ev {
                InputEvent::Change => {
                    if let Some(Overlay::Palette { selected, .. }) = &mut this.overlay {
                        *selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.accept_overlay(w, cx),
                _ => {}
            },
        );
        self.overlay = Some(Overlay::Palette {
            input,
            selected: 0,
            _sub: sub,
        });
        cx.notify();
    }

    pub fn open_language_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_file().is_none() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Select language mode"));
        input.update(cx, |i, cx| i.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut KernApp, _, ev: &InputEvent, w, cx| match ev {
                InputEvent::Change => {
                    if let Some(Overlay::Language { selected, .. }) = &mut this.overlay {
                        *selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.accept_overlay(w, cx),
                _ => {}
            },
        );
        self.overlay = Some(Overlay::Language {
            input,
            selected: 0,
            remember: false,
            _sub: sub,
        });
        cx.notify();
    }

    fn language_matches(query: &str) -> Vec<crate::lang::Language> {
        let mut all: Vec<crate::lang::Language> = crate::lang::LANGUAGES.to_vec();
        all.push(crate::lang::PLAIN);
        all.retain(|l| {
            search::fuzzy_score(query, l.display).is_some()
                || search::fuzzy_score(query, l.id).is_some()
        });
        all
    }

    fn quick_open_matches(&self, query: &str) -> Vec<PathBuf> {
        let Some(p) = &self.project else {
            return vec![];
        };
        search::fuzzy_filter(query, &p.file_index, 50)
    }

    #[cfg(feature = "autotest")]
    pub fn palette_has(&self, name: &str) -> bool {
        COMMANDS
            .iter()
            .any(|(n, _, c)| n.contains(name) && self.command_available(*c))
    }

    /// A disabled extension contributes no commands.
    fn command_available(&self, c: Command) -> bool {
        let codex = self.settings.extension_enabled("codex");
        let claude = self.settings.extension_enabled("claude");
        match c {
            Command::ToggleCodex | Command::CodexNewChat => codex,
            Command::AttachFile | Command::AttachSelection | Command::ReviewDiff => codex,
            Command::OpenClaude | Command::ClaudeEnd => claude,
            _ => true,
        }
    }

    fn palette_matches(&self, query: &str) -> Vec<usize> {
        let mut scored: Vec<(i64, usize)> = COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, (_, _, c))| self.command_available(*c))
            .filter_map(|(i, (name, _, _))| search::fuzzy_score(query, name).map(|s| (s, i)))
            .collect();
        if !query.is_empty() {
            scored.sort_by_key(|(s, _)| -*s);
        }
        scored.into_iter().map(|(_, i)| i).collect()
    }

    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let len = match &self.overlay {
            Some(Overlay::QuickOpen { input, .. }) => self
                .quick_open_matches(input.read(cx).value().as_ref())
                .len(),
            Some(Overlay::Palette { input, .. }) => {
                self.palette_matches(input.read(cx).value().as_ref()).len()
            }
            Some(Overlay::Language { input, .. }) => {
                Self::language_matches(input.read(cx).value().as_ref()).len()
            }
            _ => 0,
        };
        if len == 0 {
            return;
        }
        if let Some(
            Overlay::QuickOpen { selected, .. }
            | Overlay::Palette { selected, .. }
            | Overlay::Language { selected, .. },
        ) = &mut self.overlay
        {
            *selected = ((*selected as i32 + delta).rem_euclid(len as i32)) as usize;
        }
        cx.notify();
    }

    pub fn accept_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.overlay {
            Some(Overlay::Language {
                input,
                selected,
                remember,
                ..
            }) => {
                let q = input.read(cx).value().to_string();
                let picked = Self::language_matches(&q).get(*selected).copied();
                let remember = *remember;
                self.close_overlay(window, cx);
                if let Some(l) = picked {
                    self.set_file_language(l, remember, window, cx);
                }
            }
            Some(Overlay::QuickOpen {
                input, selected, ..
            }) => {
                let q = input.read(cx).value().to_string();
                let picked = self.quick_open_matches(&q).get(*selected).cloned();
                let root = self.project.as_ref().map(|p| p.root.clone());
                self.overlay = None;
                if let (Some(rel), Some(root)) = (picked, root) {
                    self.open_file_async(root.join(rel), true, None, window, cx);
                }
                cx.notify();
            }
            Some(Overlay::Palette {
                input, selected, ..
            }) => {
                let q = input.read(cx).value().to_string();
                let picked = self.palette_matches(&q).get(*selected).copied();
                self.overlay = None;
                if let Some(ix) = picked {
                    self.run_command(COMMANDS[ix].2, window, cx);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn run_command(&mut self, c: Command, window: &mut Window, cx: &mut Context<Self>) {
        let has_project = self.project.is_some();
        match c {
            Command::OpenFolder => self.pick_folder(window, cx),
            Command::CloneRepo => self.prompt_clone_url(window, cx),
            Command::CloneGithub => self.open_github_picker(window, cx),
            Command::ToggleTheme => self.toggle_theme(cx),
            Command::Settings => self.open_settings_tab(window, cx),
            Command::ToggleWordWrap => self.toggle_word_wrap(window, cx),
            _ if !has_project => self.toast("Open a folder first", false, cx),
            Command::ChangeLanguage => self.open_language_picker(window, cx),
            Command::Extensions => {
                self.side = SideView::SourceControl;
                self.show_side(SideView::Extensions, window, cx);
            }
            Command::CodexNewChat => self.codex_new_chat(cx),
            Command::ClaudeEnd => self.claude_end(window, cx),
            Command::NewFile => self.prompt_new_file(window, cx),
            Command::Save => self.save_active(false, window, cx),
            Command::CloseTab => self.close_active_tab(window, cx),
            Command::ToggleSidebar => self.toggle_panel(Dragging::Sidebar, window, cx),
            Command::ToggleBottom => self.toggle_panel(Dragging::Bottom, window, cx),
            Command::ToggleCodex => self.toggle_panel(Dragging::Codex, window, cx),
            Command::NewTerminal => self.new_terminal(window, cx),
            Command::ToggleTerminal => self.toggle_terminal(window, cx),
            Command::KillTerminal => self.kill_terminal(window, cx),
            Command::OpenClaude => self.open_claude(window, cx),
            Command::Run => self.run_selected(window, cx),
            Command::Stop => self.stop_run(cx),
            Command::SearchProject => {
                self.side = SideView::Search;
                self.settings.layout.sidebar_visible = true;
                if let Some(p) = &self.project {
                    p.search.input.update(cx, |i, cx| i.focus(window, cx));
                }
            }
            Command::SourceControl => {
                self.side = SideView::SourceControl;
                self.settings.layout.sidebar_visible = true;
                self.refresh_git(cx);
            }
            Command::GitPull => self.git_pull(cx),
            Command::GitPush => self.git_push(cx),
            Command::OpenPr => self.open_pull_request(cx),
            Command::AttachFile => self.agent_attach_active_file(self.agent_tab, cx),
            Command::AttachSelection => self.agent_attach_selection(self.agent_tab, cx),
            Command::TrustFolder => self.trust_project(window, cx),
            Command::GoToDefinition => self.go_to_definition(window, cx),
            Command::ReviewDiff => self.agent_open_review(self.agent_tab, window, cx),
        }
        cx.notify();
    }

    pub fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Prompt { input, kind, .. }) = &self.overlay else {
            return;
        };
        let value = input.read(cx).value().trim().to_string();
        let kind = kind.clone();
        if value.is_empty() {
            return;
        }
        self.overlay = None;
        match kind {
            PromptKind::NewFile(dir) => match crate::workspace::tree::create_file(&dir, &value) {
                Ok(path) => {
                    self.refresh_tree_dir(&dir, cx);
                    self.open_file_async(path, true, None, window, cx);
                }
                Err(e) => self.toast(e.to_string(), true, cx),
            },
            PromptKind::NewFolder(dir) => match crate::workspace::tree::create_dir(&dir, &value) {
                Ok(_) => self.refresh_tree_dir(&dir, cx),
                Err(e) => self.toast(e.to_string(), true, cx),
            },
            PromptKind::Rename(old) => match crate::workspace::tree::rename(&old, &value) {
                Ok(new) => {
                    self.after_rename(&old, &new, cx);
                    if let Some(parent) = old.parent() {
                        self.refresh_tree_dir(parent, cx);
                    }
                }
                Err(e) => self.toast(e.to_string(), true, cx),
            },
            PromptKind::CloneUrl => self.clone_with_destination(value, window, cx),
            PromptKind::NewBranch => self.git_switch(value, true, cx),
            PromptKind::RenameSession { agent, id } => self.rename_session(&agent, &id, &value, cx),
        }
        cx.notify();
    }

    fn apply_confirm(
        &mut self,
        action: ConfirmAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay = None;
        if let Some(previous) = self.overlay_return.take() {
            window.focus(&previous, cx);
        }
        match action {
            ConfirmAction::SaveAndClose(ix) => {
                if self.save_tab(ix, false, cx) {
                    self.force_close_tab(ix, window, cx);
                }
            }
            ConfirmAction::DiscardAndClose(ix) => self.force_close_tab(ix, window, cx),
            ConfirmAction::SaveAllAndQuit => {
                if self.save_all(cx) {
                    self.shutdown_processes();
                    self.persist();
                    cx.quit();
                }
            }
            ConfirmAction::AttachSensitive { agent, path } => {
                self.agent_attach_path_confirmed(agent, path, cx)
            }
            ConfirmAction::ExtensionOff { id, uninstall } => {
                self.force_extension_off(&id, uninstall, window, cx)
            }
            ConfirmAction::DiscardAndQuit => {
                self.shutdown_processes();
                self.persist();
                cx.quit();
            }
        }
        cx.notify();
    }

    pub fn open_github_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter repositories"));
        let sub = cx.subscribe_in(
            &filter,
            window,
            |_: &mut KernApp, _, _: &InputEvent, _, cx| cx.notify(),
        );
        self.overlay = Some(Overlay::Github {
            view: GithubView::Checking,
            filter,
            _sub: sub,
        });
        self.refresh_github(cx);
        cx.notify();
    }

    pub fn refresh_github(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::platform::tools::wait_for_environment();
                    match github::auth_state() {
                        AuthState::CliMissing => GithubView::CliMissing,
                        AuthState::SignedOut => GithubView::SignedOut,
                        AuthState::SignedIn => {
                            let login = github::account_login().unwrap_or_default();
                            match github::list_repos() {
                                Ok(repos) => GithubView::Repos { login, repos },
                                Err(e) => GithubView::Error(e.to_string()),
                            }
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(Overlay::Github { view, .. }) = &mut this.overlay {
                    *view = result;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Clone a selected repository through the local Git path. `gh` credentials are
    /// used by Git via the credential helper that `gh auth setup-git` configures.
    fn clone_github_repo(&mut self, repo: &Repo, window: &mut Window, cx: &mut Context<Self>) {
        let url = format!("https://github.com/{}.git", repo.name_with_owner);
        self.overlay = None;
        self.clone_with_destination(url, window, cx);
    }

    /// The official browser flow in a real PTY, so `gh` shows its one-time code.
    fn github_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gh) = crate::platform::tools::find("gh") else {
            return;
        };
        let args: Vec<String> = github::LOGIN_ARGS.iter().map(|s| s.to_string()).collect();
        let Some(cwd) = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(dirs::home_dir)
        else {
            return;
        };
        match self.login_terminal(gh, args, cwd, window, cx) {
            Ok(view) => {
                if let Some(Overlay::Github { view: v, .. }) = &mut self.overlay {
                    *v = GithubView::Loading(
                        "Finish signing in below, then press Refresh.".to_string(),
                    );
                }
                self.login_view = Some(view);
            }
            Err(e) => self.toast(e, true, cx),
        }
        cx.notify();
    }

    pub fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let palette = pal(cx);
        self.overlay.as_ref()?;
        let content: AnyElement = match self.overlay.as_ref()? {
            Overlay::Language {
                input,
                selected,
                remember,
                ..
            } => {
                let q = input.read(cx).value().to_string();
                let matches = Self::language_matches(&q);
                let (selected, remember, input) = (*selected, *remember, input.clone());
                let current = self.active_file().map(|f| f.lang.id);
                let key = self
                    .active_file()
                    .map(|f| crate::lang::association_key(&f.path))
                    .unwrap_or_default();
                panel(palette)
                    .child(div().p_2().child(Input::new(&input)))
                    .child(
                        div()
                            .id("lang-remember")
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(26.))
                            .px_3()
                            .text_size(px(11.))
                            .text_color(if remember {
                                palette.accent
                            } else {
                                palette.muted
                            })
                            .cursor_pointer()
                            .hover(move |s| s.bg(palette.hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(Overlay::Language { remember, .. }) = &mut this.overlay
                                {
                                    *remember = !*remember;
                                }
                                cx.notify();
                            }))
                            .child(if remember {
                                format!("✓ Remember for {key}")
                            } else {
                                format!("Only this file. Click to remember for {key}")
                            }),
                    )
                    .child(hsep(palette))
                    .child(
                        div()
                            .id("lang-list")
                            .max_h(px(380.))
                            .overflow_y_scroll()
                            .py_1()
                            .children(matches.into_iter().enumerate().map(|(i, l)| {
                                row_item(("lang", i), i == selected, palette)
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.close_overlay(w, cx);
                                        this.set_file_language(l, remember, w, cx);
                                    }))
                                    .child(div().flex_1().text_size(px(13.)).child(l.display))
                                    .when(current == Some(l.id), |d| {
                                        d.child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(palette.accent)
                                                .child("current"),
                                        )
                                    })
                            })),
                    )
                    .into_any_element()
            }
            Overlay::QuickOpen {
                input, selected, ..
            } => {
                let q = input.read(cx).value().to_string();
                let matches = self.quick_open_matches(&q);
                let selected = *selected;
                let input = input.clone();
                let root: PathBuf = self
                    .project
                    .as_ref()
                    .map(|p| p.root.clone())
                    .unwrap_or_default();
                list_box(
                    &input,
                    palette,
                    matches.iter().take(14).enumerate().map(|(i, rel)| {
                        let path = root.join(rel);
                        let name = rel
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let dir = rel
                            .parent()
                            .map(Path::display)
                            .map(|d| d.to_string())
                            .unwrap_or_default();
                        row_item(("qo", i), i == selected, palette)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.overlay = None;
                                this.open_file_async(path.clone(), true, None, w, cx);
                            }))
                            .child(div().text_size(px(13.)).child(name))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.muted)
                                    .child(dir),
                            )
                            .into_any_element()
                    }),
                )
            }
            Overlay::Palette {
                input, selected, ..
            } => {
                let q = input.read(cx).value().to_string();
                let matches = self.palette_matches(&q);
                let selected = *selected;
                let input = input.clone();
                list_box(
                    &input,
                    palette,
                    matches.iter().take(14).enumerate().map(|(i, &cmd)| {
                        let (name, key, command) = COMMANDS[cmd];
                        row_item(("cmd", i), i == selected, palette)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.overlay = None;
                                this.run_command(command, w, cx);
                            }))
                            .child(div().flex_1().text_size(px(13.)).child(name))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.muted)
                                    .child(shortcut(key)),
                            )
                            .into_any_element()
                    }),
                )
            }
            Overlay::Prompt {
                title,
                description,
                input,
                ..
            } => panel(palette)
                .p_3()
                .gap_2()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child(description.clone()),
                )
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_1()
                        .child(
                            button("pr-cancel", "Cancel", palette)
                                .on_click(cx.listener(|this, _, w, cx| this.close_overlay(w, cx))),
                        )
                        .child(
                            primary_button("pr-ok", "OK", palette)
                                .on_click(cx.listener(|this, _, w, cx| this.submit_prompt(w, cx))),
                        ),
                )
                .into_any_element(),
            Overlay::Confirm {
                title,
                body,
                choices,
                ..
            } => {
                let buttons: Vec<AnyElement> = choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let action = c.action.clone();
                        (if c.primary {
                            primary_button(("cf", i), c.label.clone(), palette)
                        } else {
                            button(("cf", i), c.label.clone(), palette)
                        })
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.apply_confirm(action.clone(), w, cx)
                        }))
                        .into_any_element()
                    })
                    .collect();
                panel(palette)
                    .p_3()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.clone()),
                    )
                    .child(div().text_size(px(12.)).child(body.clone()))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_1()
                            .child(
                                button(
                                    "cf-cancel",
                                    if choices.is_empty() { "OK" } else { "Cancel" },
                                    palette,
                                )
                                .on_click(cx.listener(|this, _, w, cx| this.close_overlay(w, cx))),
                            )
                            .children(buttons),
                    )
                    .into_any_element()
            }
            Overlay::Github { view, filter, .. } => {
                self.render_github(view, filter.clone(), window, cx)
            }
        };
        let trap = self.overlay_focus.clone();
        if self.overlay_needs_focus() {
            self.overlay_return = window.focused(cx);
            window.focus(&trap, cx);
            cx.notify();
        }
        Some(
            div()
                .id("overlay-scrim")
                .key_context("KernOverlay")
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(70.))
                .bg(hsla(0., 0., 0., 0.28))
                .occlude()
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                    match ev.keystroke.key.as_str() {
                        "down" => {
                            this.move_selection(1, cx);
                            cx.stop_propagation();
                        }
                        "up" => {
                            this.move_selection(-1, cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            this.close_overlay(w, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .on_action(cx.listener(|this, _: &Dismiss, w, cx| this.close_overlay(w, cx)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| this.close_overlay(w, cx)),
                )
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(content),
                )
                .focus_trap("kern-overlay", &trap)
                .into_any_element(),
        )
    }

    fn render_github(
        &self,
        view: &GithubView,
        filter: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = pal(cx);
        let _ = window;
        let header = div()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .child("Clone from GitHub");
        let body: AnyElement = match view {
            GithubView::Checking => div().text_size(px(12.)).text_color(palette.muted).child("Checking GitHub connection…").into_any_element(),
            GithubView::CliMissing => div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .child("GitHub CLI is not installed. Install it, then press Refresh:")
                .child(div().font_family("Menlo").text_color(palette.accent).child("brew install gh"))
                .child(div().text_color(palette.muted).child("Clone Repository works without GitHub."))
                .into_any_element(),
            GithubView::SignedOut => div()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(12.))
                .child("Connect through the official GitHub CLI. It opens your browser, and the login is kept in the system keychain.")
                .child(div().flex().child(primary_button("gh-login", "Sign In with Browser", palette).on_click(cx.listener(|this, _, w, cx| this.github_login(w, cx)))))
                .into_any_element(),
            GithubView::Loading(msg) => {
                let term = self.login_view.clone();
                div().flex().flex_col().gap_1().text_size(px(12.)).text_color(palette.muted).child(msg.clone()).children(term.map(|t| div().h(px(220.)).w_full().child(t))).into_any_element()
            }
            GithubView::Error(msg) => div().text_size(px(12.)).text_color(palette.danger).child(msg.clone()).into_any_element(),
            GithubView::Repos { login, repos } => {
                let q = filter.read(cx).value().to_string();
                let matched: Vec<Repo> = repos.iter().filter(|r| search::fuzzy_score(&q, &r.name_with_owner).is_some()).take(60).cloned().collect();
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(11.)).text_color(palette.muted).child(format!("Signed in as {login} via GitHub CLI")))
                    .child(Input::new(&filter))
                    .child(div().id("gh-list").max_h(px(320.)).overflow_y_scroll().children(matched.into_iter().enumerate().map(|(i, r)| {
                        let repo = r.clone();
                        row_item(("gh", i), false, palette)
                            .on_click(cx.listener(move |this, _, w, cx| this.clone_github_repo(&repo, w, cx)))
                            .child(div().flex_1().text_size(px(12.)).child(r.name_with_owner.clone()))
                            .when(r.is_private, |d| d.child(div().text_size(px(10.)).text_color(palette.warning).child("private")))
                    })))
                    .into_any_element()
            }
        };
        panel(palette)
            .p_3()
            .gap_2()
            .child(header)
            .child(body)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_1()
                    .child(
                        button("gh-refresh", "Refresh", palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.login_view = None;
                                if let Some(Overlay::Github { view, .. }) = &mut this.overlay {
                                    *view = GithubView::Checking;
                                }
                                this.refresh_github(cx);
                            },
                        )),
                    )
                    .child(button("gh-close", "Close", palette).on_click(cx.listener(
                        |this, _, w, cx| {
                            this.login_view = None;
                            this.close_overlay(w, cx);
                        },
                    ))),
            )
            .into_any_element()
    }
}

fn panel(p: super::theme::Palette) -> Div {
    div()
        .w(px(560.))
        .flex()
        .flex_col()
        .rounded(px(8.))
        .bg(p.bar)
        .border_1()
        .border_color(p.border)
        .shadow_lg()
}

fn row_item(id: impl Into<ElementId>, selected: bool, p: super::theme::Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .h(px(28.))
        .px_3()
        .when(selected, |d| d.bg(p.accent_soft))
        .hover(move |s| s.bg(p.hover))
        .cursor_pointer()
}

fn list_box(
    input: &Entity<InputState>,
    p: super::theme::Palette,
    rows: impl Iterator<Item = AnyElement>,
) -> AnyElement {
    panel(p)
        .child(div().p_2().child(Input::new(input)))
        .child(hsep(p))
        .child(div().py_1().children(rows))
        .into_any_element()
}
