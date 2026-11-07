//! Project-wide search panel.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use crate::workspace::search::{self, SearchMatch, SearchOptions};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SearchState {
    pub input: Entity<InputState>,
    pub results: Vec<SearchMatch>,
    pub running: bool,
    pub error: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub cancel: Arc<AtomicBool>,
    pub scroll: UniformListScrollHandle,
    pub _sub: Option<Subscription>,
    pub total: usize,
}

impl SearchState {
    pub fn new(window: &mut Window, cx: &mut Context<KernApp>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search in project"));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut KernApp, _, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.run_project_search(cx);
                }
            },
        );
        Self {
            input,
            results: vec![],
            running: false,
            error: None,
            case_sensitive: false,
            regex: false,
            cancel: Arc::new(AtomicBool::new(false)),
            scroll: UniformListScrollHandle::new(),
            _sub: Some(sub),
            total: 0,
        }
    }
}

impl KernApp {
    pub fn run_project_search(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let query = p.search.input.read(cx).value().to_string();
        p.search.cancel.store(true, Ordering::SeqCst);
        p.search.results.clear();
        p.search.error = None;
        p.search.total = 0;
        if query.is_empty() {
            p.search.running = false;
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        p.search.cancel = cancel.clone();
        p.search.running = true;
        let opts = SearchOptions {
            case_sensitive: p.search.case_sensitive,
            regex: p.search.regex,
        };
        let root = p.root.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let (done_tx, done_rx) = futures::channel::oneshot::channel();
        std::thread::Builder::new()
            .name("kern-search".into())
            .spawn(move || {
                let res = search::search_project(&root, &query, &opts, cancel, |batch| {
                    let _ = tx.unbounded_send(batch);
                });
                let _ = done_tx.send(res);
            })
            .ok();
        let token = p.search.cancel.clone();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(batch) = rx.next().await {
                let ok = this.update(cx, |this, cx| {
                    if let Some(p) = &mut this.project
                        && Arc::ptr_eq(&p.search.cancel, &token)
                    {
                        p.search.results.extend(batch);
                        cx.notify();
                    }
                });
                if ok.is_err() {
                    return;
                }
            }
            let result = done_rx.await;
            this.update(cx, |this, cx| {
                if let Some(p) = &mut this.project
                    && Arc::ptr_eq(&p.search.cancel, &token)
                {
                    p.search.running = false;
                    match result {
                        Ok(Ok(n)) => p.search.total = n,
                        Ok(Err(e)) => p.search.error = Some(e),
                        Err(_) => {}
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn render_search(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = pal(cx);
        let Some(p) = &self.project else {
            return div().into_any_element();
        };
        let s = &p.search;
        let root = p.root.clone();
        let results = s.results.clone();
        let count = results.len();
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .h(px(22.))
                .px_2()
                .rounded(px(3.))
                .text_size(px(11.))
                .text_color(if on { palette.accent } else { palette.muted })
                .when(on, |d| d.bg(palette.accent_soft))
                .hover(move |h| h.bg(palette.hover))
                .cursor_pointer()
                .child(label)
        };
        let status = if s.running {
            "Searching…".to_string()
        } else if let Some(e) = &s.error {
            format!("Invalid pattern: {e}")
        } else if count == 0 {
            String::new()
        } else if count >= search::MAX_RESULTS {
            format!("First {} matches", search::MAX_RESULTS)
        } else {
            format!("{count} matches")
        };
        let list = uniform_list(
            "search-results",
            count,
            cx.processor(move |_t, range: std::ops::Range<usize>, _w, cx| {
                let mut out = vec![];
                for ix in range {
                    let m = results[ix].clone();
                    let path = root.join(&m.path);
                    let (line, column) = (
                        m.line as u32,
                        m.text
                            .get(..m.column.min(m.text.len()))
                            .map_or(0, |s| s.chars().count()) as u32,
                    );
                    let show_file = ix == 0 || results[ix - 1].path != m.path;
                    out.push(
                        div()
                            .id(("res", ix))
                            .h(px(36.))
                            .w_full()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .px_3()
                            .hover(move |s| s.bg(palette.hover))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.open_file_async(
                                    path.clone(),
                                    true,
                                    Some((line, column)),
                                    w,
                                    cx,
                                )
                            }))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(if show_file {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    })
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!("{}:{}", m.path.display(), m.line + 1)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(m.text.trim_start().to_string()),
                            )
                            .into_any_element(),
                    );
                }
                out
            }),
        )
        .track_scroll(&s.scroll)
        .flex_1();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(section_title("SEARCH", palette))
            .child(div().px_2().flex_shrink_0().child(Input::new(&s.input)))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(
                        toggle("s-case", "Aa", s.case_sensitive).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(p) = &mut this.project {
                                    p.search.case_sensitive = !p.search.case_sensitive;
                                }
                                this.run_project_search(cx);
                            },
                        )),
                    )
                    .child(toggle("s-regex", ".*", s.regex).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(p) = &mut this.project {
                                p.search.regex = !p.search.regex;
                            }
                            this.run_project_search(cx);
                        },
                    )))
                    .child(div().flex_1())
                    .child(
                        button("s-go", "Search", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.run_project_search(cx))),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .flex_shrink_0()
                    .pb_1()
                    .text_size(px(11.))
                    .text_color(if s.error.is_some() {
                        palette.danger
                    } else {
                        palette.muted
                    })
                    .child(status),
            )
            .child(list)
            .into_any_element()
    }
}
