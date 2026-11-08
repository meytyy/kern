//! Tabs and the code editor: open, edit, save, close, external changes.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use super::*;
use crate::core::document::{DocumentState, ExternalAction, SaveOutcome};
use crate::lang::{self, Language};
use crate::lsp::{self, ServerStatus};
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct FileTab {
    pub path: PathBuf,
    pub doc: DocumentState,
    pub editor: Entity<EditorState>,
    pub lang: Language,
    pub dirty: bool,
    pub conflict: Option<Conflict>,
    pub version: i32,
    pub lsp: Option<Arc<crate::lsp::LspClient>>,
    pub lsp_failed: Option<(String, String)>,
    pub diagnostic_counts: (usize, usize),
    pub pull_generation: u64,
    /// Diagnostics a server pushed (rust-analyzer: cargo check) and the ones
    /// KERN pulled (its own analysis). Shown together; each source replaces
    /// only its own set.
    pub pushed_diagnostics: Vec<lsp_types::Diagnostic>,
    pub pulled_diagnostics: Vec<lsp_types::Diagnostic>,
    pub _sub: Subscription,
    pub _obs: Subscription,
}

#[derive(Clone)]
pub enum Conflict {
    /// Disk and buffer both changed.
    Changed {
        disk_text: String,
        disk_format: crate::core::textfile::FileFormat,
    },
    /// Deleted on disk while the buffer has content.
    Deleted,
}

pub struct DiffTab {
    pub key: String,
    pub title: String,
    pub state: Entity<DiffState>,
    pub patch: String,
    pub note: Option<String>,
}

pub enum Tab {
    File(Box<FileTab>),
    Diff(DiffTab),
    Settings,
    Extension(String),
    Media(Entity<super::media::MediaView>, PathBuf),
}

impl Tab {
    pub fn title(&self) -> String {
        match self {
            Tab::File(f) => f
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            Tab::Diff(d) => d.title.clone(),
            Tab::Settings => "Settings".into(),
            Tab::Extension(id) => format!("Extension: {}", super::extensions::display_name(id)),
            Tab::Media(_, path) => path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        }
    }
    pub fn is_dirty(&self) -> bool {
        matches!(self, Tab::File(f) if f.dirty)
    }
    pub fn file_path(&self) -> Option<&Path> {
        match self {
            Tab::File(f) => Some(&f.path),
            Tab::Media(_, path) => Some(path),
            _ => None,
        }
    }
    pub fn as_file(&self) -> Option<&FileTab> {
        match self {
            Tab::File(f) => Some(f),
            _ => None,
        }
    }
}

impl KernApp {
    pub fn active_file(&self) -> Option<&FileTab> {
        let p = self.project.as_ref()?;
        p.tabs.get(p.active?)?.as_file()
    }

    /// Open a file off the UI thread; optionally jump to a 0-based (line, char).
    pub fn open_file_async(
        &mut self,
        path: PathBuf,
        activate: bool,
        jump: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.project else { return };
        if let Some(ix) = p
            .tabs
            .iter()
            .position(|t| t.file_path() == Some(path.as_path()))
        {
            if activate {
                p.active = Some(ix);
            }
            if let (Some(pos), Tab::File(f)) = (jump, &p.tabs[ix]) {
                let editor = f.editor.clone();
                editor.update(cx, |e, cx| {
                    e.set_cursor_position(lsp_types::Position::new(pos.0, pos.1), window, cx)
                });
            } else if activate {
                let editor = p.tabs[ix].as_file().map(|f| f.editor.clone());
                if let Some(e) = editor {
                    e.update(cx, |e, cx| e.focus(window, cx));
                }
            }
            self.persist();
            cx.notify();
            return;
        }
        // Pictures and video open in the same place, in a read-only tab.
        if let Some(kind) = super::media::kind_for(&path) {
            let view_path = path.clone();
            let view = cx.new(|cx| super::media::MediaView::new(view_path, kind, cx));
            cx.observe(&view, |_, _, cx| cx.notify()).detach();
            p.tabs.push(Tab::Media(view, path));
            if activate || p.active.is_none() {
                p.active = Some(p.tabs.len() - 1);
            }
            self.persist();
            cx.notify();
            return;
        }
        let task_path = path.clone();
        let load = cx.background_spawn(async move { DocumentState::load(&task_path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((doc, text)) => this.add_file_tab(doc, text, activate, jump, window, cx),
                // Not text: the file opens in a read-only tab that says what it is.
                Err(e) if e.to_string().contains("binary") => {
                    this.open_binary_tab(path.clone(), cx)
                }
                Err(e) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    log::warn!("cannot open {}: {e}", path.display());
                    this.toast(format!("Cannot open {name}: {e}"), true, cx)
                }
            })
            .ok();
        })
        .detach();
    }

    /// A binary file that is not an image, video or archive: a tab with its type
    /// and size, and buttons to open or show it with the system.
    fn open_binary_tab(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        if let Some(ix) = p
            .tabs
            .iter()
            .position(|t| t.file_path() == Some(path.as_path()))
        {
            p.active = Some(ix);
            cx.notify();
            return;
        }
        let view_path = path.clone();
        let view = cx
            .new(|cx| super::media::MediaView::new(view_path, super::media::MediaKind::Binary, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        p.tabs.push(Tab::Media(view, path));
        p.active = Some(p.tabs.len() - 1);
        self.persist();
        cx.notify();
    }

    fn add_file_tab(
        &mut self,
        doc: DocumentState,
        text: String,
        activate: bool,
        jump: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.project else { return };
        let path = doc.path.clone();
        if !path.starts_with(&p.root) && !path.exists() {
            return;
        }
        // Opened again while it was loading (a restored session and a click in
        // Explorer, a double click): one tab per file.
        if p.tabs.iter().any(|t| t.file_path() == Some(path.as_path())) {
            self.open_file_async(path, activate, jump, window, cx);
            return;
        }
        let root = p.root.clone();
        let lang = lang::resolve(&path, Some(&root), &self.settings);
        let editor = new_editor(&path, lang, text.clone(), &self.settings, window, cx);
        let Some(p) = &mut self.project else { return };
        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
        let obs = cx.observe(&editor, |_, _, cx| cx.notify());
        let tab = FileTab {
            path: path.clone(),
            doc,
            editor: editor.clone(),
            lang,
            dirty: false,
            conflict: None,
            version: 1,
            lsp: None,
            lsp_failed: None,
            diagnostic_counts: (0, 0),
            pull_generation: 0,
            pushed_diagnostics: vec![],
            pulled_diagnostics: vec![],
            _sub: sub,
            _obs: obs,
        };
        p.tabs.push(Tab::File(Box::new(tab)));
        if activate || p.active.is_none() {
            p.active = Some(p.tabs.len() - 1);
            editor.update(cx, |e, cx| e.focus(window, cx));
        }
        self.attach_lsp(&path, text, window, cx);
        if let Some(pos) = jump {
            editor.update(cx, |e, cx| {
                e.set_cursor_position(lsp_types::Position::new(pos.0, pos.1), window, cx)
            });
        }
        self.persist();
        cx.notify();
    }

    fn on_editor_event(
        &mut self,
        editor: &Entity<EditorState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(ev, InputEvent::Change) {
            return;
        }
        let Some(p) = &mut self.project else { return };
        let text = editor.read(cx).text().to_string();
        let mut changed = None;
        for tab in &mut p.tabs {
            if let Tab::File(f) = tab
                && f.editor == *editor
            {
                f.dirty = f.doc.is_dirty(&text);
                f.version += 1;
                if let Some(client) = &f.lsp {
                    client.did_change(&lsp::path_to_uri(&f.path), f.version, text);
                    changed = Some(f.path.clone());
                }
                break;
            }
        }
        if let Some(path) = changed {
            self.schedule_pull_diagnostics(path, cx);
        }
        self.signature_after_edit(editor, window, cx);
        cx.notify();
    }

    pub fn save_active(&mut self, force: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let Some(ix) = p.active else { return };
        self.save_tab(ix, force, cx);
    }

    pub fn save_tab(&mut self, ix: usize, force: bool, cx: &mut Context<Self>) -> bool {
        let Some(p) = &mut self.project else {
            return false;
        };
        let root = p.root.clone();
        let Some(Tab::File(f)) = p.tabs.get_mut(ix) else {
            return false;
        };
        if f.conflict.is_some() && !force {
            self.toast(
                "Resolve the file conflict first: see the bar above the editor",
                true,
                cx,
            );
            return false;
        }
        let text = f.editor.read(cx).text().to_string();
        let path = f.path.clone();
        let result = f.doc.save(&text, force);
        match result {
            Ok(SaveOutcome::Saved) => {
                f.dirty = false;
                f.conflict = None;
                if let Some(c) = &f.lsp {
                    c.did_save(&lsp::path_to_uri(&f.path));
                }
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                self.agent_note_user_save(rel);
                self.refresh_git(cx);
                cx.notify();
                true
            }
            Ok(SaveOutcome::DiskChanged) => {
                // Surface as a conflict: never overwrite silently.
                if let Ok(ExternalAction::Conflict {
                    disk_text,
                    disk_format,
                }) = f.doc.check_external(&text)
                {
                    f.conflict = Some(Conflict::Changed {
                        disk_text,
                        disk_format,
                    });
                } else {
                    f.conflict = Some(Conflict::Changed {
                        disk_text: std::fs::read_to_string(&path).unwrap_or_default(),
                        disk_format: f.doc.format.clone(),
                    });
                }
                self.toast(
                    "The file changed on disk. Choose which version to keep.",
                    true,
                    cx,
                );
                false
            }
            Err(e) => {
                self.toast(format!("{e:#}"), true, cx);
                false
            }
        }
    }

    pub fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        let n = self.project.as_ref().map_or(0, |p| p.tabs.len());
        let mut ok = true;
        for ix in 0..n {
            if self.project.as_ref().is_some_and(|p| p.tabs[ix].is_dirty()) {
                ok &= self.save_tab(ix, false, cx);
            }
        }
        ok
    }

    /// Re-check open files against the disk after a change notification.
    pub fn check_open_files(
        &mut self,
        changed: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.project else { return };
        let mut did_change = false;
        for tab in &mut p.tabs {
            let Tab::File(f) = tab else { continue };
            let relevant = changed
                .iter()
                .any(|c| *c == f.path || f.path.starts_with(c));
            if !relevant {
                continue;
            }
            let buffer = f.editor.read(cx).text().to_string();
            match f.doc.check_external(&buffer) {
                Ok(ExternalAction::None) => {}
                Ok(ExternalAction::Reload { text, format }) => {
                    f.doc.accept_disk(text.clone(), format);
                    set_editor_text(&f.editor, text, window, cx);
                    f.dirty = false;
                    f.conflict = None;
                    did_change = true;
                }
                Ok(ExternalAction::Conflict {
                    disk_text,
                    disk_format,
                }) => {
                    f.conflict = Some(Conflict::Changed {
                        disk_text,
                        disk_format,
                    });
                    did_change = true;
                }
                Ok(ExternalAction::DeletedClean | ExternalAction::DeletedDirty) => {
                    f.conflict = Some(Conflict::Deleted);
                    did_change = true;
                }
                Ok(ExternalAction::Unreadable(why)) => {
                    log::warn!("{}: {why}", f.path.display());
                }
                Err(e) => log::warn!("{}: {e}", f.path.display()),
            }
        }
        if did_change {
            cx.notify();
        }
    }

    pub fn conflict_load_disk(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let Some(Tab::File(f)) = p.tabs.get_mut(ix) else {
            return;
        };
        if let Some(Conflict::Changed {
            disk_text,
            disk_format,
        }) = f.conflict.take()
        {
            f.doc.accept_disk(disk_text.clone(), disk_format);
            set_editor_text(&f.editor, disk_text, window, cx);
            f.dirty = false;
        }
        cx.notify();
    }

    pub fn conflict_keep_buffer(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let Some(Tab::File(f)) = p.tabs.get_mut(ix) else {
            return;
        };
        // Stop prompting for this disk version; saving still takes an explicit overwrite.
        let _ = f.doc.keep_buffer();
        f.conflict = None;
        f.dirty = true;
        cx.notify();
    }

    pub fn conflict_overwrite(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.save_tab(ix, true, cx);
    }

    pub fn conflict_diff(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let Some(Tab::File(f)) = p.tabs.get(ix) else {
            return;
        };
        let Some(Conflict::Changed { disk_text, .. }) = &f.conflict else {
            return;
        };
        let buffer = f.editor.read(cx).text().to_string();
        let name = f
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let patch = similar::TextDiff::from_lines(disk_text.as_str(), buffer.as_str())
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{name} (disk)"), &format!("b/{name} (editor)"))
            .to_string();
        let patch = format!("diff --git a/{name} b/{name}\n{patch}");
        self.open_diff_tab(
            format!("conflict:{name}"),
            format!("{name} disk ↔ editor"),
            patch,
            None,
            window,
            cx,
        );
    }

    pub fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let Some(tab) = p.tabs.get(ix) else { return };
        if tab.is_dirty() {
            let title = tab.title();
            self.overlay = Some(overlays::Overlay::confirm(
                "Unsaved changes",
                format!("Save changes to {title} before closing?"),
                vec![
                    overlays::ConfirmChoice::new(
                        "Save",
                        overlays::ConfirmAction::SaveAndClose(ix),
                        true,
                    ),
                    overlays::ConfirmChoice::new(
                        "Don't Save",
                        overlays::ConfirmAction::DiscardAndClose(ix),
                        false,
                    ),
                ],
            ));
            cx.notify();
            return;
        }
        self.force_close_tab(ix, window, cx);
    }

    pub fn force_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        if ix >= p.tabs.len() {
            return;
        }
        let tab = p.tabs.remove(ix);
        if let Tab::File(f) = &tab
            && let Some(c) = &f.lsp
        {
            c.did_close(&lsp::path_to_uri(&f.path));
        }
        p.active = if p.tabs.is_empty() {
            None
        } else {
            Some(match p.active {
                Some(a) if a > ix => a - 1,
                Some(a) if a < p.tabs.len() && a != ix => a,
                Some(_) => ix.min(p.tabs.len() - 1),
                None => 0,
            })
        };
        if let Some(Tab::File(f)) = p.active.and_then(|a| p.tabs.get(a)) {
            let e = f.editor.clone();
            e.update(cx, |e, cx| e.focus(window, cx));
        }
        self.persist();
        cx.notify();
    }

    pub fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.project.as_ref().and_then(|p| p.active) {
            self.close_tab(ix, window, cx);
        }
    }

    pub fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        p.active = Some(ix);
        if let Some(Tab::File(f)) = p.tabs.get(ix) {
            let e = f.editor.clone();
            e.update(cx, |e, cx| e.focus(window, cx));
        }
        self.persist();
        cx.notify();
    }

    /// Open (or refresh) a diff tab with a real patch.
    pub fn open_diff_tab(
        &mut self,
        key: String,
        title: String,
        patch: String,
        note: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let files = match DiffFile::parse(&patch) {
            Ok(files) => files,
            Err(e) => {
                self.toast(format!("Cannot show diff: {e}"), true, cx);
                return;
            }
        };
        if files.is_empty() {
            self.toast("No changes to show", false, cx);
            return;
        }
        let Some(p) = &mut self.project else { return };
        if let Some(ix) = p
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::Diff(d) if d.key == key))
        {
            if let Tab::Diff(d) = &mut p.tabs[ix] {
                d.state.update(cx, |s, cx| s.set_files(files, cx));
                d.patch = patch;
                d.note = note;
            }
            p.active = Some(ix);
        } else {
            let state = cx.new(|cx| DiffState::new(files, cx));
            p.tabs.push(Tab::Diff(DiffTab {
                key,
                title,
                state,
                patch,
                note,
            }));
            p.active = Some(p.tabs.len() - 1);
        }
        cx.notify();
    }

    fn render_tab_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = pal(cx);
        let Some(p) = &self.project else {
            return div().into_any_element();
        };
        let active = p.active;
        let mut items: Vec<AnyElement> = vec![];
        for (ix, tab) in p.tabs.iter().enumerate() {
            let selected = active == Some(ix);
            let title = tab.title();
            let dirty = tab.is_dirty();
            let conflicted = matches!(tab, Tab::File(f) if f.conflict.is_some());
            items.push(
                div()
                    .id(("tab", ix))
                    .group("tab")
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .pl_2()
                    .pr_1()
                    .border_r_1()
                    .border_color(palette.border)
                    .when(selected, |d| d.bg(palette.bg))
                    .when(!selected, |d| d.hover(move |s| s.bg(palette.hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, w, cx| this.activate_tab(ix, w, cx)))
                    .child(match tab {
                        Tab::Diff(_) => {
                            icon_el("git-compare", ICON_FILE, palette.accent).into_any_element()
                        }
                        Tab::Settings => {
                            icon_el("settings", ICON_FILE, palette.muted).into_any_element()
                        }
                        Tab::Extension(_) => {
                            icon_el("blocks", ICON_FILE, palette.accent).into_any_element()
                        }
                        _ => registry_icon(super::file_icons::for_file(&title), false, palette)
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(if conflicted {
                                palette.danger
                            } else if selected {
                                palette.fg
                            } else {
                                palette.muted
                            })
                            .child(title),
                    )
                    .child(
                        div()
                            .id(("tab-close", ix))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded(px(3.))
                            .hover(move |s| s.bg(palette.active))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.close_tab(ix, w, cx);
                            }))
                            .child(if dirty {
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(palette.accent_fill)
                                    .into_any_element()
                            } else {
                                div()
                                    .text_size(px(13.))
                                    .text_color(palette.muted)
                                    .child("×")
                                    .into_any_element()
                            }),
                    )
                    .into_any_element(),
            );
        }
        div()
            .id("tab-strip")
            .h(px(32.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .bg(palette.panel)
            .border_b_1()
            .border_color(palette.border)
            .overflow_x_scroll()
            .children(items)
            .into_any_element()
    }

    fn render_trust_bar(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let palette = pal(cx);
        let p = self.project.as_ref()?;
        // The question is about code files: it does not need to sit over Settings or media.
        let on_file = p
            .active
            .and_then(|a| p.tabs.get(a))
            .is_some_and(|t| t.as_file().is_some());
        if !p.trust_prompt || !on_file {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .bg(palette.accent_soft)
                .border_b_1()
                .border_color(palette.border)
                .child(div().flex_1().min_w(px(240.)).text_size(px(12.)).child("Language servers can run code from this folder (build scripts, plugins, interpreter). Enable them here?"))
                .child(primary_button("trust-yes", "Trust This Folder", palette).on_click(cx.listener(|this, _, w, cx| this.trust_project(w, cx))))
                .child(button("trust-no", "Not Now", palette).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(p) = &mut this.project {
                        p.trust_prompt = false;
                    }
                    cx.notify();
                }))),
        )
    }

    fn render_conflict_bar(
        &mut self,
        ix: usize,
        conflict: &Conflict,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = pal(cx);
        let base = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .bg(palette.hover)
            .border_b_1()
            .border_color(palette.danger);
        match conflict {
            Conflict::Changed { .. } => base
                .child(icon_el("triangle-alert", 13., palette.warning))
                .child(div().flex_1().min_w(px(220.)).text_size(px(12.)).child("This file changed on disk and has unsaved changes here. Nothing was overwritten."))
                .child(button("cf-diff", "Compare", palette).on_click(cx.listener(move |this, _, w, cx| this.conflict_diff(ix, w, cx))))
                .child(button("cf-disk", "Use Disk Version", palette).on_click(cx.listener(move |this, _, w, cx| this.conflict_load_disk(ix, w, cx))))
                .child(button("cf-keep", "Keep Editing Mine", palette).on_click(cx.listener(move |this, _, _, cx| this.conflict_keep_buffer(ix, cx))))
                .child(button("cf-over", "Overwrite Disk", palette).on_click(cx.listener(move |this, _, _, cx| this.conflict_overwrite(ix, cx))))
                .into_any_element(),
            Conflict::Deleted => base
                .child(icon_el("triangle-alert", 13., palette.warning))
                .child(div().flex_1().min_w(px(220.)).text_size(px(12.)).child("This file was deleted on disk. Your buffer is kept."))
                .child(button("cf-recreate", "Save Again", palette).on_click(cx.listener(move |this, _, _, cx| this.conflict_overwrite(ix, cx))))
                .child(button("cf-close", "Close Tab", palette).on_click(cx.listener(move |this, _, w, cx| this.force_close_tab(ix, w, cx))))
                .into_any_element(),
        }
    }

    pub fn render_editor_area(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = pal(cx);
        let _ = window;
        // Only the visible tab may play: this covers every way of leaving a tab.
        if let Some(p) = &self.project {
            let active = p.active;
            let hidden: Vec<_> = p
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, _)| Some(*i) != active)
                .filter_map(|(_, t)| match t {
                    Tab::Media(m, _) => Some(m.clone()),
                    _ => None,
                })
                .collect();
            for m in hidden {
                m.update(cx, |m, _| m.on_hidden());
            }
        }
        let strip = self.render_tab_strip(cx).into_any_element();
        let trust = self.render_trust_bar(cx).map(|t| t.into_any_element());
        let (active, conflict) = {
            let p = self.project.as_ref().expect("project");
            let conflict = p
                .active
                .and_then(|a| p.tabs.get(a))
                .and_then(|t| t.as_file())
                .and_then(|f| f.conflict.clone());
            (p.active, conflict)
        };
        let conflict_bar = match (&conflict, active) {
            (Some(c), Some(ix)) => Some(self.render_conflict_bar(ix, c, cx).into_any_element()),
            _ => None,
        };
        let breadcrumbs = self.render_breadcrumbs(cx);
        // These two tabs render from the application state itself.
        let special = {
            let p = self.project.as_ref().expect("project");
            match active.and_then(|a| p.tabs.get(a)) {
                Some(Tab::Settings) => Some(None),
                Some(Tab::Extension(id)) => Some(Some(id.clone())),
                _ => None,
            }
        };
        let special_body = special.map(|s| match s {
            None => self.render_settings_tab(window, cx),
            Some(id) => self.render_extension_page(&id, cx),
        });
        let p = self.project.as_ref().expect("project");
        let body: AnyElement = match active.and_then(|a| p.tabs.get(a)) {
            _ if special_body.is_some() => special_body.expect("checked"),
            Some(Tab::Media(m, _)) => div()
                .key_context("KernEditor")
                .size_full()
                .child(m.clone())
                .into_any_element(),
            Some(Tab::Settings | Tab::Extension(_)) => div().into_any_element(),
            Some(Tab::File(f)) => editor_tab_keys(div(), &f.editor)
                .key_context("KernEditor")
                // Esc also closes the parameter hint; the editor still gets it.
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" && this.signature.take().is_some() {
                        cx.notify();
                    }
                }))
                .size_full()
                .bg(palette.editor)
                .child(
                    Editor::new(&f.editor)
                        .h_full()
                        .bordered(false)
                        .appearance(false)
                        // The system menu shows each command's shortcut for this OS.
                        .context_menu(|menu, _, _| {
                            use gpui_kit::component::input as gi;
                            menu.menu("Cut", Box::new(gi::Cut))
                                .menu("Copy", Box::new(gi::Copy))
                                .menu("Paste", Box::new(gi::Paste))
                                .separator()
                                .menu("Select All", Box::new(gi::SelectAll))
                                .separator()
                                .menu("Go to Definition", Box::new(GoToDefinitionHere))
                                .menu("Format Document", Box::new(FormatDocument))
                        }),
                )
                .into_any_element(),
            Some(Tab::Diff(d)) => div()
                .key_context("KernEditor")
                .size_full()
                .flex()
                .flex_col()
                .when_some(d.note.clone(), |x, n| {
                    x.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_size(px(12.))
                            .text_color(palette.warning)
                            .border_b_1()
                            .border_color(palette.border)
                            .child(n),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(Diff::new(&d.state).line_number(true)),
                )
                .into_any_element(),
            None => div()
                .key_context("KernEditor")
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette.muted)
                .text_size(px(12.))
                .child(format!(
                    "{} to open a file  ·  {} for commands",
                    super::keys::QUICK_OPEN.label(),
                    super::keys::COMMAND_PALETTE.label()
                ))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(strip)
            .children(trust)
            .children(conflict_bar)
            .children(breadcrumbs)
            .child(div().flex_1().min_h_0().child(body))
    }

    /// Change Language Mode. `remember` also associates the name or extension.
    pub fn set_file_language(
        &mut self,
        lang: Language,
        remember: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.active_file().map(|f| f.path.clone()) else {
            return;
        };
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else {
            return;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if remember {
            self.settings
                .file_associations
                .insert(lang::association_key(&path), lang.id.to_string());
            self.settings
                .session_mut(&root)
                .language_overrides
                .remove(&rel);
        } else {
            self.settings
                .session_mut(&root)
                .language_overrides
                .insert(rel, lang.id.to_string());
        }
        self.persist();
        let settings = self.settings.clone();
        let Some(p) = &mut self.project else { return };
        let Some(ix) = p.active else { return };
        let Some(Tab::File(f)) = p.tabs.get_mut(ix) else {
            return;
        };
        let old = f.lang;
        f.lang = lang;
        f.diagnostic_counts = (0, 0);
        f.pushed_diagnostics.clear();
        f.pulled_diagnostics.clear();
        if let Some(c) = f.lsp.take() {
            c.did_close(&lsp::path_to_uri(&f.path));
        }
        f.lsp_failed = None;
        let text = f.editor.read(cx).text().to_string();
        let pos = f.editor.read(cx).cursor_position();
        let file_path = f.path.clone();
        // A lexer and a grammar use different highlighter factories inside the
        // widget, so crossing that line takes a new editor state.
        let crossing =
            (old.highlight == lang::Highlight::Lexer) != (lang.highlight == lang::Highlight::Lexer);
        if crossing || old.server.is_some() {
            let editor = new_editor(&file_path, lang, text.clone(), &settings, window, cx);
            let Some(p) = &mut self.project else { return };
            let Some(Tab::File(f)) = p.tabs.get_mut(ix) else {
                return;
            };
            f._sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
            f._obs = cx.observe(&editor, |_, _, cx| cx.notify());
            f.editor = editor.clone();
            editor.update(cx, |e, cx| e.set_cursor_position(pos, window, cx));
        } else {
            f.editor
                .update(cx, |e, cx| e.set_highlighter(lang.grammar, cx));
        }
        self.attach_lsp(&path, text, window, cx);
        cx.notify();
    }

    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = pal(cx);
        let p = self.project.as_ref()?;
        let path = p.active.and_then(|a| p.tabs.get(a)).and_then(|t| match t {
            Tab::File(_) | Tab::Media(..) => t.file_path().map(Path::to_path_buf),
            _ => None,
        })?;
        let rel = path.strip_prefix(&p.root).unwrap_or(&path).to_path_buf();
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let last = parts.len().saturating_sub(1);
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .h(px(22.))
                .px_3()
                .bg(palette.bg)
                .border_b_1()
                .border_color(palette.border)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.))
                .children(parts.into_iter().enumerate().map(|(i, part)| {
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .when(i > 0, |d| {
                            d.child(icon_el("chevron-right", 10., palette.muted))
                        })
                        .child(
                            div()
                                .text_color(if i == last { palette.fg } else { palette.muted })
                                .child(part),
                        )
                }))
                .into_any_element(),
        )
    }

    pub fn trust_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        p.trusted = true;
        p.trust_prompt = false;
        let root = p.root.clone();
        if !self.settings.trusted_projects.contains(&root) {
            self.settings.trusted_projects.push(root);
        }
        self.persist();
        self.reattach_lsp_all(window, cx);
        cx.notify();
    }

    pub fn lsp_status_label(&self, f: &FileTab) -> (Option<String>, Option<Hsla>) {
        let Some(p) = &self.project else {
            return (None, None);
        };
        let palette = crate::ui::theme::status_palette(self.settings.theme);
        if let Some((server, hint)) = &f.lsp_failed {
            return (Some(format!("{server}: {hint}")), Some(palette.warning));
        }
        match &f.lsp {
            Some(c) => {
                let st = p.lsp_status.get(&c.server_id);
                let (t, col) = match st {
                    Some(ServerStatus::Ready) => ("ready".to_string(), None),
                    Some(ServerStatus::Starting) | None => ("starting…".to_string(), None),
                    Some(ServerStatus::Failed(e)) => (format!("failed: {e}"), Some(palette.danger)),
                    Some(ServerStatus::Exited) => ("stopped".to_string(), Some(palette.warning)),
                    Some(ServerStatus::Missing { hint }) => {
                        (format!("not found · {hint}"), Some(palette.warning))
                    }
                };
                (Some(format!("{}: {t}", c.server_id)), col)
            }
            None if f.lang.server.is_some() && !p.trusted => {
                (Some("LSP off: folder not trusted".into()), None)
            }
            None if lang::server_for(&f.path, &p.root, &self.settings).is_none() => {
                (Some("no language server".into()), None)
            }
            None => (None, None),
        }
    }
}

/// What Tab inserts in this file: its own indentation style, else the setting.
pub fn file_tab_size(
    path: &std::path::Path,
    text: &str,
    default_size: usize,
) -> gpui_kit::component::input::TabSize {
    let d = crate::core::indent::detect(path, text, default_size);
    gpui_kit::component::input::TabSize {
        tab_size: d.size,
        hard_tabs: d.hard_tabs,
    }
}

/// A selection dragged to the start of the next line covers no text there, so
/// Tab must leave that line alone.
fn trim_selection_tail(e: &mut EditorState, cx: &mut Context<EditorState>) {
    use gpui_kit::component::RopeExt as _;
    let r = e.selected_range();
    if r.is_empty() {
        return;
    }
    let text = e.text();
    let (first, last) = (
        text.offset_to_position(r.start),
        text.offset_to_position(r.end),
    );
    if last.line > first.line && last.character == 0 {
        let reversed = e.is_selection_reversed();
        e.set_selected_range_with_direction(r.start..r.end - 1, reversed, cx);
    }
}

/// Tab and Shift+Tab in a code editor. The completion menu takes Tab first: the
/// library only offers it Enter, and would otherwise indent under an open menu.
fn editor_tab_keys(el: Div, editor: &Entity<EditorState>) -> Div {
    use gpui_kit::component::input::{Enter, IndentInline, OutdentInline};
    let indent = editor.clone();
    let outdent = editor.clone();
    el.capture_action(move |_: &IndentInline, window, cx| {
        indent.update(cx, |e, cx| {
            let enter = Enter {
                secondary: false,
                shift: false,
            };
            if e.completion_menu_state().open && e.route_overlay_action(Box::new(enter), window, cx)
            {
                cx.stop_propagation();
                return;
            }
            trim_selection_tail(e, cx);
        });
    })
    .capture_action(move |_: &OutdentInline, _window, cx| {
        outdent.update(cx, trim_selection_tail);
    })
}

/// A code editor configured from settings.
pub fn new_editor(
    path: &std::path::Path,
    lang: Language,
    text: String,
    settings: &crate::core::settings::Settings,
    window: &mut Window,
    cx: &mut Context<KernApp>,
) -> Entity<EditorState> {
    let large = text.len() > 4 * 1024 * 1024;
    let grammar = if large { "text" } else { lang.grammar };
    let e = settings.editor.clone();
    let lexer = lang.highlight == lang::Highlight::Lexer && !large;
    let tab = file_tab_size(path, &text, e.tab_size);
    cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language(grammar)
            .line_number(e.line_numbers)
            .soft_wrap(e.word_wrap)
            .indent_guides(e.indent_guides)
            .tab_size(tab)
            .default_value(text);
        if lexer {
            state.set_highlighter_factory(super::grammars::lexer_factory(), cx);
        }
        state
    })
}

/// Replace editor contents, keeping the cursor line where possible.
pub fn set_editor_text(
    editor: &Entity<EditorState>,
    text: String,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |e, cx| {
        let pos = e.cursor_position();
        // set_cursor_position focuses the editor; keep focus where the user is.
        let previous = window.focused(cx);
        e.set_value(text, window, cx);
        e.set_cursor_position(pos, window, cx);
        if let Some(focus) = previous {
            window.focus(&focus, cx);
        }
    });
}
