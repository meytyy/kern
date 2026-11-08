//! Source Control → Graph: real commits with lanes, loaded in pages off the UI
//! thread. Viewing history never changes files, the index or the branch.
use super::app::*;
use super::theme::{Palette, pal};
use super::widgets::*;
use crate::git::graph::{self, Commit, CommitFile, GraphRow, Lanes, RefKind};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

const PAGE: usize = 200;
const ROW_H: f32 = 22.;
const LANE_W: f32 = 12.;

#[derive(Default)]
pub struct GraphState {
    pub commits: Vec<Commit>,
    pub rows: Vec<GraphRow>,
    pub lanes: Lanes,
    pub loading: bool,
    pub exhausted: bool,
    pub error: Option<String>,
    pub fingerprint: String,
    pub generation: u64,
    pub scroll: UniformListScrollHandle,
    pub detail: Option<CommitDetail>,
}

pub struct CommitDetail {
    pub commit: Commit,
    pub message: String,
    /// Which parent the file list is compared with.
    pub parent_ix: usize,
    pub files: Vec<CommitFile>,
    pub error: Option<String>,
}

fn lane_color(i: usize, p: Palette) -> Hsla {
    const DARK: [u32; 7] = [
        0xc5b4f0, 0x8fb8f6, 0x7fdbca, 0xf0b37e, 0xf28fa0, 0xa9dc9d, 0xe6c07b,
    ];
    const LIGHT: [u32; 7] = [
        0x6a35c9, 0x1f5fc4, 0x0b7a84, 0xb4540a, 0xb3263e, 0x1e7a45, 0x8a5a00,
    ];
    rgb(if p.dark { DARK[i % 7] } else { LIGHT[i % 7] }).into()
}

fn paint_row(row: &GraphRow, head: bool, bounds: Bounds<Pixels>, p: Palette, window: &mut Window) {
    let x = |col: usize| bounds.origin.x + px(LANE_W * col as f32 + LANE_W / 2.);
    let (top, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
    let mid = bounds.origin.y + bounds.size.height / 2.;
    let line = |from: Point<Pixels>,
                ctrl: Option<Point<Pixels>>,
                to: Point<Pixels>,
                color: Hsla,
                window: &mut Window| {
        let mut b = PathBuilder::stroke(px(1.5));
        b.move_to(from);
        match ctrl {
            Some(c) => b.curve_to(to, c),
            None => b.line_to(to),
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };
    for &(col, color) in &row.passing {
        line(
            point(x(col), top),
            None,
            point(x(col), bottom),
            lane_color(color, p),
            window,
        );
    }
    let center = point(x(row.col), mid);
    let own = lane_color(row.color, p);
    if row.top {
        line(point(x(row.col), top), None, center, own, window);
    }
    if row.bottom {
        line(center, None, point(x(row.col), bottom), own, window);
    }
    for &(col, color) in &row.merge_in {
        line(
            point(x(col), top),
            Some(point(x(col), mid)),
            center,
            lane_color(color, p),
            window,
        );
    }
    for &(col, color) in &row.fork_out {
        line(
            center,
            Some(point(x(col), mid)),
            point(x(col), bottom),
            lane_color(color, p),
            window,
        );
    }
    let radius = if head { 4.5 } else { 3.5 };
    let dot = Bounds::new(
        center - point(px(radius), px(radius)),
        size(px(radius * 2.), px(radius * 2.)),
    );
    window.paint_quad(fill(dot, own).corner_radii(px(radius)));
    if head {
        let inner = Bounds::new(center - point(px(1.5), px(1.5)), size(px(3.), px(3.)));
        window.paint_quad(fill(inner, p.bg).corner_radii(px(1.5)));
    }
}

impl KernApp {
    fn graph(&mut self) -> Option<&mut GraphState> {
        self.project.as_mut().map(|p| &mut p.git.graph)
    }

    pub fn reload_graph(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.graph() {
            g.generation += 1;
            g.commits.clear();
            g.rows.clear();
            g.lanes = Lanes::default();
            g.exhausted = false;
            g.loading = false;
            g.error = None;
        }
        self.load_more_graph(cx);
    }

    pub fn load_more_graph(&mut self, cx: &mut Context<Self>) {
        let all = self.settings.graph_all_branches;
        let Some(p) = &mut self.project else { return };
        let Some(root) = p.git.root.clone() else {
            return;
        };
        let g = &mut p.git.graph;
        if g.loading || g.exhausted {
            return;
        }
        g.loading = true;
        let skip = g.commits.len();
        let generation = g.generation;
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_spawn(async move { graph::load_page(&root, all, skip, PAGE) })
                .await;
            this.update(cx, |this, cx| {
                let Some(g) = this.graph() else { return };
                if g.generation != generation {
                    return;
                }
                g.loading = false;
                match page {
                    Ok(commits) => {
                        g.exhausted = commits.len() < PAGE;
                        for c in commits {
                            let row = g.lanes.advance(&c);
                            g.rows.push(row);
                            g.commits.push(c);
                        }
                    }
                    Err(e) => g.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn toggle_graph(&mut self, cx: &mut Context<Self>) {
        self.settings.graph_hidden = !self.settings.graph_hidden;
        self.persist();
        if !self.settings.graph_hidden
            && self
                .project
                .as_ref()
                .is_some_and(|p| p.git.graph.commits.is_empty())
        {
            self.reload_graph(cx);
        }
        cx.notify();
    }

    pub fn set_graph_all(&mut self, all: bool, cx: &mut Context<Self>) {
        self.settings.graph_all_branches = all;
        self.persist();
        self.reload_graph(cx);
    }

    pub fn select_commit(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let Some(commit) = p.git.graph.commits.get(ix).cloned() else {
            return;
        };
        p.git.graph.detail = Some(CommitDetail {
            message: commit.subject.clone(),
            commit,
            parent_ix: 0,
            files: vec![],
            error: None,
        });
        self.load_commit_detail(cx);
    }

    pub fn select_commit_parent(&mut self, parent_ix: usize, cx: &mut Context<Self>) {
        if let Some(d) = self.graph().and_then(|g| g.detail.as_mut()) {
            d.parent_ix = parent_ix;
            d.files.clear();
        }
        self.load_commit_detail(cx);
    }

    fn load_commit_detail(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.project else { return };
        let Some(root) = p.git.root.clone() else {
            return;
        };
        let Some(d) = &p.git.graph.detail else { return };
        let hash = d.commit.hash.clone();
        let parent = d.commit.parents.get(d.parent_ix).cloned();
        cx.spawn(async move |this, cx| {
            let h = hash.clone();
            let result = cx
                .background_spawn(async move {
                    let message = graph::message(&root, &h)?;
                    let files = graph::changed_files(&root, &h, parent.as_deref())?;
                    anyhow::Ok((message, files))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(d) = this.graph().and_then(|g| g.detail.as_mut()) else {
                    return;
                };
                if d.commit.hash != hash {
                    return;
                }
                match result {
                    Ok((message, files)) => {
                        d.message = message;
                        d.files = files;
                    }
                    Err(e) => d.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn open_commit_file_diff(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &self.project else { return };
        let Some(root) = p.git.root.clone() else {
            return;
        };
        let Some(d) = &p.git.graph.detail else { return };
        let hash = d.commit.hash.clone();
        let parent = d.commit.parents.get(d.parent_ix).cloned();
        let name = PathBuf::from(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let title = format!("{name} @ {}", d.commit.short());
        let key = format!("commit:{hash}:{}:{path}", d.parent_ix);
        let note = (d.commit.parents.len() > 1).then(|| {
            format!(
                "Merge commit: compared with parent {} of {} ({})",
                d.parent_ix + 1,
                d.commit.parents.len(),
                &d.commit.parents[d.parent_ix][..8.min(d.commit.parents[d.parent_ix].len())]
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let patch = cx
                .background_spawn(async move {
                    graph::file_patch(&root, &hash, parent.as_deref(), &path)
                })
                .await;
            this.update_in(cx, |this, window, cx| match patch {
                Ok(patch) => this.open_diff_tab(key, title, patch, note, window, cx),
                Err(e) => this.toast(e.to_string(), true, cx),
            })
            .ok();
        })
        .detach();
    }

    fn render_commit_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = pal(cx);
        let d = self.project.as_ref()?.git.graph.detail.as_ref()?;
        let c = &d.commit;
        let hash = c.hash.clone();
        let parents = c.parents.clone();
        let parent_ix = d.parent_ix;
        let line = |t: String, col: Hsla| div().text_size(px(11.)).text_color(col).child(t);
        Some(
            div()
                .id("commit-detail")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .max_h(px(240.))
                .overflow_y_scroll()
                .gap(px(2.))
                .p_2()
                .border_t_1()
                .border_color(p.border)
                .bg(p.editor)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(c.subject.clone()),
                        )
                        .child(
                            icon_button("commit-copy", "copy", "Copy Commit Hash").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
                                    this.toast("Commit hash copied", false, cx);
                                }),
                            ),
                        )
                        .child(icon_button("commit-close", "close", "Close").on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(g) = this.graph() {
                                    g.detail = None;
                                }
                                cx.notify();
                            }),
                        )),
                )
                .when(d.message.trim() != c.subject.trim(), |x| {
                    x.child(line(d.message.clone(), p.fg))
                })
                .child(line(format!("{} <{}>", c.author, c.email), p.muted))
                .child(line(c.date.clone(), p.muted))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_family(crate::core::settings::MONO_FONT)
                        .text_color(p.muted)
                        .child(c.hash.clone()),
                )
                .when(parents.len() > 1, |x| {
                    x.child(line(
                        "Merge commit. Files are compared with:".to_string(),
                        p.warning,
                    ))
                    .child(div().flex().gap_1().children(
                        parents.iter().enumerate().map(|(i, h)| {
                            let label = format!("parent {} {}", i + 1, &h[..7.min(h.len())]);
                            (if i == parent_ix {
                                primary_button(("parent", i), label, p)
                            } else {
                                button(("parent", i), label, p)
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.select_commit_parent(i, cx)),
                            )
                        }),
                    ))
                })
                .when(parents.is_empty(), |x| {
                    x.child(line("Root commit".to_string(), p.muted))
                })
                .when_some(d.error.clone(), |x, e| x.child(line(e, p.danger)))
                .children(d.files.iter().enumerate().map(|(i, f)| {
                    let path = f.path.clone();
                    div()
                        .id(("cfile", i))
                        .flex()
                        .gap_2()
                        .h(px(20.))
                        .items_center()
                        .px_1()
                        .rounded(px(3.))
                        .text_size(px(12.))
                        .hover(move |s| s.bg(p.hover))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.open_commit_file_diff(path.clone(), w, cx)
                        }))
                        .child(
                            div()
                                .w(px(12.))
                                .text_size(px(11.))
                                .text_color(p.accent)
                                .child(f.status.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(f.path.clone()),
                        )
                }))
                .into_any_element(),
        )
    }

    pub fn render_graph_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let hidden = self.settings.graph_hidden;
        let all = self.settings.graph_all_branches;
        let narrow = self.settings.layout.sidebar_width < 250.;
        let header = div()
            .id("graph-header")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .h(px(26.))
            .pl_1()
            .pr_1()
            .border_t_1()
            .border_color(p.border)
            .child(
                div()
                    .id("graph-toggle")
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_graph(cx)))
                    .child(icon_el(
                        if hidden {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        },
                        12.,
                        p.muted,
                    ))
                    .child(div().text_size(px(11.)).text_color(p.muted).child("GRAPH")),
            )
            .when(!hidden, |d| {
                d.child(
                    div()
                        .id("graph-scope")
                        .px_1()
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .rounded(px(3.))
                        .text_size(px(10.))
                        .text_color(p.muted)
                        .hover(move |s| s.bg(p.hover))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_graph_all(!this.settings.graph_all_branches, cx)
                        }))
                        .tooltip(tip("Show commits of all branches or only the current one"))
                        .child(match (all, narrow) {
                            (true, false) => "All Branches",
                            (false, false) => "Current Branch",
                            (true, true) => "All",
                            (false, true) => "Current",
                        }),
                )
                .child(
                    icon_button("graph-fetch", "arrow-down-to-line", "Fetch")
                        .on_click(cx.listener(|this, _, _, cx| this.git_fetch(cx))),
                )
                .child(
                    icon_button("graph-refresh", "refresh-cw", "Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.reload_graph(cx))),
                )
            });
        if hidden {
            return header.into_any_element();
        }
        let detail = self.render_commit_detail(cx);
        let Some(project) = &self.project else {
            return div().into_any_element();
        };
        let g = &project.git.graph;
        let commits = g.commits.clone();
        let rows = g.rows.clone();
        let selected = g.detail.as_ref().map(|d| d.commit.hash.clone());
        let count = commits.len();
        let width = rows
            .iter()
            .map(|r| {
                r.passing
                    .iter()
                    .map(|x| x.0)
                    .chain(r.fork_out.iter().map(|x| x.0))
                    .chain(r.merge_in.iter().map(|x| x.0))
                    .chain([r.col])
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0)
            + 1;
        let graph_w = (width.min(14) as f32) * LANE_W + 4.;
        let error = g.error.clone();
        let loading = g.loading;
        let scroll = g.scroll.clone();
        let list = uniform_list(
            "commit-graph",
            count,
            cx.processor(
                move |this: &mut KernApp, range: std::ops::Range<usize>, _w, cx| {
                    if range.end + 30 >= count {
                        this.load_more_graph(cx);
                    }
                    let mut out = Vec::with_capacity(range.len());
                    for ix in range {
                        let c = commits[ix].clone();
                        let row = rows[ix].clone();
                        let head = c.is_head;
                        let is_selected = selected.as_ref() == Some(&c.hash);
                        let hash = c.hash.clone();
                        out.push(
                            div()
                                .id(("commit", ix))
                                .group("commit")
                                .h(px(ROW_H))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_1()
                                .pr_1()
                                .when(is_selected, |d| d.bg(p.accent_soft))
                                .hover(move |s| s.bg(p.hover))
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.select_commit(ix, cx)),
                                )
                                .child(
                                    canvas(
                                        |_, _, _| {},
                                        move |bounds, _, window, _| {
                                            paint_row(&row, head, bounds, p, window)
                                        },
                                    )
                                    .w(px(graph_w))
                                    .h(px(ROW_H))
                                    .flex_shrink_0(),
                                )
                                .child(
                                    // The message keeps room; branch labels give way to it.
                                    div()
                                        .flex_1()
                                        .min_w(px(60.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(12.))
                                        .when(head, |d| d.font_weight(FontWeight::SEMIBOLD))
                                        .child(c.subject.clone()),
                                )
                                .children({
                                    // One label and "+N"; every name is in the tooltip.
                                    let refs: Vec<_> = c
                                        .refs
                                        .iter()
                                        .filter(|r| !(r.kind == RefKind::Head && r.name == "HEAD"))
                                        .collect();
                                    let all_names = refs
                                        .iter()
                                        .map(|r| r.name.clone())
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    let more = refs.len().saturating_sub(1);
                                    refs.first().map(|r| {
                                        let color = match r.kind {
                                            RefKind::Head => p.accent,
                                            RefKind::Branch => p.success,
                                            RefKind::Remote => p.muted,
                                            RefKind::Tag => p.warning,
                                        };
                                        div()
                                            .id(("ref", ix))
                                            .flex()
                                            .flex_shrink_0()
                                            .gap_0p5()
                                            .items_center()
                                            .tooltip(tip(all_names))
                                            .child(
                                                div()
                                                    .max_w(px(80.))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .px_1()
                                                    .rounded(px(3.))
                                                    .border_1()
                                                    .border_color(color)
                                                    .text_size(px(10.))
                                                    .text_color(color)
                                                    .child(r.name.clone()),
                                            )
                                            .when(more > 0, |d| {
                                                d.child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(p.muted)
                                                        .child(format!("+{more}")),
                                                )
                                            })
                                    })
                                })
                                .child(
                                    div()
                                        .id(("copy-hash", ix))
                                        .invisible()
                                        .group_hover("commit", |s| s.visible())
                                        .flex_shrink_0()
                                        .size(px(18.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.))
                                        .hover(move |s| s.bg(p.active))
                                        .tooltip(tip("Copy commit hash"))
                                        .child(icon_el("copy", 11., p.muted))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                hash.clone(),
                                            ));
                                            this.toast("Commit hash copied", false, cx);
                                        })),
                                )
                                .into_any_element(),
                        );
                    }
                    out
                },
            ),
        )
        .track_scroll(&scroll)
        .flex_1()
        .min_h_0();
        let handle = self.drag_handle(Dragging::Graph, cx);
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .h(px(self.graph_height()))
            .child(handle)
            .child(header)
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(p.danger)
                        .child(e),
                )
            })
            .when(count == 0 && !loading, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .child("No commits yet."),
                )
            })
            .child(list)
            .children(detail)
            .into_any_element()
    }

    pub fn graph_height(&self) -> f32 {
        if self.settings.graph_height < 80. {
            280.
        } else {
            self.settings.graph_height
        }
    }
}
