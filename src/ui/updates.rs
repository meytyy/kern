//! Update notice: one background check per launch, plus Help → Check for Updates.
//! Opens the release file in the browser; KERN itself never downloads it.
use super::app::*;
use super::theme::pal;
use super::widgets::*;
use crate::core::update::{self, Outcome};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

#[derive(Default)]
pub struct UpdateState {
    pub checking: bool,
    /// The background check has shown its notice: it does not come back this launch.
    pub notified: bool,
    pub notice: Option<UpdateNotice>,
}

#[derive(Clone)]
pub struct UpdateNotice {
    pub version: String,
    pub download_url: String,
}

impl KernApp {
    /// After startup, quietly: errors only go to the log.
    pub fn schedule_update_check(&mut self, cx: &mut Context<Self>) {
        // Scripted test runs stay offline unless they point at a fixture.
        if cfg!(feature = "autotest") && std::env::var_os("KERN_UPDATE_URL").is_none() {
            return;
        }
        if std::env::var_os("KERN_NO_UPDATE_CHECK").is_some() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            this.update(cx, |this, cx| this.check_for_updates(false, cx))
                .ok();
        })
        .detach();
    }

    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.updates.checking {
            if manual {
                self.toast("Already checking for updates…", false, cx);
            }
            return;
        }
        self.updates.checking = true;
        if manual {
            self.toast("Checking for updates…", false, cx);
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async { update::check(Duration::from_secs(15)) })
                .await;
            this.update(cx, |this, cx| {
                this.updates.checking = false;
                this.update_result(result, manual, cx);
            })
            .ok();
        })
        .detach();
    }

    fn update_result(
        &mut self,
        result: Result<Outcome, String>,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        let platform = update::platform_label(std::env::consts::OS, std::env::consts::ARCH);
        let current = update::current_version();
        match result {
            Ok(Outcome::Available {
                version,
                download_url,
                ..
            }) => {
                if !manual && self.updates.notified {
                    return;
                }
                self.updates.notified = true;
                self.updates.notice = Some(UpdateNotice {
                    version: format!("v{version}"),
                    download_url,
                });
                if manual {
                    self.toast = None;
                }
            }
            Ok(Outcome::NoBuild { version, .. }) => {
                log::info!("update v{version} has no {platform} build");
                if manual {
                    self.toast(
                        format!("KERN v{version} is out, but it has no {platform} build yet"),
                        false,
                        cx,
                    );
                }
            }
            Ok(Outcome::UpToDate { .. }) => {
                if manual {
                    self.toast(format!("KERN v{current} is up to date"), false, cx);
                }
            }
            Err(e) => {
                log::warn!("update check failed: {e}");
                if manual {
                    self.toast(format!("Cannot check for updates: {e}"), true, cx);
                }
            }
        }
        cx.notify();
    }

    pub fn render_update_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let notice = self.updates.notice.clone()?;
        let p = pal(cx);
        let url = notice.download_url.clone();
        Some(
            div()
                .id("update-notice")
                .absolute()
                .bottom(px(34.))
                .right(px(12.))
                .w(px(300.))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(8.))
                .bg(p.bar)
                .border_1()
                .border_color(p.border)
                .shadow_md()
                .occlude()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon_el("arrow-down-to-line", 14., p.accent))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("KERN {} is available", notice.version)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_1()
                        .child(button("update-later", "Later", p).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.updates.notice = None;
                                cx.notify();
                            },
                        )))
                        .child(
                            primary_button("update-download", "Download Update", p).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.updates.notice = None;
                                    if let Err(e) = open::that_detached(&url) {
                                        this.toast(
                                            format!("Cannot open the browser: {e}"),
                                            true,
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                }),
                            ),
                        ),
                ),
        )
    }
}
