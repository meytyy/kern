//! Bottom panel: terminals, Run Output, Claude Code. Top-bar run controls.
use super::app::*;
use super::terminal_view::TerminalView;
use super::theme::pal;
use super::widgets::*;
use crate::agents::attachments::{Attachment, is_secret_file};

use crate::run::detect::{self, Detection, RunConfig};
use crate::run::output::{self, ErrorLocation, Link, OutputLog, Stream};
use crate::run::process::{RunEvent, RunningProcess};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

pub const CLAUDE_INSTALL_URL: &str = "https://code.claude.com/docs/en/quickstart";

#[derive(Default)]
pub struct RunState {
    pub detection: Detection,
    pub selected: Option<String>,
    pub process: Option<RunningProcess>,
    pub running: Option<RunConfig>,
    pub log: OutputLog,
    pub menu_open: bool,
    pub selection: Option<(usize, usize)>,
    pub scroll: UniformListScrollHandle,
    pub follow: bool,
    pub cwd: PathBuf,
}

impl KernApp {
    pub fn refresh_run_configs(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let root = p.root.clone();
        let session = self.settings.session(&root);
        cx.spawn(async move |this, cx| {
            let r = root.clone();
            let detection = cx
                .background_spawn(async move {
                    detect::detect(
                        &r,
                        session.package_manager.as_deref(),
                        session.python.as_deref(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(p) = &mut this.project else { return };
                if p.root != root {
                    return;
                }
                if p.run
                    .selected
                    .as_ref()
                    .is_none_or(|s| !detection.configs.iter().any(|c| &c.id == s))
                {
                    p.run.selected = detection.configs.first().map(|c| c.id.clone());
                }
                p.run.detection = detection;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn active_file_config(&self) -> Option<RunConfig> {
        let p = self.project.as_ref()?;
        let f = self.active_file()?;
        match f.lang.run {
            crate::lang::RunSupport::Python => Some(detect::python_file_config(
                &p.root,
                &f.path,
                self.settings.session(&p.root).python.as_deref(),
            )),
            crate::lang::RunSupport::Shell => Some(detect::shell_file_config(&p.root, &f.path)),
            _ => None,
        }
    }

    fn selected_config(&self) -> Option<RunConfig> {
        let p = self.project.as_ref()?;
        let id = p.run.selected.as_ref()?;
        p.run
            .detection
            .configs
            .iter()
            .find(|c| &c.id == id)
            .cloned()
            .or_else(|| self.active_file_config())
    }

    /// Explicit user action only. Nothing in a project runs without it.
    pub fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The file in the editor runs when its language says so and nothing else
        // was detected for the project.
        let Some(config) = self.selected_config().or_else(|| self.active_file_config()) else {
            self.toast(
                "Nothing to run: no package.json scripts, Python entry point or Cargo.toml found",
                true,
                cx,
            );
            return;
        };
        self.run_config(config, window, cx);
    }

    pub fn run_config(&mut self, config: RunConfig, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        if let Some(old) = p.run.process.take() {
            old.stop();
        }
        p.run.log.clear();
        p.run.selection = None;
        p.run.follow = true;
        p.run.cwd = config.cwd.clone();
        p.run.log.push_meta(format!("$ {}", config.command_line()));
        p.run
            .log
            .push_meta(format!("cwd: {}", config.cwd.display()));
        p.run.running = Some(config.clone());
        p.bottom_tab = BottomTab::RunOutput;
        self.settings.layout.bottom_visible = true;
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let (proc_tx, proc_rx) = futures::channel::oneshot::channel();
        std::thread::Builder::new()
            .name("run-spawn".into())
            .spawn(move || {
                let _ = proc_tx.send(RunningProcess::spawn(&config, tx));
            })
            .ok();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let proc = proc_rx.await.ok().flatten();
            let mine = proc.as_ref().map(RunningProcess::pgid);
            this.update(cx, |this, cx| {
                if let Some(p) = &mut this.project {
                    p.run.process = proc;
                    cx.notify();
                }
            })
            .ok();
            while let Some(ev) = rx.next().await {
                let mut batch = vec![ev];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let ok = this.update(cx, |this, cx| {
                    let Some(p) = &mut this.project else { return };
                    // Output of a process that was replaced belongs to nobody.
                    if mine.is_some() && p.run.process.as_ref().map(RunningProcess::pgid) != mine {
                        return;
                    }
                    for ev in batch {
                        match ev {
                            RunEvent::Output(stream, text) => p.run.log.push_chunk(stream, &text),
                            RunEvent::SpawnFailed(msg) => {
                                p.run.log.push_chunk(Stream::Stderr, &format!("{msg}\n"));
                                p.run.running = None;
                            }
                            RunEvent::Exited(code) => {
                                p.run.log.push_meta(match code {
                                    Some(c) => format!("process exited with code {c}"),
                                    None => "process terminated by signal".to_string(),
                                });
                                p.run.process = None;
                                p.run.running = None;
                            }
                        }
                    }
                    if p.run.follow {
                        let n = p.run.log.lines.len();
                        if n > 0 {
                            p.run.scroll.scroll_to_item(n - 1, ScrollStrategy::Bottom);
                        }
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub fn stop_run(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.project
            && let Some(proc) = &p.run.process
        {
            proc.stop();
            p.run.log.push_meta("Stopping process group…");
        }
        cx.notify();
    }

    pub fn render_run_controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal(cx);
        let p = self.project.as_ref().expect("project");
        let running = p.run.process.is_some();
        let selected = p
            .run
            .selected
            .as_ref()
            .and_then(|id| p.run.detection.configs.iter().find(|c| &c.id == id));
        let label = selected
            .map(|c| c.label.clone())
            .unwrap_or_else(|| "No run configuration".to_string());
        let menu_open = p.run.menu_open;
        let configs = p.run.detection.configs.clone();
        let managers = p.run.detection.ambiguous_managers.clone();
        let current_manager = p.run.detection.package_manager;
        let stop_prop = |_: &MouseDownEvent, _: &mut Window, cx: &mut App| cx.stop_propagation();
        div()
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("run-select")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(24.))
                    .px_2()
                    .rounded(px(4.))
                    .bg(palette.hover)
                    .hover(move |s| s.bg(palette.active))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, stop_prop)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(p) = &mut this.project {
                            p.run.menu_open = !p.run.menu_open;
                        }
                        cx.notify();
                    }))
                    .child(div().text_size(px(12.)).max_w(px(220.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
                    .child(icon_el("chevrons-up-down", 11., palette.muted)),
            )
            .child(
                div()
                    .id("run-btn")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(4.))
                    .bg(palette.accent_soft)
                    .hover(|s| s.opacity(0.85))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, stop_prop)
                    .on_click(cx.listener(|this, _, w, cx| this.run_selected(w, cx)))
                    .child(icon_el("play", 13., palette.accent)),
            )
            .child(
                div()
                    .id("stop-btn")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(4.))
                    .hover(move |s| s.bg(palette.hover))
                    .when(running, |d| d.cursor_pointer())
                    .on_mouse_down(MouseButton::Left, stop_prop)
                    .on_click(cx.listener(|this, _, _, cx| this.stop_run(cx)))
                    .child(icon_el("square", 12., if running { palette.danger } else { palette.border })),
            )
            .when(menu_open, |d| {
                d.child(deferred(
                    anchored().child(
                        div()
                            .id("run-menu")
                            .occlude()
                            .mt(px(30.))
                            .w(px(320.))
                            .max_h(px(420.))
                            .overflow_y_scroll()
                            .rounded(px(6.))
                            .bg(palette.bar)
                            .border_1()
                            .border_color(palette.border)
                            .on_mouse_down(MouseButton::Left, stop_prop)
                            .when(configs.is_empty(), |d| d.child(div().p_2().text_size(px(12.)).text_color(palette.muted).child("Nothing to run was detected.")))
                            .children(configs.into_iter().enumerate().map(|(i, c)| {
                                let id = c.id.clone();
                                div()
                                    .id(("cfg", i))
                                    .h(px(26.))
                                    .flex()
                                    .items_center()
                                    .px_2()
                                    .text_size(px(12.))
                                    .hover(move |s| s.bg(palette.hover))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(p) = &mut this.project {
                                            p.run.selected = Some(id.clone());
                                            p.run.menu_open = false;
                                        }
                                        this.persist();
                                        cx.notify();
                                    }))
                                    .child(c.label)
                            }))
                            .when(!managers.is_empty(), |d| {
                                d.child(hsep(palette))
                                    .child(div().px_2().py_1().text_size(px(11.)).text_color(palette.warning).child("Several lock files found. Choose a package manager:"))
                                    .children(managers.into_iter().enumerate().map(|(i, m)| {
                                        let name = m.to_string();
                                        div()
                                            .id(("pm", i))
                                            .h(px(26.))
                                            .flex()
                                            .items_center()
                                            .px_2()
                                            .text_size(px(12.))
                                            .when(current_manager == Some(m), |d| d.text_color(palette.accent))
                                            .hover(move |s| s.bg(palette.hover))
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(root) = this.project.as_ref().map(|p| p.root.clone()) {
                                                    this.settings.session_mut(&root).package_manager = Some(name.clone());
                                                }
                                                this.refresh_run_configs(cx);
                                                this.persist();
                                            }))
                                            .child(m)
                                    }))
                            }),
                    ),
                ))
            })
    }

    pub fn ensure_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .project
            .as_ref()
            .is_some_and(|p| p.terminals.is_empty())
        {
            self.new_terminal_inner(false, None, window, cx);
        }
    }

    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_inner(true, None, window, cx);
    }

    /// Explorer → Open in Integrated Terminal: a shell started in `dir`.
    pub fn new_terminal_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_inner(true, Some(dir), window, cx);
        self.persist();
    }

    fn new_terminal_inner(
        &mut self,
        focus: bool,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shell = self.settings.shell.clone();
        self.settings.layout.bottom_visible = true;
        let Some(p) = &mut self.project else { return };
        let n = p.terminals.len() + 1;
        let root = cwd.filter(|d| d.is_dir()).unwrap_or_else(|| p.root.clone());
        // Named after the shell that runs: zsh on a Mac, pwsh or powershell on Windows.
        let shell_name = std::path::Path::new(&crate::platform::user_shell(shell.as_deref()))
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "shell".into());
        let view =
            cx.new(|cx| TerminalView::new(format!("{shell_name} {n}"), None, &root, shell, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        if focus {
            view.update(cx, |v, cx| v.focus(window, cx));
            p.bottom_tab = BottomTab::Terminal;
        }
        p.terminals.push(view);
        p.active_terminal = p.terminals.len() - 1;
        cx.notify();
    }

    /// Ends the session of the active terminal. Hiding the panel never does this.
    pub fn kill_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let ix = p.active_terminal;
        self.close_terminal(ix, cx);
        // Nothing left to show: a new session starts only when asked for.
        if self
            .project
            .as_ref()
            .is_some_and(|p| p.terminals.is_empty())
        {
            self.hide_bottom(window, cx);
        }
    }

    fn close_terminal(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        if ix < p.terminals.len() {
            p.terminals.remove(ix);
            p.active_terminal = p.active_terminal.min(p.terminals.len().saturating_sub(1));
        }
        cx.notify();
    }

    /// A short-lived interactive terminal used for official login flows.
    pub fn login_terminal(
        &mut self,
        program: PathBuf,
        args: Vec<String>,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Entity<TerminalView>, String> {
        let view = cx.new(|cx| {
            TerminalView::new(
                "Sign in",
                Some((program.to_string_lossy().to_string(), args)),
                &cwd,
                None,
                cx,
            )
        });
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        view.update(cx, |v, cx| v.focus(window, cx));
        Ok(view)
    }

    /// Attach selected output (or the last lines) and the related file to Codex.
    pub fn send_output_to_codex(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let lines = &p.run.log.lines;
        if lines.is_empty() {
            return;
        }
        let (a, b) = p
            .run
            .selection
            .unwrap_or((lines.len().saturating_sub(40), lines.len() - 1));
        let (a, b) = (a.min(b), a.max(b).min(lines.len() - 1));
        let text = lines[a..=b]
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let cwd = p.run.cwd.clone();
        let related = lines[a..=b]
            .iter()
            .flat_map(|l| output::find_locations(&l.text))
            .find_map(|loc| output::resolve_location(&loc.path, &cwd));
        let label = p
            .run
            .running
            .as_ref()
            .map(|c| c.command_line())
            .unwrap_or_else(|| "Run output".into());
        let mut atts = vec![Attachment::Output {
            label: format!("Output: {label}"),
            content: text,
            file: related.clone(),
        }];
        let mut skipped = None;
        if let Some(file) = related {
            if is_secret_file(&file) {
                skipped = Some(file);
            } else if let Ok(content) = std::fs::read_to_string(&file) {
                atts.push(Attachment::File {
                    path: file,
                    content,
                });
            }
        }
        self.settings.layout.codex_visible = true;
        let agent = self.agent_tab;
        self.agent_connect(agent, cx);
        if let Some(c) = self.chat_mut(agent) {
            c.attachments.extend(atts);
        }
        if let Some(s) = skipped {
            self.toast(
                format!(
                    "{} looks like a secret file and was not attached",
                    s.display()
                ),
                false,
                cx,
            );
        }
        cx.notify();
    }

    fn click_location(
        &mut self,
        loc: &ErrorLocation,
        cwd: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match output::resolve_location(&loc.path, cwd) {
            Some(path) => self.open_file_async(
                path,
                true,
                Some((
                    loc.line.saturating_sub(1),
                    loc.column.unwrap_or(1).saturating_sub(1),
                )),
                window,
                cx,
            ),
            None => self.toast(format!("File not found: {}", loc.path), true, cx),
        }
    }

    fn render_run_output(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = pal(cx);
        let p = self.project.as_ref().expect("project");
        let lines = p.run.log.lines.clone();
        let count = lines.len();
        let selection = p.run.selection.map(|(a, b)| (a.min(b), a.max(b)));
        let cwd = p.run.cwd.clone();
        let scroll = p.run.scroll.clone();

        uniform_list(
            "run-output",
            count,
            cx.processor(move |_t, range: std::ops::Range<usize>, _w, cx| {
                let mut out = Vec::new();
                for ix in range {
                    let line = &lines[ix];
                    let color = match line.stream {
                        Stream::Meta => palette.accent,
                        Stream::Stderr => palette.warning,
                        Stream::Stdout => palette.fg,
                    };
                    let selected = selection.is_some_and(|(a, b)| ix >= a && ix <= b);
                    let segs = segments(&line.text);
                    let mut children: Vec<AnyElement> = vec![];
                    for (si, seg) in segs.into_iter().enumerate() {
                        match seg {
                            Segment::Plain(t) => children.push(div().child(t).into_any_element()),
                            Segment::Url(link) => {
                                let target = output::browser_url(&link.url);
                                children.push(
                                    div()
                                        .id(SharedString::from(format!("u{ix}-{si}")))
                                        .text_color(palette.accent)
                                        .underline()
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(
                                            cx.listener(move |_, _, _, cx| cx.open_url(&target)),
                                        )
                                        .child(link.url.clone())
                                        .into_any_element(),
                                );
                            }
                            Segment::Loc(loc, t) => {
                                let dir = cwd.clone();
                                if output::resolve_location(&loc.path, &dir).is_some() {
                                    children.push(
                                        div()
                                            .id(SharedString::from(format!("l{ix}-{si}")))
                                            .text_color(palette.danger)
                                            .underline()
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(cx.listener(move |this, _, w, cx| {
                                                this.click_location(&loc, &dir, w, cx)
                                            }))
                                            .child(t)
                                            .into_any_element(),
                                    );
                                } else {
                                    children.push(div().child(t).into_any_element());
                                }
                            }
                        }
                    }
                    out.push(
                        div()
                            .id(("line", ix))
                            .h(px(18.))
                            .w_full()
                            .flex()
                            .px_2()
                            .whitespace_nowrap()
                            .text_color(color)
                            .when(selected, |d| d.bg(palette.selection))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    if let Some(p) = &mut this.project {
                                        p.run.selection =
                                            match (ev.modifiers.shift, p.run.selection) {
                                                (true, Some((a, _))) => Some((a, ix)),
                                                _ => Some((ix, ix)),
                                            };
                                    }
                                    cx.notify();
                                }),
                            )
                            .children(children)
                            .into_any_element(),
                    );
                }
                out
            }),
        )
        .track_scroll(&scroll)
        .size_full()
        .font_family("Menlo")
        .text_size(px(12.))
    }

    pub fn render_bottom(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = pal(cx);
        self.ensure_terminal(window, cx);
        let run_view = self.render_run_output(cx).into_any_element();
        let claude_body = self
            .project
            .as_ref()
            .is_some_and(|p| p.bottom_tab == BottomTab::Claude)
            .then(|| self.render_claude_bottom(cx));
        let claude_on = self.settings.extension_enabled("claude");
        let codex_on = self.settings.extension_enabled("codex");
        let p = self.project.as_ref().expect("project");
        let tab = p.bottom_tab;
        let terminals = p.terminals.clone();
        let active_term = p.active_terminal.min(terminals.len().saturating_sub(1));
        let has_output = !p.run.log.lines.is_empty();
        let has_claude = p.claude_code.is_some();
        let tab_btn = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h_full()
                .px_2()
                .text_size(px(12.))
                .text_color(if on { palette.fg } else { palette.muted })
                .border_b_2()
                .border_color(if on {
                    palette.accent
                } else {
                    gpui_kit::transparent_black()
                })
                .cursor_pointer()
                .child(label)
        };
        let content: AnyElement = match tab {
            BottomTab::Terminal => match terminals.get(active_term) {
                Some(t) => t.clone().into_any_element(),
                None => div().into_any_element(),
            },
            BottomTab::RunOutput => run_view,
            BottomTab::Claude => claude_body.expect("rendered above"),
        };
        let mut term_tabs: Vec<AnyElement> = vec![];
        if tab == BottomTab::Terminal && terminals.len() > 1 {
            for (i, t) in terminals.iter().enumerate() {
                let title = t.read(cx).title.clone();
                term_tabs.push(
                    div()
                        .id(("term-tab", i))
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(20.))
                        .px_1()
                        .rounded(px(3.))
                        .max_w(px(140.))
                        .text_size(px(11.))
                        .text_color(if i == active_term {
                            palette.accent
                        } else {
                            palette.muted
                        })
                        .when(i == active_term, |d| d.bg(palette.accent_soft))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, w, cx| {
                            if let Some(p) = &mut this.project {
                                p.active_terminal = i;
                                if let Some(t) = p.terminals.get(i).cloned() {
                                    t.update(cx, |v, cx| v.focus(w, cx));
                                }
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(title),
                        )
                        .child(
                            div()
                                .id(("term-close", i))
                                .text_color(palette.muted)
                                .child("×")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_terminal(i, cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.panel)
            .child(
                div()
                    .h(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        tab_btn("tab-term", "Terminal".into(), tab == BottomTab::Terminal)
                            .on_click(cx.listener(|this, _, w, cx| {
                                if let Some(p) = &mut this.project {
                                    p.bottom_tab = BottomTab::Terminal;
                                    if let Some(t) = p.terminals.get(p.active_terminal).cloned() {
                                        t.update(cx, |v, cx| v.focus(w, cx));
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        tab_btn("tab-run", "Run Output".into(), tab == BottomTab::RunOutput)
                            .on_click(cx.listener(|this, _, w, cx| {
                                if let Some(p) = &mut this.project {
                                    p.bottom_tab = BottomTab::RunOutput;
                                }
                                // The terminal is off screen now: keys must not go to it.
                                w.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    )
                    .when(claude_on, |d| {
                        d.child(
                            tab_btn("tab-claude", "Claude Code".into(), tab == BottomTab::Claude)
                                .on_click(cx.listener(|this, _, w, cx| {
                                    if let Some(p) = &mut this.project {
                                        p.bottom_tab = BottomTab::Claude;
                                        if let Some(v) = p.claude_code.clone() {
                                            v.update(cx, |v, cx| v.focus(w, cx));
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().w(px(6.)))
                    .children(term_tabs)
                    .child(div().flex_1())
                    .when(tab == BottomTab::RunOutput && has_output && codex_on, |d| {
                        d.child(
                            button("to-agent", "Ask Agent", palette).on_click(
                                cx.listener(|this, _, _, cx| this.send_output_to_codex(cx)),
                            ),
                        )
                    })
                    .when(tab == BottomTab::RunOutput && has_output, |d| {
                        d.child(icon_button("copy-out", "copy", "Copy Output").on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(p) = &this.project {
                                    let lines = &p.run.log.lines;
                                    let text = match p.run.selection {
                                        Some((a, b)) if !lines.is_empty() => lines
                                            [a.min(b)..=a.max(b).min(lines.len() - 1)]
                                            .iter()
                                            .map(|l| l.text.as_str())
                                            .collect::<Vec<_>>()
                                            .join("\n"),
                                        _ => p.run.log.text(),
                                    };
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                }
                            }),
                        ))
                    })
                    .when(tab == BottomTab::Claude && has_claude, |d| {
                        d.child(
                            icon_button("claude-end", "trash", "End Claude Code Session")
                                .on_click(cx.listener(|this, _, w, cx| this.claude_end(w, cx))),
                        )
                    })
                    .when(tab == BottomTab::Terminal, |d| {
                        d.child(
                            icon_button("new-term", "plus", "New Terminal")
                                .on_click(cx.listener(|this, _, w, cx| this.new_terminal(w, cx))),
                        )
                        .child(
                            icon_button("kill-term", "trash", "Kill Terminal")
                                .on_click(cx.listener(|this, _, w, cx| this.kill_terminal(w, cx))),
                        )
                    })
                    .child(
                        icon_button("bottom-hide", "x", "Hide Panel")
                            .on_click(cx.listener(|this, _, w, cx| this.hide_bottom(w, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().w_full().child(content))
    }
}

enum Segment {
    Plain(String),
    Url(Link),
    Loc(ErrorLocation, String),
}

fn segments(line: &str) -> Vec<Segment> {
    let urls = output::find_urls(line);
    let locs = output::find_locations(line);
    let mut marks: Vec<(usize, usize, Option<Link>, Option<ErrorLocation>)> = vec![];
    for u in urls {
        marks.push((u.start, u.end, Some(u), None));
    }
    for l in locs {
        if !marks.iter().any(|m| l.start < m.1 && m.0 < l.end) {
            marks.push((l.start, l.end, None, Some(l)));
        }
    }
    marks.sort_by_key(|m| m.0);
    let mut out = vec![];
    let mut pos = 0;
    for (start, end, url, loc) in marks {
        if start > pos {
            out.push(Segment::Plain(line[pos..start].to_string()));
        }
        match (url, loc) {
            (Some(u), _) => out.push(Segment::Url(u)),
            (_, Some(l)) => out.push(Segment::Loc(l, line[start..end].to_string())),
            _ => {}
        }
        pos = end;
    }
    if pos < line.len() || out.is_empty() {
        out.push(Segment::Plain(line[pos..].to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn segments_partition_the_line() {
        let s = segments("  ➜  Local:   http://localhost:5173/ and src/a.ts:1:2 end");
        let rebuilt: String = s
            .iter()
            .map(|x| match x {
                Segment::Plain(t) | Segment::Loc(_, t) => t.clone(),
                Segment::Url(l) => l.url.clone(),
            })
            .collect();
        assert_eq!(
            rebuilt,
            "  ➜  Local:   http://localhost:5173/ and src/a.ts:1:2 end"
        );
        assert!(s.iter().any(|x| matches!(x, Segment::Url(_))));
        assert!(s.iter().any(|x| matches!(x, Segment::Loc(..))));
    }
}
