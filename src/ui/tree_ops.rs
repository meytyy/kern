//! Explorer edits in place: a name field in a row of the tree for New File, New
//! Folder and Rename; Move to… and drag and drop onto folders; Move to Trash.
//! The header buttons, the File menu, the palette and the right-click menu all
//! come through here.
use super::app::*;
use super::editor_area::Tab;
use super::*;
use crate::workspace::tree;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use std::path::{Path, PathBuf};

pub enum EditKind {
    New { dir: PathBuf, folder: bool },
    Rename(PathBuf),
}

/// The name field shown in the tree.
pub struct InlineEdit {
    pub kind: EditKind,
    pub input: Entity<InputState>,
    /// Shown in the row under the field; the typed name stays for fixing.
    pub error: Option<String>,
    error_value: String,
    /// Set once the field has had the focus; losing it afterwards cancels.
    pub had_focus: bool,
    _sub: Subscription,
}

/// "Move to Trash" on macOS, "Move to Recycle Bin" on Windows.
pub fn trash_label() -> &'static str {
    if cfg!(windows) {
        "Move to Recycle Bin"
    } else {
        "Move to Trash"
    }
}

fn trash_name() -> &'static str {
    if cfg!(windows) {
        "the Recycle Bin"
    } else {
        "the Trash"
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

impl KernApp {
    fn edit_input(
        &mut self,
        initial: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Subscription) {
        let (initial, placeholder) = (initial.to_string(), placeholder.to_string());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut KernApp, input, ev: &InputEvent, w, cx| match ev {
                InputEvent::PressEnter { .. } => this.commit_tree_edit(w, cx),
                // Clicking anywhere else (the editor, another row, another app)
                // drops the unconfirmed name: nothing was created yet.
                InputEvent::Blur => {
                    if this
                        .project
                        .as_ref()
                        .and_then(|p| p.tree.edit.as_ref())
                        .is_some_and(|e| &e.input == input)
                    {
                        this.drop_tree_edit(cx);
                    }
                }
                InputEvent::Change => {
                    // Only a real edit of the rejected name clears its message.
                    if let Some(p) = &mut this.project
                        && let Some(e) = &mut p.tree.edit
                        && e.error.is_some()
                        && e.input.read(cx).value().trim() != e.error_value
                    {
                        e.error = None;
                        cx.notify();
                    }
                }
                _ => {}
            },
        );
        (input, sub)
    }

    /// A name field for a new file or folder at the top of `dir`, which opens.
    pub fn start_new_item(
        &mut self,
        dir: PathBuf,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.project else {
            self.toast("Open a folder first", false, cx);
            return;
        };
        if !dir.starts_with(&p.root) || !dir.is_dir() {
            return;
        }
        // The folder and every folder above it open, so the field is visible.
        let mut up = dir.clone();
        loop {
            p.tree.expanded.insert(up.clone());
            if up == p.root || !up.pop() || !up.starts_with(&p.root) {
                break;
            }
        }
        let load = !p.tree.children.contains_key(&dir);
        let placeholder = if folder { "Folder name" } else { "File name" };
        let (input, sub) = self.edit_input("", placeholder, window, cx);
        input.update(cx, |i, cx| i.focus(window, cx));
        self.settings.layout.sidebar_visible = true;
        self.side = SideView::Files;
        if let Some(p) = &mut self.project {
            p.tree.edit = Some(InlineEdit {
                kind: EditKind::New {
                    dir: dir.clone(),
                    folder,
                },
                input,
                error: None,
                error_value: String::new(),
                had_focus: false,
                _sub: sub,
            });
            p.tree.reveal_edit = true;
        }
        if load {
            self.refresh_tree_dir(&dir, cx);
        }
        self.reveal_pending();
        cx.notify();
    }

    /// Rename in the row itself; the name without its extension is selected.
    pub fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        if path == p.root {
            self.toast("The project folder cannot be renamed from here", false, cx);
            return;
        }
        let name = name_of(&path);
        let (input, sub) = self.edit_input(&name, "Name", window, cx);
        // `report.final.cs` selects `report.final`; `.gitignore` selects it all.
        let stem_end = match name.rfind('.') {
            _ if path.is_dir() => name.len(),
            Some(0) | None => name.len(),
            Some(i) => i,
        };
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.set_selected_range(0..stem_end, cx);
        });
        if let Some(p) = &mut self.project {
            p.tree.selected = Some(path.clone());
            p.tree.edit = Some(InlineEdit {
                kind: EditKind::Rename(path),
                input,
                error: None,
                error_value: String::new(),
                had_focus: false,
                _sub: sub,
            });
            p.tree.reveal_edit = true;
        }
        self.reveal_pending();
        cx.notify();
    }

    /// Esc, or Enter with no name: the row goes and the tree keeps the focus.
    pub fn cancel_tree_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drop_tree_edit(cx) {
            window.focus(&self.tree_focus, cx);
        }
    }

    /// Removes the name field without touching the focus (it went elsewhere).
    pub fn drop_tree_edit(&mut self, cx: &mut Context<Self>) -> bool {
        let dropped = self
            .project
            .as_mut()
            .is_some_and(|p| p.tree.edit.take().is_some());
        if dropped {
            cx.notify();
        }
        dropped
    }

    /// Enter in the name field.
    pub fn commit_tree_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let Some(edit) = &p.tree.edit else { return };
        let root = p.root.clone();
        let value = edit.input.read(cx).value().trim().to_string();
        let fail = |this: &mut Self, e: anyhow::Error, cx: &mut Context<Self>| {
            if let Some(p) = &mut this.project
                && let Some(edit) = &mut p.tree.edit
            {
                edit.error = Some(e.to_string());
                edit.error_value = value.clone();
            }
            cx.notify();
        };
        match &edit.kind {
            EditKind::New { dir, folder } => {
                if value.is_empty() {
                    return self.cancel_tree_edit(window, cx);
                }
                let (dir, folder) = (dir.clone(), *folder);
                let made = if folder {
                    tree::create_dir(&root, &dir, &value)
                } else {
                    tree::create_file(&root, &dir, &value)
                };
                match made {
                    Ok(path) => {
                        if let Some(p) = &mut self.project {
                            p.tree.edit = None;
                        }
                        self.after_create(&dir, path, folder, window, cx);
                    }
                    Err(e) => fail(self, e, cx),
                }
            }
            EditKind::Rename(old) => {
                let old = old.clone();
                if value.is_empty() || value == name_of(&old) {
                    return self.cancel_tree_edit(window, cx);
                }
                match tree::rename(&root, &old, &value) {
                    Ok(new) => {
                        if let Some(p) = &mut self.project {
                            p.tree.edit = None;
                        }
                        self.after_move(&old, &new, cx);
                        window.focus(&self.tree_focus, cx);
                    }
                    Err(e) => fail(self, e, cx),
                }
            }
        }
    }

    /// After a rename or move: tabs follow (unsaved text stays in its buffer),
    /// both folders are listed again, the item is selected, Git refreshes.
    pub fn after_move(&mut self, old: &Path, new: &Path, cx: &mut Context<Self>) {
        self.after_rename(old, new, cx);
        let Some(p) = &mut self.project else { return };
        for t in &mut p.tabs {
            if let Tab::Media(_, path) = t
                && let Ok(rest) = path.strip_prefix(old)
            {
                let moved = if rest.as_os_str().is_empty() {
                    new.to_path_buf()
                } else {
                    new.join(rest)
                };
                *path = moved;
            }
        }
        // An open folder keeps its expanded state, and so do its subfolders.
        let moved: Vec<PathBuf> = p
            .tree
            .expanded
            .iter()
            .filter(|d| d.starts_with(old))
            .cloned()
            .collect();
        for d in moved {
            p.tree.expanded.remove(&d);
            p.tree.children.remove(&d);
            let rest = d.strip_prefix(old).unwrap_or(Path::new(""));
            p.tree.expanded.insert(if rest.as_os_str().is_empty() {
                new.to_path_buf()
            } else {
                new.join(rest)
            });
        }
        let mut dirs: Vec<PathBuf> = vec![];
        for d in [old.parent(), new.parent()].into_iter().flatten() {
            if !dirs.iter().any(|x| x == d) {
                dirs.push(d.to_path_buf());
            }
        }
        if let Some(parent) = new.parent() {
            p.tree.expanded.insert(parent.to_path_buf());
        }
        if new.is_dir() {
            dirs.push(new.to_path_buf());
        }
        p.tree.selected = Some(new.to_path_buf());
        p.tree.reveal = Some(new.to_path_buf());
        for d in dirs {
            self.refresh_tree_dir(&d, cx);
        }
        self.refresh_git(cx);
        self.refresh_file_index(cx);
        self.persist();
        cx.notify();
    }

    /// Move to… and drops onto a folder row.
    pub fn move_item(&mut self, src: &Path, dest: &Path, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let root = p.root.clone();
        match tree::move_into(&root, src, dest) {
            Ok(new) => {
                let src = src.to_path_buf();
                self.after_move(&src, &new, cx);
                let shown = new
                    .strip_prefix(&root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                self.toast(format!("Moved to {shown}"), false, cx);
            }
            Err(e) => self.toast(e.to_string(), true, cx),
        }
    }

    /// A drop onto its own row or its current folder is not a move.
    pub fn drop_on_folder(&mut self, src: &Path, dest: &Path, cx: &mut Context<Self>) {
        if src == dest || src.parent() == Some(dest) {
            return;
        }
        self.move_item(src, dest, cx);
    }

    /// Open tabs of `path` (a file, or anything inside a folder).
    fn tabs_under(&self, path: &Path) -> Vec<(usize, bool)> {
        let Some(p) = &self.project else {
            return vec![];
        };
        p.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                t.file_path()
                    .filter(|f| f.starts_with(path))
                    .map(|_| (i, t.is_dirty()))
            })
            .collect()
    }

    /// Move to Trash: a folder is confirmed by name, and unsaved edits in an
    /// open file are saved first or the action is cancelled.
    pub fn request_trash(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        if path == p.root {
            self.toast("The project folder cannot be deleted from here", true, cx);
            return;
        }
        let dirty = self.tabs_under(&path).iter().filter(|(_, d)| *d).count();
        let name = name_of(&path);
        let is_dir = path.is_dir();
        if !is_dir && dirty == 0 {
            return self.trash_now(path, false, cx);
        }
        let mut body = if is_dir {
            format!(
                "Move the folder “{name}” and everything in it to {}?",
                trash_name()
            )
        } else {
            format!("“{name}” has unsaved changes.")
        };
        if dirty > 0 && is_dir {
            body.push_str(&format!(
                " {dirty} open file{} in it {} unsaved changes.",
                if dirty == 1 { "" } else { "s" },
                if dirty == 1 { "has" } else { "have" }
            ));
        }
        let go = if dirty > 0 {
            overlays::ConfirmChoice::new(
                &format!("Save and {}", trash_label()),
                overlays::ConfirmAction::Trash {
                    path: path.clone(),
                    save_first: true,
                },
                true,
            )
        } else {
            overlays::ConfirmChoice::new(
                trash_label(),
                overlays::ConfirmAction::Trash {
                    path: path.clone(),
                    save_first: false,
                },
                true,
            )
        };
        self.overlay_return = Some(self.tree_focus.clone());
        self.overlay = Some(overlays::Overlay::confirm(
            trash_label(),
            body,
            // The dialog has its own Cancel button.
            vec![go],
        ));
        cx.notify();
    }

    pub fn trash_now(&mut self, path: PathBuf, save_first: bool, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let root = p.root.clone();
        if save_first {
            for (ix, dirty) in self.tabs_under(&path) {
                if dirty && !self.save_tab(ix, false, cx) {
                    self.toast("Not moved: the changes could not be saved", true, cx);
                    return;
                }
            }
        }
        if let Err(e) = tree::trash(&root, &path) {
            self.toast(e.to_string(), true, cx);
            return;
        }
        // Its tabs close; their text is in the trash with the file.
        let Some(p) = &mut self.project else { return };
        let mut ix = p.tabs.len();
        while ix > 0 {
            ix -= 1;
            if p.tabs[ix].file_path().is_some_and(|f| f.starts_with(&path)) {
                let tab = p.tabs.remove(ix);
                if let Tab::File(f) = &tab
                    && let Some(c) = &f.lsp
                {
                    c.did_close(&crate::lsp::path_to_uri(&f.path));
                }
            }
        }
        p.active = if p.tabs.is_empty() {
            None
        } else {
            Some(p.active.unwrap_or(0).min(p.tabs.len() - 1))
        };
        p.tree.expanded.retain(|d| !d.starts_with(&path));
        p.tree.children.retain(|d, _| !d.starts_with(&path));
        if p.tree
            .selected
            .as_ref()
            .is_some_and(|s| s.starts_with(&path))
        {
            p.tree.selected = path.parent().map(Path::to_path_buf);
        }
        if let Some(parent) = path.parent() {
            let parent = parent.to_path_buf();
            self.refresh_tree_dir(&parent, cx);
        }
        self.refresh_git(cx);
        self.refresh_file_index(cx);
        self.persist();
        self.toast(
            format!("Moved “{}” to {}", name_of(&path), trash_name()),
            false,
            cx,
        );
        cx.notify();
    }

    /// Every folder of the project that `src` could move into.
    pub fn move_targets(&self, src: &Path) -> Vec<PathBuf> {
        let Some(p) = &self.project else {
            return vec![];
        };
        let mut out = vec![PathBuf::new()];
        let walker = ignore::WalkBuilder::new(&p.root)
            .hidden(false)
            .require_git(false)
            .add_custom_ignore_filename(".kernignore")
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        for e in walker.flatten().take(20_000) {
            if e.depth() > 0
                && e.file_type().is_some_and(|t| t.is_dir())
                && let Ok(rel) = e.path().strip_prefix(&p.root)
            {
                out.push(rel.to_path_buf());
            }
            if out.len() > 5_000 {
                break;
            }
        }
        let current = src.parent().map(Path::to_path_buf);
        out.retain(|rel| {
            let abs = p.root.join(rel);
            !abs.starts_with(src) && Some(&abs) != current.as_ref()
        });
        out
    }
}
