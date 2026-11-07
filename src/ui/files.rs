//! Files sidebar: lazily loaded tree with create and rename.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use super::*;
use crate::git::status::FileState;
use crate::workspace::tree::{self, Entry};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct TreeState {
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<Entry>>,
    pub selected: Option<PathBuf>,
    pub scroll: UniformListScrollHandle,
}

#[derive(Clone)]
struct Row {
    entry: Entry,
    depth: usize,
    expanded: bool,
}

impl KernApp {
    pub fn refresh_tree_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let root = p.root.clone();
        let target = dir.to_path_buf();
        cx.spawn(async move |this, cx| {
            let (r, t) = (root.clone(), target.clone());
            let result = cx
                .background_spawn(async move { tree::list_dir(&r, &t) })
                .await;
            this.update(cx, |this, cx| {
                let Some(p) = &mut this.project else { return };
                if p.root != root {
                    return;
                }
                match result {
                    Ok(list) => {
                        p.tree.children.insert(target, list);
                    }
                    Err(_) => {
                        p.tree.children.remove(&target);
                        p.tree.expanded.remove(&target);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The project root first, then whatever is expanded under it.
    fn visible_rows(&self) -> Vec<Row> {
        let Some(p) = &self.project else {
            return vec![];
        };
        let root_open = p.tree.expanded.contains(&p.root);
        let mut out = vec![Row {
            entry: Entry {
                path: p.root.clone(),
                name: p.name.clone(),
                is_dir: true,
                ignored: false,
            },
            depth: 0,
            expanded: root_open,
        }];
        fn walk(dir: &Path, depth: usize, st: &TreeState, out: &mut Vec<Row>) {
            let Some(list) = st.children.get(dir) else {
                return;
            };
            for e in list {
                let expanded = e.is_dir && st.expanded.contains(&e.path);
                out.push(Row {
                    entry: e.clone(),
                    depth,
                    expanded,
                });
                if expanded {
                    walk(&e.path, depth + 1, st, out);
                }
            }
        }
        if root_open {
            walk(&p.root, 1, &p.tree, &mut out);
        }
        out
    }

    fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        if !p.tree.expanded.remove(path) {
            p.tree.expanded.insert(path.to_path_buf());
            self.refresh_tree_dir(path, cx);
        }
    }

    fn click_row(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.tree_focus, cx);
        let Some(p) = &mut self.project else { return };
        p.tree.selected = Some(entry.path.clone());
        if entry.is_dir {
            self.toggle_dir(&entry.path, cx);
        } else {
            self.open_file_async(entry.path.clone(), true, None, window, cx);
            // Opening a file moves the focus to its editor: the tree keeps the selection.
        }
        cx.notify();
    }

    /// Arrow keys in the tree. Files open only with Enter or Space.
    pub fn tree_key(&mut self, key: file_icons::TreeKey, cx: &mut Context<Self>) {
        let rows = self.visible_rows();
        let Some(p) = &mut self.project else { return };
        let shape: Vec<(usize, bool, bool)> = rows
            .iter()
            .map(|r| (r.depth, r.entry.is_dir, r.expanded))
            .collect();
        let selected = p
            .tree
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| r.entry.path == *s));
        match file_icons::tree_move(&shape, selected, key) {
            file_icons::TreeMove::Select(ix) => {
                p.tree.selected = Some(rows[ix].entry.path.clone());
                p.tree.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            file_icons::TreeMove::Expand(ix) | file_icons::TreeMove::Collapse(ix) => {
                let path = rows[ix].entry.path.clone();
                self.toggle_dir(&path, cx);
            }
            file_icons::TreeMove::Nothing => {}
        }
        cx.notify();
    }

    pub fn tree_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.visible_rows();
        let Some(selected) = self.project.as_ref().and_then(|p| p.tree.selected.clone()) else {
            return;
        };
        if let Some(row) = rows.iter().find(|r| r.entry.path == selected) {
            let entry = row.entry.clone();
            self.click_row(&entry, window, cx);
        }
    }

    /// Git letters by absolute path, and the folders that contain a change.
    fn git_marks(&self) -> (HashMap<PathBuf, FileState>, HashSet<PathBuf>) {
        let mut files = HashMap::new();
        let mut dirs = HashSet::new();
        let Some(p) = &self.project else {
            return (files, dirs);
        };
        let Some(status) = &p.git.status else {
            return (files, dirs);
        };
        for change in &status.files {
            let path = p.root.join(&change.path);
            let mut parent = path.parent();
            while let Some(dir) = parent {
                if !dir.starts_with(&p.root) || !dirs.insert(dir.to_path_buf()) {
                    break;
                }
                parent = dir.parent();
            }
            files.insert(path, change.state);
        }
        (files, dirs)
    }

    /// Directory for new items: the selected folder, or the selected file's folder.
    pub fn target_dir(&self) -> Option<PathBuf> {
        let p = self.project.as_ref()?;
        Some(match &p.tree.selected {
            Some(s) if s.is_dir() => s.clone(),
            Some(s) => s
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| p.root.clone()),
            None => p.root.clone(),
        })
    }

    pub fn prompt_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.target_dir() else { return };
        self.overlay = Some(overlays::Overlay::prompt(
            "New File",
            format!("In {}", dir.display()),
            "",
            overlays::PromptKind::NewFile(dir),
            window,
            cx,
        ));
        cx.notify();
    }

    pub fn prompt_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.target_dir() else { return };
        self.overlay = Some(overlays::Overlay::prompt(
            "New Folder",
            format!("In {}", dir.display()),
            "",
            overlays::PromptKind::NewFolder(dir),
            window,
            cx,
        ));
        cx.notify();
    }

    pub fn prompt_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.project.as_ref().and_then(|p| p.tree.selected.clone()) else {
            self.toast("Select a file or folder first", false, cx);
            return;
        };
        if self.project.as_ref().is_some_and(|p| p.root == sel) {
            self.toast("The project folder cannot be renamed from here", false, cx);
            return;
        }
        let name = sel
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.overlay = Some(overlays::Overlay::prompt(
            "Rename",
            sel.display().to_string(),
            &name,
            overlays::PromptKind::Rename(sel),
            window,
            cx,
        ));
        cx.notify();
    }

    /// Open tabs follow a rename of their file or a parent folder.
    pub fn after_rename(&mut self, old: &Path, new: &Path, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        for t in &mut p.tabs {
            if let super::editor_area::Tab::File(f) = t
                && let Ok(rest) = f.path.strip_prefix(old)
            {
                // `join` with an empty path would append a trailing slash.
                let updated = if rest.as_os_str().is_empty() {
                    new.to_path_buf()
                } else {
                    new.join(rest)
                };
                // The language server sees a rename as close + open.
                if let Some(c) = &f.lsp {
                    c.did_close(&crate::lsp::path_to_uri(&f.path));
                    let lang = f.lang;
                    c.did_open(
                        &crate::lsp::path_to_uri(&updated),
                        lang.lsp_id,
                        f.version,
                        f.editor.read(cx).text().to_string(),
                    );
                }
                f.path = updated.clone();
                f.doc.path = updated;
            }
        }
        p.tree.selected = Some(new.to_path_buf());
        if let Some(parent) = new.parent() {
            let parent = parent.to_path_buf();
            self.refresh_tree_dir(&parent, cx);
        }
        self.persist();
    }

    pub fn render_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = pal(cx);
        let rows = self.visible_rows();
        let count = rows.len();
        let active_path = self.active_file().map(|f| f.path.clone());
        let selected = self.project.as_ref().and_then(|p| p.tree.selected.clone());
        let dirty: HashSet<PathBuf> = self
            .project
            .as_ref()
            .map(|p| {
                p.tabs
                    .iter()
                    .filter(|t| t.is_dirty())
                    .filter_map(|t| t.file_path().map(Path::to_path_buf))
                    .collect()
            })
            .unwrap_or_default();
        let (git_files, git_dirs) = self.git_marks();
        let tree_focused = self.tree_focus.is_focused(window);
        let scroll = self
            .project
            .as_ref()
            .map(|p| p.tree.scroll.clone())
            .unwrap_or_default();
        let list = uniform_list(
            "file-tree",
            count,
            cx.processor(move |_this, range: std::ops::Range<usize>, _w, cx| {
                let mut out = Vec::with_capacity(range.len());
                for ix in range {
                    let row = rows[ix].clone();
                    let is_active = active_path.as_ref() == Some(&row.entry.path);
                    let is_selected = selected.as_ref() == Some(&row.entry.path);
                    let entry = row.entry.clone();
                    let is_root = row.depth == 0;
                    let drag_path = (!row.entry.is_dir).then(|| row.entry.path.clone());
                    let found = if row.entry.is_dir {
                        file_icons::for_folder(&row.entry.name, row.expanded)
                    } else {
                        file_icons::for_file(&row.entry.name)
                    };
                    let state = git_files.get(&row.entry.path).copied();
                    let name_color = if row.entry.ignored {
                        palette.muted.opacity(0.8)
                    } else {
                        palette.fg
                    };
                    out.push(
                        div()
                            .id(("row", ix))
                            .h(px(ROW_H))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pl(px(8. + row.depth as f32 * 12.))
                            .pr_2()
                            .when(is_active, |d| d.bg(palette.accent_soft))
                            .when(is_selected && !is_active, |d| d.bg(palette.hover))
                            .when(is_selected && tree_focused, |d| {
                                d.border_1().border_color(palette.accent)
                            })
                            .hover(move |s| s.bg(palette.hover))
                            .cursor_pointer()
                            .on_click(
                                cx.listener(move |this, _, w, cx| this.click_row(&entry, w, cx)),
                            )
                            .when_some(drag_path, |d, path| {
                                d.on_drag(agent_chat::DraggedFile(path), |file, _, _, cx| {
                                    cx.new(|_| agent_chat::DragPreview(file.name()))
                                })
                            })
                            .child(registry_icon(found, row.entry.ignored, palette))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .when(is_root, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .text_color(name_color)
                                    .child(row.entry.name.clone()),
                            )
                            .when(dirty.contains(&row.entry.path), |d| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(palette.accent_fill),
                                )
                            })
                            .when_some(state, |d, s| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .w(px(12.))
                                        .text_center()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(git_color(s, palette))
                                        .child(s.letter()),
                                )
                            })
                            .when(
                                row.entry.is_dir && !is_root && git_dirs.contains(&row.entry.path),
                                |d| {
                                    d.child(
                                        div()
                                            .flex_shrink_0()
                                            .w(px(12.))
                                            .flex()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .size(px(5.))
                                                    .rounded_full()
                                                    .bg(palette.tint_yellow),
                                            ),
                                    )
                                },
                            )
                            .into_any_element(),
                    );
                }
                out
            }),
        )
        .track_scroll(&scroll)
        .flex_1();
        let focus = self.tree_focus.clone();
        div()
            .id("explorer")
            .key_context("KernTree")
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|t, _: &TreeUp, _, cx| t.tree_key(file_icons::TreeKey::Up, cx)))
            .on_action(
                cx.listener(|t, _: &TreeDown, _, cx| t.tree_key(file_icons::TreeKey::Down, cx)),
            )
            .on_action(
                cx.listener(|t, _: &TreeLeft, _, cx| t.tree_key(file_icons::TreeKey::Left, cx)),
            )
            .on_action(
                cx.listener(|t, _: &TreeRight, _, cx| t.tree_key(file_icons::TreeKey::Right, cx)),
            )
            .on_action(cx.listener(|t, _: &TreeActivate, w, cx| t.tree_activate(w, cx)))
            .child(
                panel_header("Explorer", palette)
                    .child(
                        icon_button("new-file", "file-plus", "New File")
                            .on_click(cx.listener(|this, _, w, cx| this.prompt_new_file(w, cx))),
                    )
                    .child(
                        icon_button("new-folder", "folder-plus", "New Folder")
                            .on_click(cx.listener(|this, _, w, cx| this.prompt_new_folder(w, cx))),
                    )
                    .child(
                        icon_button("rename", "pencil", "Rename")
                            .on_click(cx.listener(|this, _, w, cx| this.prompt_rename(w, cx))),
                    )
                    .child(
                        icon_button("refresh-tree", "refresh-cw", "Refresh").on_click(cx.listener(
                            |this, _, _, cx| {
                                let dirs: Vec<PathBuf> = this
                                    .project
                                    .as_ref()
                                    .map(|p| p.tree.children.keys().cloned().collect())
                                    .unwrap_or_default();
                                for d in dirs {
                                    this.refresh_tree_dir(&d, cx);
                                }
                            },
                        )),
                    ),
            )
            .child(list)
    }
}

fn git_color(state: FileState, p: super::theme::Palette) -> Hsla {
    match state {
        FileState::Modified | FileState::TypeChanged | FileState::Renamed | FileState::Copied => {
            p.tint_yellow
        }
        FileState::Added | FileState::Untracked => p.tint_green,
        FileState::Deleted | FileState::Conflicted => p.danger,
    }
}
