//! Settings: a tab in the central area, with search and categories. Every row
//! says whether it is global or belongs to the open project.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use super::*;
use crate::core::settings::{CustomLsp, Settings, ThemeChoice};
use crate::lang::{self, Highlight, LANGUAGES};
use crate::platform::tools::{self, Probe};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Editor,
    Appearance,
    Terminal,
    Languages,
    Git,
    Extensions,
    Tools,
}

const CATEGORIES: &[(Category, &str)] = &[
    (Category::Editor, "Editor"),
    (Category::Appearance, "Appearance"),
    (Category::Terminal, "Terminal"),
    (Category::Languages, "Languages"),
    (Category::Git, "Git"),
    (Category::Extensions, "Extensions"),
    (Category::Tools, "Tools"),
];

#[derive(Clone, Copy)]
enum Scope {
    Global,
    Project,
}

pub struct SettingsForm {
    stops: std::cell::RefCell<std::collections::HashMap<ElementId, FocusHandle>>,
    pub query: Entity<InputState>,
    pub category: Category,
    pub font_family: Entity<InputState>,
    pub shell: Entity<InputState>,
    pub tool_name: Entity<InputState>,
    pub tool_path: Entity<InputState>,
    pub python: Entity<InputState>,
    pub assoc_pattern: Entity<InputState>,
    pub assoc_language: Entity<InputState>,
    pub lsp_name: Entity<InputState>,
    pub lsp_cmd: Entity<InputState>,
    pub lsp_args: Entity<InputState>,
    pub lsp_ext: Entity<InputState>,
    pub lsp_roots: Entity<InputState>,
    pub probes: BTreeMap<String, Probe>,
    pub probing: bool,
    /// Width available to the rows, worked out from the window and the open panels
    /// on each render. Rows stack and the language table becomes a list below
    /// the thresholds in `Layout`.
    pub content_width: f32,
    pub _subs: Vec<Subscription>,
}

/// Where the Settings layout changes, in pixels of row width.
pub struct Layout;
impl Layout {
    /// Below this, a control goes under its title and description.
    pub const STACK: f32 = 600.;
    /// The language table needs this much for its five columns.
    pub const TABLE: f32 = 760.;
}

impl SettingsForm {
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<KernApp>) -> Self {
        let mut mk = |placeholder: &str, value: String| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let query = mk("Search settings", String::new());
        let font_family = mk("Font family", settings.editor.font_family.clone());
        let shell = mk(
            "Default: $SHELL, then /bin/zsh",
            settings.shell.clone().unwrap_or_default(),
        );
        let tool_name = mk("Tool (codex, node, gh…)", String::new());
        let tool_path = mk("Absolute path; empty resets", String::new());
        let python = mk(
            &format!(
                "Default: .venv/{}",
                crate::platform::VENV_PYTHON.replace('\\', "/")
            ),
            String::new(),
        );
        let assoc_pattern = mk("*.conf or a file name", String::new());
        let assoc_language = mk("Language id (toml, yaml…)", String::new());
        let lsp_name = mk("Name", String::new());
        let lsp_cmd = mk("Command", String::new());
        let lsp_args = mk("Arguments (e.g. --stdio)", String::new());
        let lsp_ext = mk("Extensions (go, templ)", String::new());
        let lsp_roots = mk("Root markers (go.mod, .git)", String::new());
        let subs = vec![
            cx.subscribe_in(
                &query,
                window,
                |_: &mut KernApp, _, _: &InputEvent, _, cx| cx.notify(),
            ),
            cx.subscribe_in(
                &font_family,
                window,
                |this: &mut KernApp, input, ev: &InputEvent, w, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let v = input.read(cx).value().trim().to_string();
                        if !v.is_empty() && v != this.settings.editor.font_family {
                            this.settings.editor.font_family = v;
                            this.apply_editor_settings(w, cx);
                        }
                    }
                },
            ),
            cx.subscribe_in(
                &shell,
                window,
                |this: &mut KernApp, _, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.save_shell(cx);
                    }
                },
            ),
        ];
        Self {
            stops: Default::default(),
            query,
            category: Category::Editor,
            font_family,
            shell,
            tool_name,
            tool_path,
            python,
            assoc_pattern,
            assoc_language,
            lsp_name,
            lsp_cmd,
            lsp_args,
            lsp_ext,
            lsp_roots,
            probes: BTreeMap::new(),
            probing: false,
            content_width: 900.,
            _subs: subs,
        }
    }
}

fn words(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every command worth diagnosing: the known tools and all language servers.
fn tools_to_probe() -> Vec<String> {
    let mut names: Vec<String> = tools::KNOWN_TOOLS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    for l in LANGUAGES {
        if let Some(s) = l.server {
            for c in s.info().commands {
                if !names.iter().any(|n| n == c) {
                    names.push(c.to_string());
                }
            }
        }
    }
    names
}

impl KernApp {
    pub fn open_settings_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.project {
            Some(p) => {
                let ix = p
                    .tabs
                    .iter()
                    .position(|t| matches!(t, editor_area::Tab::Settings));
                let ix = ix.unwrap_or_else(|| {
                    p.tabs.push(editor_area::Tab::Settings);
                    p.tabs.len() - 1
                });
                p.active = Some(ix);
            }
            None => self.settings_standalone = true,
        }
        let q = self.settings_form.query.clone();
        q.update(cx, |i, cx| i.focus(window, cx));
        if self.settings_form.probes.is_empty() {
            self.run_tool_probes(cx);
        }
        cx.notify();
    }

    /// Runs every tool off the UI thread: being on PATH proves nothing by itself.
    pub fn run_tool_probes(&mut self, cx: &mut Context<Self>) {
        if self.settings_form.probing {
            return;
        }
        self.settings_form.probing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move {
                    let names = tools_to_probe();
                    let handles: Vec<_> = names
                        .into_iter()
                        .map(|n| {
                            std::thread::spawn(move || {
                                let result = tools::probe(&n);
                                (n, result)
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .filter_map(|h| h.join().ok())
                        .collect::<BTreeMap<_, _>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.settings_form.probes = results;
                this.settings_form.probing = false;
                this.after_probes(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Push editor settings to every open editor.
    pub fn apply_editor_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        theme::apply(&self.settings, cx);
        let e = self.settings.editor.clone();
        if let Some(p) = &self.project {
            for t in &p.tabs {
                if let Some(f) = t.as_file() {
                    f.editor.update(cx, |ed, cx| {
                        ed.set_soft_wrap(e.word_wrap, window, cx);
                        ed.set_line_number(e.line_numbers, window, cx);
                        ed.set_indent_guides(e.indent_guides, window, cx);
                        let tab =
                            super::editor_area::file_tab_size(&f.path, &ed.value(), e.tab_size);
                        ed.set_tab_size(tab, cx);
                    });
                }
            }
        }
        self.persist();
        cx.refresh_windows();
        cx.notify();
    }

    pub fn toggle_word_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.editor.word_wrap = !self.settings.editor.word_wrap;
        self.apply_editor_settings(window, cx);
    }

    fn save_shell(&mut self, cx: &mut Context<Self>) {
        let v = self.settings_form.shell.read(cx).value().trim().to_string();
        let new = (!v.is_empty()).then_some(v);
        if new != self.settings.shell {
            self.settings.shell = new;
            self.persist();
            self.toast("Shell saved. New terminal sessions will use it.", false, cx);
        }
    }

    fn save_tool_path(&mut self, cx: &mut Context<Self>) {
        let name = self
            .settings_form
            .tool_name
            .read(cx)
            .value()
            .trim()
            .to_string();
        let path = self
            .settings_form
            .tool_path
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty() {
            return;
        }
        if path.is_empty() {
            self.settings.tool_paths.remove(&name);
        } else {
            self.settings.tool_paths.insert(name, PathBuf::from(path));
        }
        tools::set_overrides(self.settings.tool_paths.clone());
        self.persist();
        self.run_tool_probes(cx);
    }

    fn save_python(&mut self, cx: &mut Context<Self>) {
        let v = self
            .settings_form
            .python
            .read(cx)
            .value()
            .trim()
            .to_string();
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        self.settings.session_mut(&root).python = (!v.is_empty()).then(|| PathBuf::from(v));
        self.persist();
        self.refresh_run_configs(cx);
        self.toast(
            "Interpreter saved. Reopen Python files to apply it to the language server.",
            false,
            cx,
        );
    }

    fn add_association(&mut self, cx: &mut Context<Self>) {
        let pattern = self
            .settings_form
            .assoc_pattern
            .read(cx)
            .value()
            .trim()
            .to_string();
        let id = self
            .settings_form
            .assoc_language
            .read(cx)
            .value()
            .trim()
            .to_string();
        // The id (`csharp`) or the name shown in the list (`C#`), any case.
        let language = lang::by_id(&id).or_else(|| {
            LANGUAGES
                .iter()
                .find(|l| l.display.eq_ignore_ascii_case(&id) || l.id.eq_ignore_ascii_case(&id))
                .copied()
        });
        let Some(language) = language.filter(|_| !pattern.is_empty()) else {
            self.toast(
                "Enter a file pattern and a language from the list below, by name or id",
                true,
                cx,
            );
            return;
        };
        self.settings
            .file_associations
            .insert(pattern, language.id.to_string());
        self.persist();
        cx.notify();
    }

    fn add_custom_lsp(&mut self, cx: &mut Context<Self>) {
        let f = &self.settings_form;
        let entry = CustomLsp {
            name: f.lsp_name.read(cx).value().trim().to_string(),
            command: f.lsp_cmd.read(cx).value().trim().to_string(),
            args: shlex::split(f.lsp_args.read(cx).value().as_ref()).unwrap_or_default(),
            extensions: words(f.lsp_ext.read(cx).value().as_ref()),
            root_markers: words(f.lsp_roots.read(cx).value().as_ref()),
        };
        if entry.name.is_empty() || entry.command.is_empty() || entry.extensions.is_empty() {
            self.toast("Name, command and extensions are required", true, cx);
            return;
        }
        self.settings.custom_lsp.retain(|c| c.name != entry.name);
        self.settings.custom_lsp.push(entry);
        self.persist();
        cx.notify();
    }

    fn switch(&self, id: &'static str, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = pal(cx);
        let focus = self.keyboard_stop(id, cx);
        div()
            .id(id)
            .track_focus(&focus)
            .focus_visible(move |s| s.border_2().border_color(p.accent))
            .flex()
            .flex_shrink_0()
            .items_center()
            .w(px(34.))
            .h(px(18.))
            .p(px(2.))
            .rounded(px(9.))
            .bg(if on { p.accent_fill } else { p.active })
            .cursor_pointer()
            .when(on, |d| d.justify_end())
            .child(
                div()
                    .size(px(14.))
                    .rounded_full()
                    .bg(if on { p.accent_fg } else { p.editor }),
            )
    }

    /// A persistent Tab stop for a hand-built control; Enter and Space then click it.
    fn keyboard_stop(&self, id: impl Into<ElementId>, cx: &mut Context<Self>) -> FocusHandle {
        self.settings_form
            .stops
            .borrow_mut()
            .entry(id.into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    fn stepper(
        &self,
        id: &'static str,
        value: String,
        on: impl Fn(&mut Self, i32, &mut Window, &mut Context<Self>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = pal(cx);
        let on2 = on.clone();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .child(
                button((id, 0usize), "−", p)
                    .on_click(cx.listener(move |this, _, w, cx| on(this, -1, w, cx))),
            )
            .child(
                div()
                    .w(px(40.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .child(value),
            )
            .child(
                button((id, 1usize), "+", p)
                    .on_click(cx.listener(move |this, _, w, cx| on2(this, 1, w, cx))),
            )
    }

    fn stacked(&self) -> bool {
        self.settings_form.content_width < Layout::STACK
    }

    /// A text field's slot: its natural width beside the title, or a share of
    /// the row once controls go under the description.
    fn field(&self, width: f32) -> Div {
        if self.stacked() {
            div().flex_1().min_w(px(64.))
        } else {
            div().w(px(width)).flex_shrink_0()
        }
    }

    /// A line of controls; it spans the row when stacked so buttons stay visible.
    fn controls(&self) -> Div {
        div()
            .flex()
            .items_center()
            .gap_1()
            .when(self.stacked(), |d| d.w_full())
    }

    /// Copies a value that may be longer than its field, such as a path.
    fn copy_button(
        &self,
        id: impl Into<ElementId>,
        value: String,
    ) -> gpui_kit::component::button::Button {
        icon_button(id, "copy", "Copy Full Value").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()))
        })
    }

    /// A settings row, or nothing when the search query excludes it.
    fn row(
        &self,
        title: &str,
        description: &str,
        scope: Scope,
        control: impl IntoElement,
        cx: &App,
    ) -> Option<AnyElement> {
        let p = pal(cx);
        let q = self.settings_form.query.read(cx).value().to_lowercase();
        if !q.is_empty()
            && !title.to_lowercase().contains(q.as_ref() as &str)
            && !description.to_lowercase().contains(q.as_ref() as &str)
        {
            return None;
        }
        let tag = match scope {
            Scope::Global => "Global",
            Scope::Project => "This project",
        };
        let stacked = self.stacked();
        Some(
            div()
                .flex()
                .when(stacked, |d| d.flex_col().items_start().gap_2())
                .when(!stacked, |d| d.items_center().gap_3())
                .py_2()
                .border_b_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(13.))
                                .child(title.to_string())
                                .child(
                                    div()
                                        .px_1()
                                        .rounded(px(3.))
                                        .bg(p.hover)
                                        .text_size(px(10.))
                                        .text_color(if matches!(scope, Scope::Project) {
                                            p.accent
                                        } else {
                                            p.muted
                                        })
                                        .child(tag),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(p.muted)
                                .child(description.to_string()),
                        ),
                )
                .child(if stacked {
                    div().w_full().flex().child(control).into_any_element()
                } else {
                    control.into_any_element()
                })
                .into_any_element(),
        )
    }

    fn editor_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let e = self.settings.editor.clone();
        let family = self.settings_form.font_family.clone();
        let mut rows = vec![];
        rows.push(self.row(
            "Font Family",
            "Applied on Enter. Use a monospaced family.",
            Scope::Global,
            self.field(200.).child(Input::new(&family)),
            cx,
        ));
        let c = self.stepper(
            "font-size",
            format!("{}", e.font_size),
            |t, d, w, cx| {
                t.settings.editor.font_size =
                    (t.settings.editor.font_size + d as f32).clamp(9., 32.);
                t.apply_editor_settings(w, cx);
            },
            cx,
        );
        rows.push(self.row(
            "Font Size",
            "Code size in the editor.",
            Scope::Global,
            c,
            cx,
        ));
        let c = self
            .switch("sw-lines", e.line_numbers, cx)
            .on_click(cx.listener(|t, _, w, cx| {
                t.settings.editor.line_numbers = !t.settings.editor.line_numbers;
                t.apply_editor_settings(w, cx);
            }));
        rows.push(self.row(
            "Line Numbers",
            "Show real file line numbers in the gutter.",
            Scope::Global,
            c,
            cx,
        ));
        let c = self
            .switch("sw-wrap", e.word_wrap, cx)
            .on_click(cx.listener(|t, _, w, cx| t.toggle_word_wrap(w, cx)));
        rows.push(self.row(
            "Word Wrap",
            &format!(
                "{}. Off: lines scroll horizontally. On: the number stays on the first visual row.",
                super::keys::WORD_WRAP.label()
            ),
            Scope::Global,
            c,
            cx,
        ));
        let c = self.stepper(
            "tab-size",
            format!("{}", e.tab_size),
            |t, d, w, cx| {
                t.settings.editor.tab_size =
                    (t.settings.editor.tab_size as i32 + d).clamp(1, 8) as usize;
                t.apply_editor_settings(w, cx);
            },
            cx,
        );
        rows.push(self.row(
            "Tab Size",
            "Used when a file has no indentation of its own; also the width of tab characters.",
            Scope::Global,
            c,
            cx,
        ));
        let c = self
            .switch("sw-guides", e.indent_guides, cx)
            .on_click(cx.listener(|t, _, w, cx| {
                t.settings.editor.indent_guides = !t.settings.editor.indent_guides;
                t.apply_editor_settings(w, cx);
            }));
        rows.push(self.row(
            "Indent Guides",
            "Vertical guides at indentation levels.",
            Scope::Global,
            c,
            cx,
        ));
        let c = self
            .switch("sw-active", e.active_line, cx)
            .on_click(cx.listener(|t, _, w, cx| {
                t.settings.editor.active_line = !t.settings.editor.active_line;
                t.apply_editor_settings(w, cx);
            }));
        rows.push(self.row(
            "Highlight Active Line",
            "Tint the line with the cursor.",
            Scope::Global,
            c,
            cx,
        ));
        rows.into_iter().flatten().collect()
    }

    fn appearance_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = pal(cx);
        let cur = self.settings.theme;
        let opt =
            |id: &'static str, label: &'static str, v: ThemeChoice, cx: &mut Context<Self>| {
                (if cur == v {
                    primary_button(id, label, p)
                } else {
                    button(id, label, p)
                })
                .on_click(cx.listener(move |t, _, _, cx| t.set_theme(v, cx)))
            };
        let control = div()
            .flex()
            .gap_1()
            .child(opt("t-dark", "Graphite", ThemeChoice::Dark, cx))
            .child(opt("t-light", "Cream", ThemeChoice::Light, cx))
            .child(opt("t-system", "System", ThemeChoice::System, cx));
        self.row(
            "Theme",
            "System follows the light or dark appearance of the OS while KERN is running.",
            Scope::Global,
            control,
            cx,
        )
        .into_iter()
        .collect()
    }

    fn terminal_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = self.settings.terminal.clone();
        let shell = self.settings_form.shell.clone();
        let mut rows = vec![];
        rows.push(self.row(
            "Shell",
            "For new sessions. Applied on Enter.",
            Scope::Global,
            self.field(240.).child(Input::new(&shell)),
            cx,
        ));
        let c = self.stepper(
            "term-size",
            format!("{}", t.font_size),
            |a, d, w, cx| {
                a.settings.terminal.font_size =
                    (a.settings.terminal.font_size + d as f32).clamp(9., 28.);
                a.apply_editor_settings(w, cx);
            },
            cx,
        );
        rows.push(self.row(
            "Font Size",
            "Terminal and Claude Code tabs.",
            Scope::Global,
            c,
            cx,
        ));
        let c = self.stepper(
            "term-scroll",
            format!("{}", t.scrollback),
            |a, d, w, cx| {
                let v = a.settings.terminal.scrollback as i64 + d as i64 * 5000;
                a.settings.terminal.scrollback = v.clamp(1000, 100_000) as usize;
                a.apply_editor_settings(w, cx);
            },
            cx,
        );
        rows.push(self.row(
            "Scrollback Lines",
            "For new sessions.",
            Scope::Global,
            c,
            cx,
        ));
        rows.into_iter().flatten().collect()
    }

    fn languages_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = pal(cx);
        let mut rows: Vec<AnyElement> = vec![];
        let has_project = self.project.is_some();
        if has_project {
            let py = self.settings_form.python.clone();
            let full = py.read(cx).value().to_string();
            let c = self
                .controls()
                .child(self.field(240.).child(Input::new(&py)))
                .child(self.copy_button("copy-py", full))
                .child(
                    button("save-py", "Save", p)
                        .on_click(cx.listener(|t, _, _, cx| t.save_python(cx))),
                );
            rows.extend(self.row(
                "Python Interpreter",
                "Used by Run and by the Python language server.",
                Scope::Project,
                c,
                cx,
            ));
            let trusted = self.project.as_ref().is_some_and(|p| p.trusted);
            let c = if trusted {
                div()
                    .text_size(px(12.))
                    .text_color(p.success)
                    .child("Trusted")
                    .into_any_element()
            } else {
                primary_button("trust-set", "Trust This Folder", p)
                    .on_click(cx.listener(|t, _, w, cx| t.trust_project(w, cx)))
                    .into_any_element()
            };
            rows.extend(self.row(
                "Language Servers in This Folder",
                "Servers can run project code, so each folder needs your permission.",
                Scope::Project,
                c,
                cx,
            ));
        }
        let (pat, lang_id) = (
            self.settings_form.assoc_pattern.clone(),
            self.settings_form.assoc_language.clone(),
        );
        let c = self
            .controls()
            .child(self.field(150.).child(Input::new(&pat)))
            .child(self.field(150.).child(Input::new(&lang_id)))
            .child(
                button("add-assoc", "Add", p)
                    .on_click(cx.listener(|t, _, _, cx| t.add_association(cx))),
            );
        rows.extend(self.row(
            "File Associations",
            "Also set by Change Language Mode from the status bar.",
            Scope::Global,
            c,
            cx,
        ));
        for (i, (k, v)) in self
            .settings
            .file_associations
            .clone()
            .into_iter()
            .enumerate()
        {
            let key = k.clone();
            rows.push(
                div()
                    .id(("assoc", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(24.))
                    .text_size(px(12.))
                    .child(
                        // Long patterns and language names wrap instead of hiding Remove.
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family("Menlo")
                            .child(format!("{k} → {v}")),
                    )
                    .child(
                        icon_button(("assoc-del", i), "close", "Remove").on_click(cx.listener(
                            move |t, _, _, cx| {
                                t.settings.file_associations.remove(&key);
                                t.persist();
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }
        // The support matrix: what KERN can do vs. what is present on this Mac.
        let q = self.settings_form.query.read(cx).value().to_lowercase();
        let probes = self.settings_form.probes.clone();
        let cell = |t: String, w: f32, color: Hsla| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(color)
                .child(t)
        };
        // Wide: the five-column table. Narrow: one block per language, so Run and
        // long install hints never end up past the edge.
        let table = self.settings_form.content_width >= Layout::TABLE;
        rows.push(
            div()
                .flex()
                .gap_2()
                .pt_3()
                .pb_1()
                .border_b_1()
                .border_color(p.border)
                .when(!table, |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .child("Languages: highlighting, language server and Run"),
                    )
                })
                .when(table, |d| d.child(cell("Language".into(), 90., p.muted)))
                .when(table, |d| {
                    d.child(cell("Files".into(), 170., p.muted))
                        .child(cell("Highlight".into(), 76., p.muted))
                        .child(cell("Language server".into(), 230., p.muted))
                        .child(cell("Run".into(), 150., p.muted))
                })
                .into_any_element(),
        );
        for l in LANGUAGES {
            if !q.is_empty()
                && !l.display.to_lowercase().contains(q.as_ref() as &str)
                && !l.id.contains(q.as_ref() as &str)
            {
                continue;
            }
            let mut files: Vec<String> = l.extensions.iter().map(|e| format!(".{e}")).collect();
            files.extend(l.filenames.iter().map(|f| f.to_string()));
            let highlight = match l.highlight {
                Highlight::TreeSitter => "Tree-sitter",
                Highlight::Lexer => "KERN lexer",
                Highlight::None => "none",
            };
            let (server, color) = match l.server {
                None => ("no server in KERN".to_string(), p.muted),
                Some(s) => {
                    let info = s.info();
                    let found = info.commands.iter().find_map(|c| match probes.get(*c) {
                        Some(Probe::Ready { .. }) => Some(c.to_string()),
                        _ => None,
                    });
                    match found {
                        Some(c) => (format!("{} · found: {c}", info.id), p.success),
                        None if probes.is_empty() => (format!("{} · checking…", info.id), p.muted),
                        // TypeScript usually comes from the project, not from this Mac.
                        None if s == lang::BuiltinServer::TypeScript => (
                            "project's TypeScript, or npm install -g typescript".to_string(),
                            p.muted,
                        ),
                        None => (
                            format!("{} · not on this Mac: {}", info.id, info.install_hint),
                            p.warning,
                        ),
                    }
                }
            };
            if !table {
                let detail = |label: &'static str, value: String, color: Hsla| {
                    div()
                        .flex()
                        .gap_2()
                        .text_size(px(11.))
                        .child(
                            div()
                                .w(px(104.))
                                .flex_shrink_0()
                                .text_color(p.muted)
                                .child(label),
                        )
                        .child(div().flex_1().min_w_0().text_color(color).child(value))
                };
                rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .py_1p5()
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .items_baseline()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(l.display.to_string()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(p.muted)
                                        .font_family("Menlo")
                                        .child(l.id),
                                ),
                        )
                        .child(detail("Files", files.join(" "), p.fg))
                        .child(detail(
                            "Highlight",
                            if l.note.is_empty() {
                                highlight.to_string()
                            } else {
                                format!("{highlight} · {}", l.note)
                            },
                            p.fg,
                        ))
                        .child(detail("Language server", server, color))
                        .child(detail("Run", l.run.label().to_string(), p.fg))
                        .into_any_element(),
                );
                continue;
            }
            rows.push(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .w(px(90.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(11.)).child(l.display.to_string()))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(p.muted)
                                    .font_family("Menlo")
                                    .child(l.id),
                            ),
                    )
                    .child(cell(files.join(" "), 170., p.muted))
                    .child(
                        div()
                            .w(px(76.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(11.)).child(highlight))
                            .when(!l.note.is_empty(), |d| {
                                d.child(
                                    div().text_size(px(10.)).text_color(p.warning).child(l.note),
                                )
                            }),
                    )
                    .child(cell(server, 230., color))
                    .child(cell(l.run.label().to_string(), 150., p.muted))
                    .into_any_element(),
            );
        }
        // Custom servers.
        let (ln, lc, la, le, lr) = {
            let f = &self.settings_form;
            (
                f.lsp_name.clone(),
                f.lsp_cmd.clone(),
                f.lsp_args.clone(),
                f.lsp_ext.clone(),
                f.lsp_roots.clone(),
            )
        };
        let form = div()
            .flex()
            .flex_col()
            .gap_1()
            .when(self.stacked(), |d| d.w_full())
            .when(!self.stacked(), |d| d.w(px(260.)))
            .child(Input::new(&ln))
            .child(Input::new(&lc))
            .child(Input::new(&la))
            .child(Input::new(&le))
            .child(Input::new(&lr))
            .child(
                div().flex().child(
                    button("add-lsp", "Add Server", p)
                        .on_click(cx.listener(|t, _, _, cx| t.add_custom_lsp(cx))),
                ),
            );
        rows.extend(self.row(
            "Custom Language Server",
            "Command, arguments, file extensions, root markers.",
            Scope::Global,
            form,
            cx,
        ));
        for (i, c) in self.settings.custom_lsp.clone().into_iter().enumerate() {
            let name = c.name.clone();
            rows.push(
                div()
                    .id(("clsp", i))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().text_size(px(12.)).child(format!(
                        "{}: {} {} [{}]",
                        c.name,
                        c.command,
                        c.args.join(" "),
                        c.extensions.join(", ")
                    )))
                    .child(
                        icon_button(("clsp-del", i), "close", "Remove").on_click(cx.listener(
                            move |t, _, _, cx| {
                                t.settings.custom_lsp.retain(|x| x.name != name);
                                t.persist();
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn git_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let visible = !self.settings.graph_hidden;
        let all = self.settings.graph_all_branches;
        let a = self
            .switch("sw-graph", visible, cx)
            .on_click(cx.listener(|t, _, _, cx| t.toggle_graph(cx)));
        let b = self.switch("sw-all", all, cx).on_click(
            cx.listener(|t, _, _, cx| t.set_graph_all(!t.settings.graph_all_branches, cx)),
        );
        [
            self.row(
                "Show Graph",
                "Commit graph under Changes in Source Control.",
                Scope::Global,
                a,
                cx,
            ),
            self.row(
                "Graph: All Branches",
                "Off shows the current branch only.",
                Scope::Global,
                b,
                cx,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn tools_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = pal(cx);
        let mut rows: Vec<AnyElement> = vec![];
        let probing = self.settings_form.probing;
        let stacked = self.stacked();
        let (tn, tp) = (
            self.settings_form.tool_name.clone(),
            self.settings_form.tool_path.clone(),
        );
        rows.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_2()
                .when(stacked, |d| d.flex_col().items_start())
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(p.muted).child("Each tool is actually started with --version. Being on PATH alone is not enough."))
                .child(button("check-tools", if probing { "Checking…" } else { "Check Again" }, p).on_click(cx.listener(|t, _, _, cx| t.run_tool_probes(cx))))
                .into_any_element(),
        );
        let overrides = self.settings.tool_paths.clone();
        let q = self.settings_form.query.read(cx).value().to_lowercase();
        for (i, name) in tools_to_probe().into_iter().enumerate() {
            if !q.is_empty() && !name.contains(q.as_ref() as &str) {
                continue;
            }
            let hint = tools::install_hint(&name).to_string();
            let hint = if hint.is_empty() {
                LANGUAGES
                    .iter()
                    .filter_map(|l| l.server)
                    .find(|s| s.info().commands.contains(&name.as_str()))
                    .map(|s| s.info().install_hint.to_string())
                    .unwrap_or_default()
            } else {
                hint
            };
            let found_path = match self.settings_form.probes.get(&name) {
                Some(Probe::Ready { path, .. }) | Some(Probe::Broken { path, .. }) => {
                    Some(path.display().to_string())
                }
                _ => None,
            };
            let name_w = if self.stacked() { 110. } else { 190. };
            let (status, color) = match self.settings_form.probes.get(&name) {
                None => ("checking…".to_string(), p.muted),
                Some(Probe::Ready { path, version }) if version.is_empty() => {
                    (path.display().to_string(), p.fg)
                }
                Some(Probe::Ready { path, version }) => {
                    (format!("{} · {version}", path.display()), p.fg)
                }
                Some(Probe::Broken { path, reason }) => (
                    format!("cannot run {}: {reason} · {hint}", path.display()),
                    p.danger,
                ),
                Some(Probe::Missing) => (format!("not found · {hint}"), p.warning),
            };
            rows.push(
                div()
                    .id(("tool", i))
                    .flex()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .w(px(name_w))
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .child(if overrides.contains_key(&name) {
                                format!("{name} (custom path)")
                            } else {
                                name.clone()
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(color)
                            .child(status),
                    )
                    .when_some(found_path, |d, path| {
                        d.child(self.copy_button(("tool-copy", i), path))
                    })
                    .into_any_element(),
            );
        }
        let form = self
            .controls()
            .child(self.field(130.).child(Input::new(&tn)))
            .child(self.field(220.).child(Input::new(&tp)))
            .child(
                button("save-tool", "Apply", p)
                    .on_click(cx.listener(|t, _, _, cx| t.save_tool_path(cx))),
            );
        rows.extend(self.row(
            "Tool Path",
            "Use this binary instead of searching PATH.",
            Scope::Global,
            form,
            cx,
        ));
        rows
    }

    pub fn render_settings_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        // The tab's real width: the window minus the activity bar and whatever
        // panels are open beside the editor area.
        let l = &self.settings.layout;
        let mut center = l.window_width - 44.;
        if self.project.is_some() {
            if l.sidebar_visible {
                center -= l.sidebar_width + 2.;
            }
            if l.codex_visible {
                center -= l.codex_width + 2.;
            }
        }
        let categories_w = if center < 560. { 112. } else { 150. };
        // Category list, its border, the body's padding and a scrollbar.
        self.settings_form.content_width = (center - categories_w - 1. - 40. - 8.).max(160.);
        let query_empty = self.settings_form.query.read(cx).value().is_empty();
        let current = self.settings_form.category;
        // A search spans all categories; otherwise only the selected one is shown.
        let mut sections: Vec<(&'static str, Vec<AnyElement>)> = vec![];
        for (cat, name) in CATEGORIES {
            if !query_empty || *cat == current {
                let rows = match cat {
                    Category::Editor => self.editor_rows(cx),
                    Category::Appearance => self.appearance_rows(cx),
                    Category::Terminal => self.terminal_rows(cx),
                    Category::Languages => self.languages_rows(cx),
                    Category::Git => self.git_rows(cx),
                    Category::Extensions => self.extensions_settings_rows(cx),
                    Category::Tools => self.tools_rows(cx),
                };
                if !rows.is_empty() {
                    sections.push((name, rows));
                }
            }
        }
        let empty = sections.is_empty();
        let query = self.settings_form.query.clone();
        div()
            .key_context("KernSettings")
            .size_full()
            .flex()
            .flex_col()
            .bg(p.editor)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Settings"),
                    )
                    .child(div().flex_1().max_w(px(420.)).child(Input::new(&query)))
                    .child(div().flex_1())
                    .when(self.project.is_none(), |d| {
                        d.child(button("settings-close", "Close", p).on_click(cx.listener(
                            |t, _, _, cx| {
                                t.settings_standalone = false;
                                cx.notify();
                            },
                        )))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(categories_w))
                            .flex_shrink_0()
                            .py_2()
                            .border_r_1()
                            .border_color(p.border)
                            .children(CATEGORIES.iter().enumerate().map(|(i, (cat, name))| {
                                let cat = *cat;
                                let on = cat == current && query_empty;
                                let stop = self.keyboard_stop(("cat", i), cx);
                                div()
                                    .id(("cat", i))
                                    .track_focus(&stop)
                                    .focus_visible(move |s| s.bg(p.hover).text_color(p.accent))
                                    .h(px(26.))
                                    .flex()
                                    .items_center()
                                    .px_3()
                                    .text_size(px(12.))
                                    .text_color(if on { p.accent } else { p.fg })
                                    .when(on, |d| d.bg(p.accent_soft))
                                    .hover(move |s| s.bg(p.hover))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |t, _, _, cx| {
                                        t.settings_form.category = cat;
                                        if (cat == Category::Tools || cat == Category::Languages)
                                            && t.settings_form.probes.is_empty()
                                        {
                                            t.run_tool_probes(cx);
                                        }
                                        cx.notify();
                                    }))
                                    .child(*name)
                            })),
                    )
                    .child(
                        div()
                            .id("settings-body")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .px_5()
                            .py_2()
                            .children(sections.into_iter().map(|(name, rows)| {
                                div()
                                    .pb_4()
                                    .child(
                                        div()
                                            .pt_2()
                                            .pb_1()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(name),
                                    )
                                    .children(rows)
                            }))
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .pt_4()
                                        .text_size(px(12.))
                                        .text_color(p.muted)
                                        .child("No settings match this search."),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}
