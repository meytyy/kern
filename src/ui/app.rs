//! Application state, root layout, persistence.
use super::widgets::*;
use super::*;
use crate::core::settings::{Settings, ThemeChoice};
use crate::git::RepoStatus;
use crate::lsp::{LspClient, ServerStatus};
use crate::workspace::watch::ProjectWatcher;

use gpui_kit::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use theme::pal;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SideView {
    Files,
    Search,
    SourceControl,
    Extensions,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BottomTab {
    Terminal,
    RunOutput,
    Claude,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dragging {
    Sidebar,
    Bottom,
    Codex,
    Graph,
}

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub trusted: bool,
    pub trust_prompt: bool,
    pub tabs: Vec<editor_area::Tab>,
    pub active: Option<usize>,
    pub tree: files::TreeState,
    pub file_index: Arc<Vec<PathBuf>>,
    pub search: search_panel::SearchState,
    pub git: scm::GitState,
    pub run: bottom::RunState,
    pub terminals: Vec<Entity<terminal_view::TerminalView>>,
    pub active_terminal: usize,
    /// The project's Claude Code session in the bottom panel.
    pub claude_code: Option<Entity<terminal_view::TerminalView>>,
    pub claude_message: Option<String>,
    pub bottom_tab: BottomTab,
    /// The panel has a height of its own; otherwise it takes 20% when first shown.
    pub bottom_height_set: bool,
    pub lsp_clients: HashMap<(String, PathBuf), Arc<LspClient>>,
    pub lsp_status: BTreeMap<String, ServerStatus>,
    pub codex: codex_panel::CodexState,
    pub _watcher: Option<ProjectWatcher>,
}

pub struct KernApp {
    pub settings: Settings,
    pub focus: FocusHandle,
    pub project: Option<Project>,
    pub side: SideView,
    pub dragging: Option<Dragging>,
    pub overlay: Option<overlays::Overlay>,
    pub overlay_focus: FocusHandle,
    pub overlay_return: Option<FocusHandle>,
    pub toast: Option<(String, bool)>,
    pub toast_gen: u64,
    pub clone_progress: Option<String>,
    pub settings_form: settings_panel::SettingsForm,
    pub _subs: Vec<Subscription>,
    pub settings_standalone: bool,
    pub sessions: crate::agents::sessions::SessionIndex,
    pub extension_facts: std::collections::BTreeMap<String, extensions::ExtensionFacts>,
    pub extension_query: Entity<gpui_kit::component::input::InputState>,
    pub agents_search: Entity<gpui_kit::component::input::InputState>,
    pub agent_tab: agents_panel::AgentTab,
    pub titlebar_drag: bool,
    pub tree_focus: FocusHandle,
    #[cfg(feature = "autotest")]
    pub test_clipboard: Option<ClipboardItem>,
    /// The reasoning slider and its listener, recreated when the levels change.
    pub effort_slider: Option<(
        agents_panel::AgentTab,
        Vec<String>,
        Entity<gpui_kit::component::slider::SliderState>,
        Subscription,
    )>,
    pub login_view: Option<Entity<terminal_view::TerminalView>>,
    pub updates: updates::UpdateState,
    /// The menu bar drawn in the title bar (Windows).
    pub menu_bar: Option<Entity<gpui_kit::component::menu::AppMenuBar>>,
    pub claude_key_input: Entity<gpui_kit::component::input::InputState>,
}

impl KernApp {
    pub fn new(
        settings: Settings,
        initial_project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let bounds_sub = cx.observe_window_bounds(window, |this, window, cx| {
            let size = window.bounds().size;
            this.settings.layout.window_width = f32::from(size.width);
            this.settings.layout.window_height = f32::from(size.height);
            cx.notify();
        });
        let settings_form = settings_panel::SettingsForm::new(&settings, window, cx);
        let extension_query = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx).placeholder("Search extensions")
        });
        let agents_search = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx).placeholder("Search chats")
        });
        let claude_key_input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .masked(true)
                .placeholder("Anthropic API key (sk-ant-…)")
        });
        let search_sub = cx.subscribe(
            &agents_search,
            |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
        );
        let ext_sub = cx.subscribe(
            &extension_query,
            |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
        );
        let appearance_sub = cx.observe_window_appearance(window, |this, _, cx| {
            if this.settings.theme == ThemeChoice::System {
                theme::apply(&this.settings, cx);
                cx.refresh_windows();
                cx.notify();
            }
        });
        let mut app = Self {
            settings,
            focus,
            project: None,
            side: SideView::Files,
            dragging: None,
            overlay: None,
            overlay_focus: cx.focus_handle(),
            overlay_return: None,
            toast: None,
            toast_gen: 0,
            clone_progress: None,
            settings_form,
            _subs: vec![bounds_sub, appearance_sub, search_sub, ext_sub],
            settings_standalone: false,
            sessions: crate::agents::sessions::SessionIndex::load(),
            extension_facts: Default::default(),
            extension_query,
            agents_search,
            agent_tab: agents_panel::AgentTab::Codex,
            titlebar_drag: false,
            tree_focus: cx.focus_handle(),
            #[cfg(feature = "autotest")]
            test_clipboard: None,
            effort_slider: None,
            login_view: None,
            updates: Default::default(),
            menu_bar: in_window_menu().then(|| gpui_kit::component::menu::AppMenuBar::new(cx)),
            claude_key_input,
        };
        window.on_window_should_close(cx, {
            let this = cx.entity().downgrade();
            move |window, cx| {
                this.update(cx, |app, cx| app.request_quit(window, cx, false))
                    .unwrap_or(true)
            }
        });
        cx.on_app_quit(|this, _cx| {
            this.shutdown_processes();
            this.persist();
            async {}
        })
        .detach();
        if let Some(path) = initial_project {
            app.open_project(path, window, cx);
        }
        #[cfg(feature = "autotest")]
        autotest::maybe_start(window, cx);
        app.schedule_update_check(cx);
        // After login env capture finishes, tool statuses may have changed.
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async { crate::platform::tools::wait_for_environment() })
                .await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        app
    }

    pub fn toast(&mut self, message: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.toast = Some((message.into(), error));
        self.toast_gen += 1;
        let generation = self.toast_gen;
        let delay = if error { 9 } else { 4 };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(delay))
                .await;
            this.update(cx, |this, cx| {
                if this.toast_gen == generation {
                    this.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn shutdown_processes(&mut self) {
        if let Some(p) = &self.project
            && let Some(proc) = &p.run.process
        {
            proc.stop_blocking();
        }
    }

    pub fn persist(&mut self) {
        if let Some(p) = &self.project {
            let open: Vec<PathBuf> = p
                .tabs
                .iter()
                .filter_map(|t| t.file_path().map(Path::to_path_buf))
                .collect();
            let active = p
                .active
                .and_then(|i| p.tabs.get(i))
                .and_then(|t| t.file_path().map(Path::to_path_buf));
            let run_config = p.run.selected.clone();
            let root = p.root.clone();
            let height_set = p.bottom_height_set;
            let (bottom_visible, bottom_height) = (
                self.settings.layout.bottom_visible,
                self.settings.layout.bottom_height,
            );
            let session = self.settings.session_mut(&root);
            session.bottom_visible = Some(bottom_visible);
            session.bottom_height = height_set.then_some(bottom_height);
            session.open_files = open;
            session.active_file = active;
            session.run_config = run_config;
        }
        self.settings.save();
    }

    /// Open a folder in this window, after dealing with unsaved work.
    pub fn open_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Plain, not verbatim, on Windows: the Recycle Bin, Explorer and the file
        // watcher all use plain paths.
        let root = crate::platform::simplify_path(&std::fs::canonicalize(&path).unwrap_or(path));
        self.shutdown_processes();
        self.persist();
        self.settings.add_recent(&root);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.display().to_string());
        window.set_window_title(&format!("{name} — KERN"));
        let trusted = self.settings.trusted_projects.contains(&root);
        let panel = self.settings.session(&root);
        self.settings.layout.bottom_visible = panel.bottom_visible.unwrap_or(false);
        if let Some(h) = panel.bottom_height {
            self.settings.layout.bottom_height = h;
        }
        let mut project = Project {
            root: root.clone(),
            name,
            trusted,
            trust_prompt: !trusted,
            tabs: vec![],
            active: None,
            tree: files::TreeState::default(),
            file_index: Arc::new(vec![]),
            search: search_panel::SearchState::new(window, cx),
            git: scm::GitState::new(window, cx),
            run: bottom::RunState::default(),
            terminals: vec![],
            active_terminal: 0,
            claude_code: None,
            claude_message: None,
            bottom_tab: BottomTab::Terminal,
            bottom_height_set: panel.bottom_height.is_some(),
            lsp_clients: HashMap::new(),
            lsp_status: BTreeMap::new(),
            codex: codex_panel::CodexState::new(window, cx),
            _watcher: None,
        };
        project.tree.expanded.insert(root.clone());
        self.project = Some(project);

        // File watching off the UI thread.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
        let watcher = ProjectWatcher::start(&root, move |paths| {
            let _ = tx.unbounded_send(paths);
        });
        match watcher {
            Ok(w) => {
                if let Some(p) = &mut self.project {
                    p._watcher = Some(w);
                }
            }
            Err(e) => self.toast(format!("File watching is unavailable: {e}"), true, cx),
        }
        let watched_root = root.clone();
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while let Some(paths) = rx.next().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    let same = this
                        .project
                        .as_ref()
                        .is_some_and(|p| p.root == watched_root);
                    if same {
                        this.on_fs_changes(paths, window, cx);
                    }
                    same
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        self.refresh_tree_dir(&root, cx);
        self.refresh_file_index(cx);
        self.refresh_git(cx);
        self.refresh_run_configs(cx);

        // Restore tabs of the previous session.
        let session = self.settings.session(&root);
        if let Some(sel) = session.run_config.clone()
            && let Some(p) = &mut self.project
        {
            p.run.selected = Some(sel);
        }
        let active = session.active_file.clone();
        for file in session.open_files {
            if file.is_file() {
                let activate = active.as_ref() == Some(&file);
                self.open_file_async(file, activate, None, window, cx);
            }
        }
        // The previous project's editors are gone with it: without a focused
        // element shortcuts and menu actions would have nowhere to go.
        window.focus(&self.focus, cx);
        self.persist();
        cx.notify();
    }

    fn on_fs_changes(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let git_state_changed = paths
            .iter()
            .any(|x| x.components().any(|c| c.as_os_str() == ".git"));
        let mut dirs: HashSet<PathBuf> = HashSet::new();
        for path in &paths {
            if path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            if let Some(parent) = path.parent()
                && p.tree.children.contains_key(parent)
            {
                dirs.insert(parent.to_path_buf());
            }
            if p.tree.children.contains_key(path) {
                dirs.insert(path.clone());
            }
        }
        let content_changed = paths
            .iter()
            .any(|x| !x.components().any(|c| c.as_os_str() == ".git"));
        for d in dirs {
            self.refresh_tree_dir(&d, cx);
        }
        self.check_open_files(&paths, window, cx);
        if content_changed {
            self.refresh_file_index(cx);
        }
        if content_changed || git_state_changed {
            self.refresh_git(cx);
        }
        if paths.iter().any(|x| {
            matches!(
                x.file_name().and_then(|n| n.to_str()),
                Some(
                    "package.json"
                        | "Cargo.toml"
                        | "pyproject.toml"
                        | "pnpm-lock.yaml"
                        | "yarn.lock"
                        | "package-lock.json"
                )
            )
        }) {
            self.refresh_run_configs(cx);
        }
    }

    pub fn refresh_file_index(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let root = p.root.clone();
        cx.spawn(async move |this, cx| {
            let r = root.clone();
            let files = cx
                .background_spawn(async move {
                    crate::workspace::search::list_files(
                        &r,
                        &std::sync::atomic::AtomicBool::new(false),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(p) = &mut this.project
                    && p.root == root
                {
                    p.file_index = Arc::new(files);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn set_theme(&mut self, choice: ThemeChoice, cx: &mut Context<Self>) {
        self.settings.theme = choice;
        theme::apply(&self.settings, cx);
        cx.refresh_windows();
        self.persist();
        cx.notify();
    }

    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let next = if theme::is_dark(self.settings.theme, cx) {
            ThemeChoice::Light
        } else {
            ThemeChoice::Dark
        };
        self.set_theme(next, cx);
    }

    pub fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    this.request_open_project(path, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Switch to another project unless unsaved files need a decision first.
    pub fn request_open_project(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_dirty_tabs() {
            self.overlay = Some(overlays::Overlay::confirm(
                "Unsaved changes",
                "Save your changes before switching projects, or close those tabs first.",
                vec![],
            ));
            cx.notify();
            return;
        }
        self.open_project(path, window, cx);
    }

    pub fn has_dirty_tabs(&self) -> bool {
        self.project
            .as_ref()
            .is_some_and(|p| p.tabs.iter().any(|t| t.is_dirty()))
    }

    /// Quit / close-window request. Returns whether closing may proceed now.
    pub fn request_quit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        quit: bool,
    ) -> bool {
        let dirty: Vec<String> = self
            .project
            .as_ref()
            .map(|p| {
                p.tabs
                    .iter()
                    .filter(|t| t.is_dirty())
                    .map(|t| t.title())
                    .collect()
            })
            .unwrap_or_default();
        if dirty.is_empty() {
            self.shutdown_processes();
            self.persist();
            if quit {
                cx.quit();
            }
            return true;
        }
        let _ = window;
        self.overlay = Some(overlays::Overlay::confirm(
            "Unsaved changes",
            format!("These files have unsaved changes:\n{}", dirty.join("\n")),
            vec![
                overlays::ConfirmChoice::new(
                    "Save All & Quit",
                    overlays::ConfirmAction::SaveAllAndQuit,
                    true,
                ),
                overlays::ConfirmChoice::new(
                    "Discard & Quit",
                    overlays::ConfirmAction::DiscardAndQuit,
                    false,
                ),
            ],
        ));
        cx.notify();
        false
    }

    pub fn hide_bottom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.layout.bottom_visible = false;
        self.focus_editor(window, cx);
        self.persist();
        cx.notify();
    }

    fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_file().map(|f| f.editor.clone()) {
            Some(editor) => editor.update(cx, |e, cx| e.focus(window, cx)),
            None => window.focus(&self.focus, cx),
        }
    }

    /// Ctrl+`: Terminal showing already? Hide it. Otherwise show and focus it. The
    /// terminal lives in the project, not in the panel, so nothing restarts.
    pub fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        if self.settings.layout.bottom_visible && p.bottom_tab == BottomTab::Terminal {
            self.hide_bottom(window, cx);
            return;
        }
        self.settings.layout.bottom_visible = true;
        self.ensure_terminal(window, cx);
        if let Some(p) = &mut self.project {
            p.bottom_tab = BottomTab::Terminal;
            if let Some(t) = p.terminals.get(p.active_terminal).cloned() {
                t.update(cx, |v, cx| v.focus(window, cx));
            }
        }
        self.persist();
        cx.notify();
    }

    pub fn toggle_panel(&mut self, which: Dragging, window: &mut Window, cx: &mut Context<Self>) {
        match which {
            Dragging::Sidebar => {
                self.settings.layout.sidebar_visible = !self.settings.layout.sidebar_visible
            }
            Dragging::Bottom => {
                self.settings.layout.bottom_visible = !self.settings.layout.bottom_visible;
                if self.settings.layout.bottom_visible {
                    self.ensure_terminal(window, cx);
                }
            }
            Dragging::Graph => self.settings.graph_hidden = !self.settings.graph_hidden,
            Dragging::Codex => {
                self.settings.layout.codex_visible = !self.settings.layout.codex_visible;
                if self.settings.layout.codex_visible {
                    self.agent_connect(self.agent_tab, cx);
                }
            }
        }
        self.persist();
        cx.notify();
    }

    pub fn drag_handle(&self, kind: Dragging, cx: &mut Context<Self>) -> AnyElement {
        self.handle(kind, cx).into_any_element()
    }

    fn handle(&self, kind: Dragging, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let vertical = matches!(kind, Dragging::Sidebar | Dragging::Codex);
        let active = self.dragging == Some(kind);
        let base = div()
            .id(match kind {
                Dragging::Sidebar => "drag-side",
                Dragging::Bottom => "drag-bottom",
                Dragging::Codex => "drag-codex",
                Dragging::Graph => "drag-graph",
            })
            .flex_shrink_0()
            .bg(if active { p.accent_fill } else { p.border })
            .hover(move |s| s.bg(p.accent_fill))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.dragging = Some(kind);
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        if vertical {
            base.w(px(2.)).h_full().cursor_col_resize()
        } else {
            base.h(px(2.)).w_full().cursor_row_resize()
        }
    }

    fn on_drag_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.dragging else { return };
        if !ev.dragging() {
            self.dragging = None;
            self.persist();
            cx.notify();
            return;
        }
        let win = window.viewport_size();
        let l = &mut self.settings.layout;
        let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
        match kind {
            Dragging::Sidebar => {
                l.sidebar_width = (x - 44.).clamp(160., f32::from(win.width) * 0.5)
            }
            Dragging::Codex => {
                l.codex_width = (f32::from(win.width) - x).clamp(260., f32::from(win.width) * 0.6)
            }
            Dragging::Graph => {
                self.settings.graph_height =
                    (f32::from(win.height) - 24. - y).clamp(90., f32::from(win.height) * 0.75)
            }
            Dragging::Bottom => {
                l.bottom_height =
                    (f32::from(win.height) - 24. - y).clamp(80., f32::from(win.height) * 0.75);
                if let Some(p) = &mut self.project {
                    p.bottom_height_set = true;
                }
            }
        }
        cx.notify();
    }

    fn render_topbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        // Native traffic lights occupy this space on macOS, except in full screen.
        let left_inset = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            80.
        } else {
            12.
        };
        let mac = cfg!(target_os = "macos");
        let run_controls = self.project.is_some().then(|| self.render_run_controls(cx));
        let agents_available =
            self.settings.extension_enabled("codex") || self.settings.extension_enabled("claude");
        let proj = self.project.as_ref();
        let name = proj
            .map(|x| x.name.clone())
            .unwrap_or_else(|| "KERN".into());
        let branch = proj.and_then(|x| x.git.status.as_ref()).map(branch_label);
        div()
            .id("topbar")
            .h(px(38.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(left_inset))
            .when(mac, |d| d.pr_2())
            .bg(p.bar)
            .border_b_1()
            .border_color(p.border)
            .when(mac, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, _| {
                        if ev.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            this.titlebar_drag = true;
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.titlebar_drag = false),
                )
                // Only a drag that began on the title bar moves the window: dragging a
                // panel border across this bar must not.
                .on_mouse_move(cx.listener(
                    |this, ev: &MouseMoveEvent, window, _| {
                        if this.titlebar_drag && ev.dragging() {
                            this.titlebar_drag = false;
                            window.start_window_move();
                        }
                    },
                ))
            })
            .child(logo_image(18.))
            .when_some(self.menu_bar.clone(), |d, bar| {
                d.child(
                    div()
                        .id("app-menu")
                        .h(px(26.))
                        .flex_shrink_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(bar),
                )
            })
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.fg)
                    .child(name),
            )
            .when_some(branch, |d, b| {
                d.child(
                    div()
                        .id("top-branch")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .rounded(px(4.))
                        .hover(move |s| s.bg(p.hover))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_side(SideView::SourceControl, window, cx)
                        }))
                        .child(icon_el("git-branch", 12., p.muted))
                        .child(div().text_size(px(12.)).text_color(p.muted).child(b)),
                )
            })
            // Windows moves, snaps and maximizes the window from this area itself.
            .child(
                div()
                    .id("titlebar-drag")
                    .flex_1()
                    .h_full()
                    .when(!mac, |d| d.window_control_area(WindowControlArea::Drag)),
            )
            .children(run_controls)
            .when(proj.is_some(), |d| {
                d.child(
                    icon_button(
                        "toggle-bottom",
                        "panel-bottom",
                        format!("Toggle Bottom Panel ({})", keys::TOGGLE_BOTTOM.label()),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(
                        cx.listener(|this, _, w, cx| this.toggle_panel(Dragging::Bottom, w, cx)),
                    ),
                )
                .when(agents_available, |d| {
                    d.child(
                        icon_button(
                            "toggle-codex",
                            "sparkles",
                            format!("Toggle Codex Panel ({})", keys::TOGGLE_CODEX.label()),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(|this, _, w, cx| this.toggle_panel(Dragging::Codex, w, cx)),
                        ),
                    )
                })
            })
            .when(!mac, |d| d.child(window_controls(window, p)))
    }

    fn settings_tab_active(&self) -> bool {
        self.project.as_ref().is_some_and(|p| {
            matches!(
                p.active.and_then(|a| p.tabs.get(a)),
                Some(editor_area::Tab::Settings)
            )
        })
    }

    pub fn show_side(&mut self, view: SideView, window: &mut Window, cx: &mut Context<Self>) {
        if self.side == view && self.settings.layout.sidebar_visible {
            self.settings.layout.sidebar_visible = false;
        } else {
            self.side = view;
            self.settings.layout.sidebar_visible = true;
        }
        if view == SideView::Search
            && self.settings.layout.sidebar_visible
            && let Some(p) = &self.project
        {
            p.search.input.update(cx, |i, cx| i.focus(window, cx));
        }
        if view == SideView::SourceControl {
            self.refresh_git(cx);
        }
        self.persist();
        cx.notify();
    }

    fn render_activity(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let item = |id: &'static str,
                    name: &'static str,
                    view: SideView,
                    this: &Self,
                    cx: &mut Context<Self>| {
            let selected = this.side == view && this.settings.layout.sidebar_visible;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(p.accent_soft))
                .hover(move |s| s.bg(p.hover))
                .cursor_pointer()
                .child(icon_el(
                    name,
                    17.,
                    if selected { p.accent } else { p.muted },
                ))
                .on_click(cx.listener(move |this, _, w, cx| this.show_side(view, w, cx)))
        };
        div()
            .w(px(44.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_1()
            .bg(p.bar)
            .border_r_1()
            .border_color(p.border)
            .child(item("act-files", "folder", SideView::Files, self, cx))
            .child(item("act-search", "search", SideView::Search, self, cx))
            .child(item(
                "act-scm",
                "git-branch",
                SideView::SourceControl,
                self,
                cx,
            ))
            .child(item("act-ext", "blocks", SideView::Extensions, self, cx))
            .child(div().flex_1())
            .child(
                div()
                    .id("act-settings")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(32.))
                    .rounded(px(5.))
                    .when(self.settings_tab_active(), |d| d.bg(p.accent_soft))
                    .hover(move |s| s.bg(p.hover))
                    .cursor_pointer()
                    .child(icon_el(
                        "settings",
                        17.,
                        if self.settings_tab_active() {
                            p.accent
                        } else {
                            p.muted
                        },
                    ))
                    .on_click(cx.listener(|this, _, w, cx| this.open_settings_tab(w, cx))),
            )
            .child(div().h(px(6.)))
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let mut left: Vec<AnyElement> = vec![];
        let mut right: Vec<AnyElement> = vec![];
        let text = |s: String, color: Hsla| div().text_size(px(11.)).text_color(color).child(s);
        if let Some(proj) = &self.project {
            if let Some(st) = &proj.git.status {
                left.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icon_el("git-branch", 11., p.muted))
                        .child(text(branch_label(st), p.muted))
                        .into_any_element(),
                );
                if st.conflicts() > 0 {
                    left.push(
                        text(format!("{} conflicts", st.conflicts()), p.danger).into_any_element(),
                    );
                }
            } else if !proj.git.is_repo {
                left.push(text("no Git repository".into(), p.muted).into_any_element());
            }
            if let Some(tab) = proj.active.and_then(|i| proj.tabs.get(i))
                && let Some(ft) = tab.as_file()
            {
                let pos = ft.editor.read(cx).cursor_position();
                let (errors, warnings) = ft.diagnostic_counts;
                if errors + warnings > 0 {
                    left.push(
                        text(
                            format!("{errors} errors, {warnings} warnings"),
                            if errors > 0 { p.danger } else { p.warning },
                        )
                        .into_any_element(),
                    );
                }
                let lsp_label = self.lsp_status_label(ft);
                right.push(lsp_label.0.map_or_else(
                    || div().into_any_element(),
                    |l| text(l, lsp_label.1.unwrap_or(p.muted)).into_any_element(),
                ));
                right.push(text(ft.doc.format.label().to_string(), p.muted).into_any_element());
                if ft.doc.format.bom {
                    right.push(text("BOM".into(), p.muted).into_any_element());
                }
                let wrap = self.settings.editor.word_wrap;
                right.push(
                    div()
                        .id("status-wrap")
                        .px_1()
                        .rounded(px(3.))
                        .text_size(px(11.))
                        .text_color(if wrap { p.accent } else { p.muted })
                        .hover(move |s| s.bg(p.hover))
                        .cursor_pointer()
                        .child("Wrap")
                        .on_click(cx.listener(|this, _, w, cx| this.toggle_word_wrap(w, cx)))
                        .into_any_element(),
                );
                right.push(
                    div()
                        .id("status-lang")
                        .px_1()
                        .rounded(px(3.))
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .hover(move |s| s.bg(p.hover))
                        .cursor_pointer()
                        .child(ft.lang.display.to_string())
                        .on_click(cx.listener(|this, _, w, cx| this.open_language_picker(w, cx)))
                        .into_any_element(),
                );
                right.push(
                    text(
                        format!("Ln {}, Col {}", pos.line + 1, pos.character + 1),
                        p.muted,
                    )
                    .into_any_element(),
                );
            }
        }
        div()
            .id("statusbar")
            .h(px(24.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .bg(p.bar)
            .border_t_1()
            .border_color(p.border)
            .children(left)
            .child(div().flex_1())
            .children(right)
    }

    fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let p = pal(cx);
        let (msg, error) = self.toast.clone()?;
        Some(
            div()
                .absolute()
                .bottom(px(34.))
                .right(px(12.))
                .max_w(px(520.))
                .px_3()
                .py_2()
                .rounded(px(6.))
                .bg(p.bar)
                .border_1()
                .border_color(if error { p.danger } else { p.border })
                .text_size(px(12.))
                .text_color(if error { p.danger } else { p.fg })
                .child(msg),
        )
    }

    fn render_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // First time this project shows the panel: a fifth of the working area.
        if self.settings.layout.bottom_visible
            && let Some(p) = &mut self.project
            && !p.bottom_height_set
        {
            p.bottom_height_set = true;
            let area = f32::from(window.viewport_size().height) - 62.;
            self.settings.layout.bottom_height = (area * 0.2).max(80.);
        }
        let l = self.settings.layout.clone();
        let sidebar = l
            .sidebar_visible
            .then(|| self.render_sidebar(window, cx).into_any_element());
        let center = self.render_editor_area(window, cx).into_any_element();
        let bottom = l
            .bottom_visible
            .then(|| self.render_bottom(window, cx).into_any_element());
        let codex = l.codex_visible.then(|| self.render_agents(window, cx));
        let side_h = self.handle(Dragging::Sidebar, cx).into_any_element();
        let bottom_h = self.handle(Dragging::Bottom, cx).into_any_element();
        let codex_h = self.handle(Dragging::Codex, cx).into_any_element();
        let activity = self.render_activity(cx).into_any_element();
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .child(activity)
            .when_some(sidebar, |d, s| {
                d.child(
                    div()
                        .w(px(l.sidebar_width))
                        .h_full()
                        .flex_shrink_0()
                        .child(s),
                )
                .child(side_h)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().w_full().child(center))
                    .when_some(bottom, |d, b| {
                        d.child(bottom_h).child(
                            div()
                                .h(px(l.bottom_height))
                                .flex_shrink_0()
                                .w_full()
                                .child(b),
                        )
                    }),
            )
            .when_some(codex, |d, c| {
                d.child(codex_h)
                    .child(div().w(px(l.codex_width)).h_full().flex_shrink_0().child(c))
            })
    }

    fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let content = match self.side {
            SideView::Files => self.render_files(window, cx).into_any_element(),
            SideView::Search => self.render_search(cx).into_any_element(),
            SideView::SourceControl => self.render_scm(window, cx).into_any_element(),
            SideView::Extensions => self.render_extensions_panel(cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.panel)
            .child(content)
    }
}

pub fn branch_label(st: &RepoStatus) -> String {
    let mut label = st.branch.clone().unwrap_or_else(|| {
        if st.detached {
            "detached".into()
        } else {
            "—".into()
        }
    });
    if st.ahead > 0 {
        label.push_str(&format!(" ↑{}", st.ahead));
    }
    if st.behind > 0 {
        label.push_str(&format!(" ↓{}", st.behind));
    }
    label
}

impl Focusable for KernApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KernApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        // Shortcuts are dispatched from the focused element. With nothing focused,
        // at startup or after a focused view left the screen, they would be lost.
        if window.focused(cx).is_none() {
            let focus = self.focus.clone();
            window.defer(cx, move |window, cx| {
                if window.focused(cx).is_none() {
                    window.focus(&focus, cx);
                }
            });
        }
        // The Explorer's name field goes when the Explorer closes or gives way to
        // another view, and when the focus leaves it (a click in the editor or
        // elsewhere): nothing was created, so nothing is left behind.
        let field_left = match self.project.as_mut().and_then(|p| p.tree.edit.as_mut()) {
            Some(e) => {
                let focused = e.input.read(cx).focus_handle(cx).is_focused(window);
                let left = e.had_focus && !focused;
                e.had_focus |= focused;
                left
            }
            None => false,
        };
        if field_left || !self.settings.layout.sidebar_visible || self.side != SideView::Files {
            self.drop_tree_edit(cx);
        }
        let top = self.render_topbar(window, cx).into_any_element();
        let body = if self.project.is_some() {
            self.render_workspace(window, cx).into_any_element()
        } else if self.settings_standalone {
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.render_settings_tab(cx))
                .into_any_element()
        } else {
            self.render_start(cx).into_any_element()
        };
        let status = self.render_status(cx).into_any_element();
        let overlay = self.render_overlay(window, cx);
        let toast = self.render_toast(cx);
        let update_notice = self.render_update_notice(cx);
        div()
            .id("kern-root")
            .key_context("KernApp")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.fg)
            .text_size(px(13.))
            .font_family(".SystemUIFont")
            .on_action(cx.listener(|this, _: &Save, w, cx| this.save_active(false, w, cx)))
            .on_action(cx.listener(|this, _: &QuickOpen, w, cx| {
                this.open_quick_open(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &CommandPalette, w, cx| {
                this.open_command_palette(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &FindInProject, w, cx| {
                this.side = SideView::Search;
                this.settings.layout.sidebar_visible = true;
                if let Some(p) = &this.project {
                    p.search.input.update(cx, |i, cx| i.focus(w, cx));
                }
                cx.notify();
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, w, cx| {
                this.toggle_panel(Dragging::Sidebar, w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleBottom, w, cx| {
                this.toggle_panel(Dragging::Bottom, w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleCodex, w, cx| {
                this.toggle_panel(Dragging::Codex, w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, w, cx| {
                this.new_terminal(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, w, cx| {
                this.toggle_terminal(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseTab, w, cx| this.close_active_tab(w, cx)))
            .on_action(cx.listener(|this, _: &OpenFolder, w, cx| {
                this.pick_folder(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &RunProject, w, cx| this.run_selected(w, cx)))
            .on_action(cx.listener(|this, _: &StopProject, _, cx| this.stop_run(cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, w, cx| {
                this.open_settings_tab(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleWordWrap, w, cx| this.toggle_word_wrap(w, cx)))
            .on_action(cx.listener(|this, _: &ChangeLanguage, w, cx| {
                this.open_language_picker(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowExtensions, w, cx| {
                this.side = SideView::SourceControl;
                this.show_side(SideView::Extensions, w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenClaude, w, cx| {
                this.open_claude(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &NewFile, w, cx| {
                this.prompt_new_file(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &NewFolder, w, cx| {
                this.prompt_new_folder(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(cx.listener(|this, _: &KillTerminal, w, cx| {
                this.kill_terminal(w, cx);
                hold_focus(w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &CheckForUpdates, _, cx| this.check_for_updates(true, cx)),
            )
            .on_action(cx.listener(|this, _: &DownloadUpdate, _, cx| this.download_update(cx)))
            .on_action(cx.listener(|this, _: &RestartToUpdate, _, cx| this.restart_to_update(cx)))
            .on_action(cx.listener(|this, _: &DownloadLatestRelease, _, cx| {
                this.download_latest_manually(cx)
            }))
            .on_action(cx.listener(|this, _: &AboutKern, _, cx| this.show_about(cx)))
            .on_action(
                cx.listener(|this, _: &GoToDefinitionHere, w, cx| this.go_to_definition(w, cx)),
            )
            .on_action(cx.listener(|this, _: &Quit, w, cx| {
                this.request_quit(w, cx, true);
            }))
            .on_mouse_move(cx.listener(Self::on_drag_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging.take().is_some() {
                        this.persist();
                        cx.notify();
                    }
                }),
            )
            .child(top)
            .child(body)
            .child(status)
            .children(update_notice)
            .children(overlay)
            .children(toast)
    }
}

/// Menu actions move the focus (a new terminal, a prompt). The title-bar menu on
/// Windows gives the focus back to where it was when it opened, after the action
/// ran: whatever the action focused is focused again once that has happened.
fn hold_focus(window: &mut Window, cx: &mut App) {
    if let Some(focused) = window.focused(cx) {
        window.defer(cx, move |window, cx| window.focus(&focused, cx));
    }
}

/// Minimize, maximize or restore, and close, drawn as Windows draws them. Windows
/// itself handles the clicks through the hit test, including snap layouts.
fn window_controls(window: &Window, p: super::theme::Palette) -> impl IntoElement {
    let maximized = window.is_maximized();
    let button = |id: &'static str, glyph: &'static str, area: WindowControlArea, close: bool| {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(46.))
            .h_full()
            .flex_shrink_0()
            .font_family("Segoe Fluent Icons")
            .text_size(px(10.))
            .text_color(p.fg)
            .hover(move |s| {
                if close {
                    s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))
                } else {
                    s.bg(p.hover)
                }
            })
            .window_control_area(area)
            .child(glyph)
    };
    div()
        .flex()
        .h_full()
        .flex_shrink_0()
        .child(button("win-min", "\u{E921}", WindowControlArea::Min, false))
        .child(button(
            "win-max",
            if maximized { "\u{E923}" } else { "\u{E922}" },
            WindowControlArea::Max,
            false,
        ))
        .child(button(
            "win-close",
            "\u{E8BB}",
            WindowControlArea::Close,
            true,
        ))
}
