//! Start screen.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

impl KernApp {
    pub fn render_start(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = pal(cx);
        let action =
            |id: &'static str, icon: &'static str, label: &'static str, hint: &'static str| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_3()
                    .w(px(340.))
                    .h(px(44.))
                    .px_3()
                    .rounded(px(6.))
                    .bg(palette.panel)
                    .border_1()
                    .border_color(palette.border)
                    .hover(move |s| s.bg(palette.hover))
                    .cursor_pointer()
                    .child(icon_el(icon, 16., palette.accent))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).child(label))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.muted)
                                    .child(hint),
                            ),
                    )
            };
        let recents: Vec<AnyElement> = self
            .settings
            .recent_projects
            .iter()
            .take(8)
            .enumerate()
            .map(|(ix, path)| {
                let target = path.clone();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let exists = path.is_dir();
                div()
                    .id(("recent", ix))
                    .flex()
                    .flex_col()
                    .w(px(340.))
                    .px_3()
                    .py_1()
                    .rounded(px(4.))
                    .hover(move |s| s.bg(palette.hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        if target.is_dir() {
                            this.request_open_project(target.clone(), w, cx);
                        } else {
                            this.toast(format!("Folder not found: {}", target.display()), true, cx);
                        }
                    }))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(if exists { palette.fg } else { palette.muted })
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(path.display().to_string()),
                    )
                    .into_any_element()
            })
            .collect();
        let progress = self.clone_progress.clone();
        div()
            .id("start")
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .overflow_y_scroll()
            .child(logo_image(72.))
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("KERN"),
            )
            .child(div().h(px(10.)))
            .child(
                action(
                    "start-open",
                    "folder-open",
                    "Open Folder",
                    "Work on a local project",
                )
                .on_click(cx.listener(|this, _, w, cx| this.pick_folder(w, cx))),
            )
            .child(
                action(
                    "start-clone",
                    "git-branch",
                    "Clone Repository",
                    "From any Git URL; no account needed",
                )
                .on_click(cx.listener(|this, _, w, cx| this.prompt_clone_url(w, cx))),
            )
            .child(
                action(
                    "start-github",
                    "github",
                    "Clone from GitHub",
                    "Sign in and pick a repository",
                )
                .on_click(cx.listener(|this, _, w, cx| this.open_github_picker(w, cx))),
            )
            .when_some(progress, |d, msg| {
                d.child(
                    div()
                        .w(px(340.))
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child(msg),
                )
            })
            .when(!recents.is_empty(), |d| {
                d.child(div().h(px(10.)))
                    .child(
                        div()
                            .w(px(340.))
                            .px_3()
                            .text_size(px(11.))
                            .text_color(palette.muted)
                            .child("RECENT PROJECTS"),
                    )
                    .children(recents)
            })
    }
}
