//! Source Control panel and clone/GitHub flows. Local Git never requires GitHub.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use super::*;
use crate::git::{self, FileChange, RepoStatus};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

pub struct GitState {
    pub is_repo: bool,
    pub root: Option<PathBuf>,
    pub status: Option<RepoStatus>,
    pub busy: Option<String>,
    pub message: Option<(String, bool)>,
    pub commit_input: Entity<TextareaState>,
    pub branches: Vec<String>,
    pub branch_menu: bool,
    pub just_pushed: bool,
    pub refresh_running: bool,
    pub refresh_again: bool,
    pub graph: super::graph_panel::GraphState,
}

impl GitState {
    pub fn new(window: &mut Window, cx: &mut Context<KernApp>) -> Self {
        let commit_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Commit message")
        });
        Self {
            is_repo: false,
            root: None,
            status: None,
            busy: None,
            message: None,
            commit_input,
            branches: vec![],
            branch_menu: false,
            just_pushed: false,
            refresh_running: false,
            refresh_again: false,
            graph: Default::default(),
        }
    }
}

impl KernApp {
    /// Reload Git state on a background thread. Coalesces concurrent requests.
    pub fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        if p.git.refresh_running {
            p.git.refresh_again = true;
            return;
        }
        p.git.refresh_running = true;
        let root = p.root.clone();
        cx.spawn(async move |this, cx| {
            let r = root.clone();
            let result = cx
                .background_spawn(async move {
                    crate::platform::tools::wait_for_environment();
                    let repo = git::repo_root(&r)?;
                    let status = git::status(&repo);
                    let branches = git::branches(&repo).unwrap_or_default();
                    let refs = git::refs_fingerprint(&repo);
                    Some((repo, status, branches, refs))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(p) = &mut this.project else { return };
                if p.root != root {
                    return;
                }
                p.git.refresh_running = false;
                let mut reload_graph = false;
                match result {
                    Some((repo, status, branches, refs)) => {
                        // Commit, pull, fetch, switching branch: all move a ref.
                        reload_graph = p.git.graph.fingerprint != refs;
                        p.git.graph.fingerprint = refs;
                        p.git.is_repo = true;
                        p.git.root = Some(repo);
                        p.git.branches = branches;
                        match status {
                            Ok(st) => p.git.status = Some(st),
                            Err(e) => p.git.message = Some((e.to_string(), true)),
                        }
                    }
                    None => {
                        p.git.is_repo = false;
                        p.git.root = None;
                        p.git.status = None;
                    }
                }
                let again = std::mem::take(&mut p.git.refresh_again);
                if reload_graph {
                    this.reload_graph(cx);
                }
                cx.notify();
                if again {
                    this.refresh_git(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Run a Git operation in the background and report the outcome.
    fn git_op(
        &mut self,
        label: &str,
        op: impl FnOnce(PathBuf) -> anyhow::Result<String> + Send + 'static,
        after: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.project else { return };
        let Some(root) = p.git.root.clone() else {
            return;
        };
        if p.git.busy.is_some() {
            return;
        }
        p.git.busy = Some(label.to_string());
        p.git.message = None;
        cx.notify();
        let label = label.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { op(root) }).await;
            this.update(cx, |this, cx| {
                if let Some(p) = &mut this.project {
                    p.git.busy = None;
                    let ok = result.is_ok();
                    p.git.message = Some(match result {
                        Ok(msg) if msg.trim().is_empty() => (format!("{label}: done"), false),
                        Ok(msg) => (msg.trim().to_string(), false),
                        Err(e) => (e.to_string(), true),
                    });
                    if ok {
                        after(this, cx);
                    }
                }
                this.refresh_git(cx);
            })
            .ok();
        })
        .detach();
    }

    fn git_stage(&mut self, path: String, stage: bool, cx: &mut Context<Self>) {
        let label = if stage { "Stage" } else { "Unstage" };
        self.git_op(
            label,
            move |root| {
                if stage {
                    git::stage(&root, &path)?
                } else {
                    git::unstage(&root, &path)?
                }
                Ok(String::new())
            },
            |this, _| {
                if let Some(p) = &mut this.project {
                    p.git.message = None;
                }
            },
            cx,
        );
    }

    pub fn git_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let message = p.git.commit_input.read(cx).value().to_string();
        if message.trim().is_empty() {
            self.toast("Enter a commit message", true, cx);
            return;
        }
        if p.git.status.as_ref().is_none_or(|s| s.staged_count() == 0) {
            self.toast(
                "Nothing is staged. Stage the files to commit first.",
                true,
                cx,
            );
            return;
        }
        let input = p.git.commit_input.clone();
        let handle = window.window_handle();
        self.git_op(
            "Commit",
            move |root| {
                let id = git::commit(&root, &message)?;
                Ok(format!("Committed {id}"))
            },
            move |_, cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    input.update(cx, |i, cx| i.set_value("", window, cx))
                });
            },
            cx,
        );
    }

    pub fn git_push(&mut self, cx: &mut Context<Self>) {
        self.git_op(
            "Push",
            |root| {
                git::push(&root)?;
                Ok("Pushed".into())
            },
            |this, _| {
                if let Some(p) = &mut this.project {
                    p.git.just_pushed = true;
                }
            },
            cx,
        );
    }

    pub fn git_fetch(&mut self, cx: &mut Context<Self>) {
        self.git_op("Fetch", |root| git::fetch(&root), |_, _| {}, cx);
    }

    pub fn git_pull(&mut self, cx: &mut Context<Self>) {
        self.git_op("Pull", |root| git::pull(&root), |_, _| {}, cx);
    }

    pub fn git_switch(&mut self, name: String, create: bool, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.project {
            p.git.branch_menu = false;
        }
        self.git_op(
            "Switch branch",
            move |root| {
                git::switch_branch(&root, &name, create)?;
                Ok(format!("On branch {name}"))
            },
            |_, _| {},
            cx,
        );
    }

    pub fn open_pull_request(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let (Some(root), Some(branch)) = (
            p.git.root.clone(),
            p.git.status.as_ref().and_then(|s| s.branch.clone()),
        ) else {
            return;
        };
        let remote = git::remote_url(&root, "origin").unwrap_or_default();
        match git::pull_request_url(&remote, &branch) {
            Some(url) => cx.open_url(&url),
            None => self.toast("The origin remote is not on GitHub", true, cx),
        }
    }

    pub fn show_file_diff(
        &mut self,
        change: &FileChange,
        staged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &self.project else { return };
        let Some(root) = p.git.root.clone() else {
            return;
        };
        let path = change.path.clone();
        let untracked = change.untracked;
        let title = format!(
            "{} ({})",
            PathBuf::from(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            if staged { "staged" } else { "changes" }
        );
        let key = format!("git:{}:{path}", if staged { "s" } else { "u" });
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { git::diff_file(&root, &path, staged, untracked) })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(patch) => this.open_diff_tab(key, title, patch, None, window, cx),
                Err(e) => this.toast(e.to_string(), true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn prompt_clone_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(overlays::Overlay::prompt(
            "Clone Repository",
            "Repository URL. You will choose the destination folder next.",
            "",
            overlays::PromptKind::CloneUrl,
            window,
            cx,
        ));
        cx.notify();
    }

    /// Ask where to put the clone, then clone in the background.
    pub fn clone_with_destination(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("repo")
            .to_string();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(parent) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    this.start_clone(url, parent.join(name), window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn start_clone(
        &mut self,
        url: String,
        dest: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clone_progress = Some(format!("Cloning {url}…"));
        self.toast(format!("Cloning into {}…", dest.display()), false, cx);
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
        let (done_tx, done_rx) = futures::channel::oneshot::channel();
        let d = dest.clone();
        std::thread::Builder::new()
            .name("git-clone".into())
            .spawn(move || {
                crate::platform::tools::wait_for_environment();
                let res = git::clone(&url, &d, |line| {
                    let _ = tx.unbounded_send(line);
                });
                let _ = done_tx.send(res);
            })
            .ok();
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while let Some(line) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.clone_progress = Some(line);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = done_rx.await;
            this.update_in(cx, |this, window, cx| {
                this.clone_progress = None;
                match result {
                    Ok(Ok(())) => this.request_open_project(dest, window, cx),
                    Ok(Err(e)) => this.toast(format!("Clone failed: {e}"), true, cx),
                    Err(_) => this.toast("Clone failed", true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn render_scm(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = pal(cx);
        let _ = window;
        let graph_section = self
            .project
            .as_ref()
            .is_some_and(|p| p.git.is_repo)
            .then(|| self.render_graph_section(cx));
        let Some(p) = &self.project else {
            return div().into_any_element();
        };
        if !p.git.is_repo {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(section_title("SOURCE CONTROL", palette))
                .child(
                    div()
                        .px_3()
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child("This folder is not a Git repository."),
                )
                .into_any_element();
        }
        let git = &p.git;
        let st = git.status.clone().unwrap_or_default();
        let staged: Vec<FileChange> = st
            .files
            .iter()
            .filter(|f| f.staged.is_some())
            .cloned()
            .collect();
        let changes: Vec<FileChange> = st
            .files
            .iter()
            .filter(|f| f.unstaged.is_some() || f.untracked)
            .cloned()
            .collect();
        let busy = git.busy.clone();
        let branch = app::branch_label(&st);
        let in_conflict = st.merging || st.rebasing || st.conflicts() > 0;
        let conflict_note = in_conflict.then(|| {
            let what = if st.rebasing {
                "Rebase in progress"
            } else if st.merging {
                "Merge in progress"
            } else {
                "Unmerged files"
            };
            format!(
                "{what}: {} conflicted file(s). Fix them, stage, then commit.",
                st.conflicts()
            )
        });
        let message = git.message.clone();
        let menu = git.branch_menu.then(|| git.branches.clone());
        let can_pr = git.just_pushed && st.branch.is_some();
        let input = git.commit_input.clone();

        let mut rows: Vec<AnyElement> = vec![];
        let row = |change: &FileChange, is_staged: bool, ix: usize, cx: &mut Context<Self>| {
            let c = change.clone();
            let c2 = change.clone();
            let name = PathBuf::from(&change.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let dir = PathBuf::from(&change.path)
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let state = if is_staged {
                change.staged.unwrap_or(change.state)
            } else {
                change.state
            };
            let id = ix * 2 + is_staged as usize;
            div()
                .id(("chg", id))
                .group("chg")
                .h(px(24.))
                .flex()
                .items_center()
                .gap_1()
                .pl_3()
                .pr_1()
                .hover(move |s| s.bg(palette.hover))
                .cursor_pointer()
                .on_click(
                    cx.listener(move |this, _, w, cx| this.show_file_diff(&c, is_staged, w, cx)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(palette.fg)
                        .whitespace_nowrap()
                        .child(name),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child(dir),
                )
                .child(
                    div()
                        .w(px(14.))
                        .text_size(px(11.))
                        .text_color(if state == git::status::FileState::Conflicted {
                            palette.danger
                        } else {
                            palette.accent
                        })
                        .child(state.letter()),
                )
                .child(
                    div()
                        .id(("chg-act", id))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded(px(3.))
                        .hover(move |s| s.bg(palette.active))
                        .text_size(px(14.))
                        .text_color(palette.muted)
                        .child(if is_staged { "−" } else { "+" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.git_stage(c2.path.clone(), !is_staged, cx);
                        })),
                )
                .into_any_element()
        };
        if !staged.is_empty() {
            rows.push(
                section_title(format!("STAGED ({})", staged.len()), palette).into_any_element(),
            );
            for (i, c) in staged.iter().enumerate() {
                rows.push(row(c, true, i, cx));
            }
        }
        rows.push(
            section_title(format!("CHANGES ({})", changes.len()), palette).into_any_element(),
        );
        for (i, c) in changes.iter().enumerate() {
            rows.push(row(c, false, i, cx));
        }
        if changes.is_empty() && staged.is_empty() {
            rows.push(
                div()
                    .px_3()
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child("No changes.")
                    .into_any_element(),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h(px(30.))
                    .pl_3()
                    .pr_1()
                    .gap_1()
                    .child(
                        div()
                            .id("scm-branch")
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(p) = &mut this.project {
                                    p.git.branch_menu = !p.git.branch_menu;
                                }
                                cx.notify();
                            }))
                            .child(icon_el("git-branch", 12., palette.muted))
                            .child(div().text_size(px(12.)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(branch))
                            .child(icon_el("chevrons-up-down", 11., palette.muted)),
                    )
                    .child(icon_button("scm-pull", "arrow-down-to-line", "Pull").on_click(cx.listener(|this, _, _, cx| this.git_pull(cx))))
                    .child(icon_button("scm-push", "arrow-up-from-line", "Push").on_click(cx.listener(|this, _, _, cx| this.git_push(cx))))
                    .child(icon_button("scm-refresh", "refresh-cw", "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh_git(cx)))),
            )
            .when_some(menu, |d, branches| {
                d.child(
                    div()
                        .id("branch-menu")
                        .flex_shrink_0()
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .mx_2()
                        .mb_1()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(palette.border)
                        .bg(palette.bar)
                        .children(branches.into_iter().enumerate().map(|(i, b)| {
                            let name = b.clone();
                            div()
                                .id(("branch", i))
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .px_2()
                                .text_size(px(12.))
                                .hover(move |s| s.bg(palette.hover))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| this.git_switch(name.clone(), false, cx)))
                                .child(b)
                        }))
                        .child(
                            div()
                                .id("branch-new")
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .px_2()
                                .text_size(px(12.))
                                .text_color(palette.accent)
                                .hover(move |s| s.bg(palette.hover))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.overlay = Some(overlays::Overlay::prompt("New Branch", "Created from the current commit. Your changes stay in place.", "", overlays::PromptKind::NewBranch, w, cx));
                                    cx.notify();
                                }))
                                .child("+ New branch…"),
                        ),
                )
            })
            .when_some(conflict_note, |d, n| d.child(div().mx_2().mb_1().p_2().rounded(px(4.)).bg(palette.hover).text_size(px(12.)).text_color(palette.danger).child(n)))
            .child(tab_moves_focus(div()).px_2().flex_shrink_0().child(Textarea::new(&input).h(px(62.))))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(primary_button("scm-commit", format!("Commit {} staged", st.staged_count()), palette).on_click(cx.listener(|this, _, w, cx| this.git_commit(w, cx))))
                    .child(div().flex_1())
                    .when(can_pr, |d| d.child(button("scm-pr", "Open Pull Request", palette).on_click(cx.listener(|this, _, _, cx| this.open_pull_request(cx))))),
            )
            .when_some(busy, |d, b| d.child(div().px_3().pb_1().flex_shrink_0().text_size(px(11.)).text_color(palette.accent).child(format!("{b}…"))))
            .when_some(message, |d, (m, err)| d.child(div().px_3().pb_1().flex_shrink_0().max_h(px(120.)).overflow_hidden().text_size(px(11.)).text_color(if err { palette.danger } else { palette.muted }).child(m)))
            .child(hsep(palette))
            .child(div().id("scm-list").flex_1().min_h_0().overflow_y_scroll().children(rows))
            .children(graph_section)
            .into_any_element()
    }
}
