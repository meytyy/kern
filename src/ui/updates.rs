//! Updates: one background check per launch, plus Help → Check for Updates.
//!
//! Windows updates from inside KERN: Download Update fetches the release's
//! installer and its signature into KERN's own folder, checks the Ed25519
//! signature and the bytes, then Restart to Update lets you save, quits and
//! runs the installer silently; it waits for KERN to close and starts the new
//! version. macOS opens the release file in the browser. The release page stays
//! in the Help menu as the manual way.
use super::app::*;
use super::overlays;
use super::theme::pal;
use super::widgets::*;
use crate::core::update::{self, Outcome, Package};
use crate::core::update_sign;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub const RELEASES_PAGE: &str = "https://github.com/meytyy/kern/releases/latest";

#[derive(Default)]
pub struct UpdateState {
    pub checking: bool,
    /// The background check has shown its notice: it does not come back this launch.
    pub notified: bool,
    /// The newest release this launch knows about, shown or not.
    pub offer: Option<UpdateNotice>,
    /// Whether the card is on screen; Later hides it and keeps the state.
    pub visible: bool,
    pub phase: Phase,
    cancel: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Default, PartialEq, Debug)]
pub enum Phase {
    #[default]
    Offer,
    Downloading {
        received: u64,
        total: u64,
    },
    Failed(String),
    /// A verified installer, ready to run.
    Ready(PathBuf),
}

#[derive(Clone)]
pub struct UpdateNotice {
    pub version: String,
    pub download_url: String,
    pub page_url: String,
    pub package: Package,
}

/// Installing from KERN: Windows. `KERN_INAPP_UPDATE=1` turns on the download
/// and its checks elsewhere, for testing them; installing stays Windows-only.
pub fn in_app_download() -> bool {
    cfg!(windows) || std::env::var_os("KERN_INAPP_UPDATE").is_some()
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
        // A download in progress or a ready update: show it instead of asking again.
        if manual
            && self.updates.offer.is_some()
            && !matches!(self.updates.phase, Phase::Offer | Phase::Failed(_))
        {
            self.updates.visible = true;
            cx.notify();
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
                page_url,
                package,
            }) => {
                if !manual && self.updates.notified {
                    return;
                }
                self.updates.notified = true;
                let version = format!("v{version}");
                let same = self
                    .updates
                    .offer
                    .as_ref()
                    .is_some_and(|o| o.version == version);
                if !same {
                    self.updates.phase = Phase::Offer;
                }
                self.updates.offer = Some(UpdateNotice {
                    version,
                    download_url,
                    page_url,
                    package,
                });
                self.updates.visible = true;
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

    /// Download Update: the signature first, then the package, then both checks.
    pub fn download_update(&mut self, cx: &mut Context<Self>) {
        let Some(offer) = self.updates.offer.clone() else {
            return;
        };
        if !in_app_download() {
            self.open_update_link(&offer.download_url, cx);
            return;
        }
        if matches!(self.updates.phase, Phase::Downloading { .. }) {
            return;
        }
        let Some(sig_url) = offer.package.signature_url.clone() else {
            self.updates.phase = Phase::Failed(
                "This release has no signature, so KERN will not install it. Download it manually."
                    .into(),
            );
            cx.notify();
            return;
        };
        let version = offer.version.trim_start_matches('v').to_string();
        let dir = crate::core::settings::config_dir().join("updates");
        // Only this version's files are kept.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if e.file_name() != version.as_str() {
                    std::fs::remove_dir_all(e.path()).ok();
                }
            }
        }
        let dir = dir.join(&version);
        let received = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        self.updates.cancel = Some(cancel.clone());
        self.updates.phase = Phase::Downloading {
            received: 0,
            total: offer.package.size,
        };
        self.updates.visible = true;
        cx.notify();
        let done = Arc::new(AtomicBool::new(false));
        // Progress: the UI reads the counter while the download runs.
        {
            let (received, done) = (received.clone(), done.clone());
            cx.spawn(async move |this, cx| {
                while !done.load(Ordering::Relaxed) {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    let r = received.load(Ordering::Relaxed);
                    let alive = this.update(cx, |this, cx| {
                        if let Phase::Downloading { received, .. } = &mut this.updates.phase {
                            *received = r;
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let job = {
            let (received, done) = (received.clone(), done.clone());
            let file = offer.package.file.clone();
            let url = offer.download_url.clone();
            let github_sha = offer.package.sha256.clone();
            move || -> Result<PathBuf, String> {
                let result = (|| {
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let text = update::fetch_text(&sig_url, Duration::from_secs(30))?;
                    let manifest = update_sign::verify(&text, update_sign::PUBLIC_KEY_HEX)?;
                    if manifest.version != version || manifest.file != file {
                        return Err("the signature is for a different release file".into());
                    }
                    let path = dir.join(&file);
                    // A complete, verified file from an earlier try is reused.
                    if update_sign::check_package(&path, &manifest, github_sha.as_deref()).is_ok() {
                        received.store(manifest.size, Ordering::Relaxed);
                        return Ok(path);
                    }
                    std::fs::remove_file(&path).ok();
                    update::download(&url, &path, &cancel, |n| {
                        received.store(n, Ordering::Relaxed)
                    })?;
                    if let Err(e) =
                        update_sign::check_package(&path, &manifest, github_sha.as_deref())
                    {
                        std::fs::remove_file(&path).ok();
                        return Err(e);
                    }
                    Ok(path)
                })();
                done.store(true, Ordering::Relaxed);
                result
            }
        };
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            this.update(cx, |this, cx| {
                this.updates.cancel = None;
                this.updates.phase = match result {
                    Ok(path) => Phase::Ready(path),
                    Err(e) if e == "cancelled" => Phase::Offer,
                    Err(e) => {
                        log::warn!("update download failed: {e}");
                        Phase::Failed(e)
                    }
                };
                this.updates.visible = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn cancel_update_download(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.updates.cancel {
            c.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Restart to Update: unsaved files can be saved first.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.updates.phase, Phase::Ready(_)) {
            return;
        }
        if self.has_dirty_tabs() {
            self.overlay = Some(overlays::Overlay::confirm(
                "Restart to Update",
                "Some files have unsaved changes. Save them before KERN restarts?",
                vec![
                    overlays::ConfirmChoice::new(
                        "Restart Without Saving",
                        overlays::ConfirmAction::DiscardAndUpdate,
                        false,
                    ),
                    overlays::ConfirmChoice::new(
                        "Save and Restart",
                        overlays::ConfirmAction::SaveAllAndUpdate,
                        true,
                    ),
                ],
            ));
            cx.notify();
            return;
        }
        self.apply_update(cx);
    }

    /// Starts the installer, then quits. If it cannot start, KERN keeps running.
    pub fn apply_update(&mut self, cx: &mut Context<Self>) {
        let Phase::Ready(path) = self.updates.phase.clone() else {
            return;
        };
        match crate::platform::launch_update(&path) {
            Ok(()) => {
                log::info!("starting the update installer {}", path.display());
                self.shutdown_processes();
                self.persist();
                cx.quit();
            }
            Err(e) => {
                self.toast(format!("The update did not start: {e}"), true, cx);
            }
        }
    }

    fn open_update_link(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Err(e) = open::that_detached(url) {
            self.toast(format!("Cannot open the browser: {e}"), true, cx);
        }
    }

    /// Help → Download Latest Release…: the manual way, always available.
    pub fn download_latest_manually(&mut self, cx: &mut Context<Self>) {
        let url = self
            .updates
            .offer
            .as_ref()
            .map(|o| o.page_url.clone())
            .unwrap_or_else(|| RELEASES_PAGE.to_string());
        self.open_update_link(&url, cx);
    }

    /// Help → About KERN.
    pub fn show_about(&mut self, cx: &mut Context<Self>) {
        let body = format!(
            "Version {} (build {}) · {}",
            env!("CARGO_PKG_VERSION"),
            env!("KERN_BUILD"),
            update::platform_label(std::env::consts::OS, std::env::consts::ARCH),
        );
        self.overlay = Some(overlays::Overlay::confirm("About KERN", body, vec![]));
        cx.notify();
    }

    pub fn render_update_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.updates.visible {
            return None;
        }
        let notice = self.updates.offer.clone()?;
        let p = pal(cx);
        let phase = self.updates.phase.clone();
        let title = match &phase {
            Phase::Offer => format!("KERN {} is available", notice.version),
            Phase::Downloading { .. } => format!("Downloading KERN {}…", notice.version),
            Phase::Failed(_) => "The update did not download".to_string(),
            Phase::Ready(_) => format!("KERN {} is ready to install", notice.version),
        };
        let later = |id: &'static str| {
            button(id, "Later", p).on_click(cx.listener(|this, _, _, cx| {
                this.updates.visible = false;
                cx.notify();
            }))
        };
        let buttons: Vec<AnyElement> = match &phase {
            Phase::Offer => vec![
                later("update-later").into_any_element(),
                primary_button("update-download", "Download Update", p)
                    .on_click(cx.listener(|this, _, _, cx| this.download_update(cx)))
                    .into_any_element(),
            ],
            Phase::Downloading { .. } => vec![
                button("update-cancel", "Cancel", p)
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_update_download(cx)))
                    .into_any_element(),
            ],
            Phase::Failed(_) => {
                let url = notice.download_url.clone();
                vec![
                    button("update-manual", "Download Manually", p)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_update_link(&url, cx)),
                        )
                        .into_any_element(),
                    primary_button("update-retry", "Retry", p)
                        .on_click(cx.listener(|this, _, _, cx| this.download_update(cx)))
                        .into_any_element(),
                ]
            }
            Phase::Ready(_) => vec![
                later("update-later-ready").into_any_element(),
                primary_button("update-restart", "Restart to Update", p)
                    .on_click(cx.listener(|this, _, _, cx| this.restart_to_update(cx)))
                    .into_any_element(),
            ],
        };
        let detail: Option<AnyElement> = match &phase {
            Phase::Downloading { received, total } => {
                let fraction = if *total > 0 {
                    (*received as f32 / *total as f32).clamp(0., 1.)
                } else {
                    0.
                };
                let mb = |b: u64| b as f32 / 1_048_576.;
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .id("update-progress")
                                .h(px(4.))
                                .w_full()
                                .rounded_full()
                                .bg(p.hover)
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(fraction))
                                        .rounded_full()
                                        .bg(p.accent_fill),
                                ),
                        )
                        .child(div().text_size(px(11.)).text_color(p.muted).child(format!(
                            "{:.0}% · {:.1} of {:.1} MB",
                            fraction * 100.,
                            mb(*received),
                            mb(*total)
                        )))
                        .into_any_element(),
                )
            }
            Phase::Failed(e) => Some(
                div()
                    .text_size(px(11.))
                    .text_color(p.danger)
                    .child(e.clone())
                    .into_any_element(),
            ),
            Phase::Ready(_) => Some(
                div()
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child("Signature and download checked. KERN restarts on the new version.")
                    .into_any_element(),
            ),
            Phase::Offer => None,
        };
        Some(
            div()
                .id("update-notice")
                .absolute()
                .bottom(px(34.))
                .right(px(12.))
                .w(px(320.))
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
                                .child(title),
                        ),
                )
                .children(detail)
                .child(div().flex().justify_end().gap_1().children(buttons)),
        )
    }
}
