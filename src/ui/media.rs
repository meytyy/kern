//! Read-only image and video tabs. Decoding and probing happen off the UI thread.
use super::assets::icon;
use super::theme::{Palette, pal};
use super::widgets::*;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
use crate::platform::video::{Status, VideoPlayer};

/// Above this, a picture is not decoded: it could take gigabytes of memory.
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 268_000_000;
const MAX_VALIDATED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Raster,
    Svg,
    Video,
}

pub fn kind_for(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" => Some(MediaKind::Raster),
        "svg" => Some(MediaKind::Svg),
        "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" => Some(MediaKind::Video),
        _ => None,
    }
}

/// Blank every reference to a resource outside the file itself. Only local data
/// remains: `data:` URIs and `#fragments`. The renderer has no network either.
pub fn sanitize_svg(svg: &str) -> (String, usize) {
    use once_cell::sync::Lazy;
    static HREF: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(r#"(?i)((?:xlink:)?href\s*=\s*)("[^"]*"|'[^']*')"#).unwrap()
    });
    let mut blocked = 0;
    let out = HREF.replace_all(svg, |c: &regex::Captures<'_>| {
        let value = c[2].trim_matches(['"', '\'']);
        if value.starts_with('#') || value.starts_with("data:") {
            c[0].to_string()
        } else {
            blocked += 1;
            format!("{}\"#\"", &c[1])
        }
    });
    (out.into_owned(), blocked)
}

/// Pixel size from `width`/`height` or `viewBox`.
pub fn svg_size(svg: &str) -> Option<(f32, f32)> {
    use once_cell::sync::Lazy;
    static VIEW: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(
            r#"viewBox\s*=\s*["']\s*[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)[\s,]+([\d.]+)"#,
        )
        .unwrap()
    });
    static W: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(r#"<svg[^>]*\swidth\s*=\s*["']([\d.]+)(?:px)?["']"#).unwrap()
    });
    static H: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(r#"<svg[^>]*\sheight\s*=\s*["']([\d.]+)(?:px)?["']"#).unwrap()
    });
    if let (Some(w), Some(h)) = (W.captures(svg), H.captures(svg)) {
        return Some((w[1].parse().ok()?, h[1].parse().ok()?));
    }
    let v = VIEW.captures(svg)?;
    Some((v[1].parse().ok()?, v[2].parse().ok()?))
}

#[derive(Clone, Copy, PartialEq)]
enum Zoom {
    Fit,
    Factor(f32),
}

struct ImageState {
    source: Option<ImageSource>,
    size: Option<(f32, f32)>,
    format: String,
    alpha: bool,
    note: Option<String>,
    zoom: Zoom,
    pan: Point<Pixels>,
    last_drag: Option<Point<Pixels>>,
    view: Size<Pixels>,
    decode_failed: bool,
}

#[cfg(target_os = "macos")]
struct VideoState {
    player: VideoPlayer,
    frame: Option<SurfaceSource>,
    status: Status,
    opened_at: std::time::Instant,
    frames_seen: u64,
    scrubbing: bool,
    bar: Bounds<Pixels>,
    volume_bar: Bounds<Pixels>,
    current: f64,
    duration: f64,
    _ticker: Task<()>,
}

enum Content {
    Loading,
    Image(ImageState),
    #[cfg(target_os = "macos")]
    Video(VideoState),
    Failed(String),
}

pub struct MediaView {
    pub path: PathBuf,
    pub kind: MediaKind,
    content: Content,
}

impl MediaView {
    pub fn new(path: PathBuf, kind: MediaKind, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            path,
            kind,
            content: Content::Loading,
        };
        match kind {
            MediaKind::Raster | MediaKind::Svg => view.load_image(cx),
            MediaKind::Video => view.load_video(cx),
        }
        view
    }

    fn load_image(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let kind = self.kind;
        cx.spawn(async move |this, cx| {
            let probed = cx
                .background_spawn(async move { probe_image(&path, kind) })
                .await;
            this.update(cx, |this, cx| {
                this.content = match probed {
                    Ok(p) => Content::Image(ImageState {
                        source: Some(match p.svg_bytes {
                            Some(bytes) => ImageSource::Image(Arc::new(Image::from_bytes(
                                ImageFormat::Svg,
                                bytes,
                            ))),
                            None => ImageSource::from(this.path.clone()),
                        }),
                        size: Some(p.size),
                        format: p.format,
                        alpha: p.alpha,
                        note: p.note,
                        zoom: Zoom::Fit,
                        pan: Point::default(),
                        last_drag: None,
                        view: size(px(0.), px(0.)),
                        decode_failed: false,
                    }),
                    Err(e) => Content::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(not(target_os = "macos"))]
    fn load_video(&mut self, _cx: &mut Context<Self>) {
        self.content = Content::Failed(
            "This build plays video inside KERN only on macOS. Open it in the system player."
                .into(),
        );
    }

    #[cfg(target_os = "macos")]
    fn load_video(&mut self, cx: &mut Context<Self>) {
        match VideoPlayer::open(&self.path) {
            Ok(player) => {
                let ticker = cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(16))
                            .await;
                        if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                            break;
                        }
                    }
                });
                self.content = Content::Video(VideoState {
                    player,
                    frame: None,
                    status: Status::Loading,
                    opened_at: std::time::Instant::now(),
                    frames_seen: 0,
                    scrubbing: false,
                    bar: Bounds::default(),
                    volume_bar: Bounds::default(),
                    current: 0.0,
                    duration: 0.0,
                    _ticker: ticker,
                });
            }
            Err(e) => self.content = Content::Failed(e),
        }
    }

    #[cfg(target_os = "macos")]
    fn tick(&mut self, cx: &mut Context<Self>) {
        let Content::Video(v) = &mut self.content else {
            return;
        };
        let mut changed = false;
        let status = v.player.status();
        if status != v.status {
            v.status = status;
            changed = true;
        }
        match &v.status {
            Status::Failed(reason) => {
                let msg = format!("KERN cannot play this file: {reason}");
                self.content = Content::Failed(msg);
                cx.notify();
                return;
            }
            Status::Ready => v.player.seed_first_frame(),
            Status::Loading => {}
        }
        if let Some(frame) = v.player.poll_frame() {
            v.frame = Some(SurfaceSource::from(frame));
            v.frames_seen += 1;
            changed = true;
        }
        // Ready, yet nothing decodes: a codec the system cannot play, or audio only.
        if v.status == Status::Ready
            && v.frames_seen == 0
            && v.opened_at.elapsed() > Duration::from_secs(4)
        {
            let (w, h) = v.player.natural_size();
            let msg = if w == 0.0 || h == 0.0 {
                "This file has no video track that the system can display.".to_string()
            } else {
                "The system opened this file but could not decode a video frame. The codec is probably unsupported.".to_string()
            };
            self.content = Content::Failed(msg);
            cx.notify();
            return;
        }
        let (cur, dur) = (
            v.player.current_time().unwrap_or(0.0),
            v.player.duration().unwrap_or(0.0),
        );
        if (cur - v.current).abs() > 0.05 || (dur - v.duration).abs() > 0.01 {
            v.current = cur;
            v.duration = dur;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Called when the tab is no longer the visible one.
    pub fn on_hidden(&mut self) {
        #[cfg(target_os = "macos")]
        if let Content::Video(v) = &self.content {
            v.player.pause();
        }
    }

    #[cfg(feature = "autotest")]
    pub fn describe(&self) -> String {
        match &self.content {
            Content::Loading => "loading".into(),
            Content::Failed(e) => format!("failed: {e}"),
            Content::Image(i) => format!(
                "image {:?} {} failed={} note={:?}",
                i.size, i.format, i.decode_failed, i.note
            ),
            #[cfg(target_os = "macos")]
            Content::Video(v) => {
                let (w, h) = v.player.natural_size();
                format!(
                    "video {}x{} frames={} playing={} t={:.2}/{:.2} vol={:.2} muted={}",
                    w,
                    h,
                    v.frames_seen,
                    v.player.is_playing(),
                    v.current,
                    v.duration,
                    v.player.volume(),
                    v.player.muted()
                )
            }
        }
    }

    #[cfg(feature = "autotest")]
    pub fn test_action(&mut self, action: &str, cx: &mut Context<Self>) {
        match (&mut self.content, action) {
            #[cfg(target_os = "macos")]
            (Content::Video(v), "play") => v.player.play(),
            #[cfg(target_os = "macos")]
            (Content::Video(v), "pause") => v.player.pause(),
            #[cfg(target_os = "macos")]
            (Content::Video(v), "seekmid") => v.player.seek(v.duration / 2.0),
            #[cfg(target_os = "macos")]
            (Content::Video(v), "mute") => v.player.set_muted(true),
            #[cfg(target_os = "macos")]
            (Content::Video(v), "vol50") => v.player.set_volume(0.5),
            (Content::Image(i), "100") => i.zoom = Zoom::Factor(1.0),
            (Content::Image(i), "fit") => {
                i.zoom = Zoom::Fit;
                i.pan = Point::default();
            }
            (Content::Image(i), "in") => i.zoom = Zoom::Factor(4.0),
            (Content::Image(i), "pan") => i.pan = point(px(-40.), px(-30.)),
            _ => {}
        }
        cx.notify();
    }

    fn open_externally(&self) {
        if let Err(e) = open::that_detached(&self.path) {
            log::warn!("cannot open {}: {e}", self.path.display());
        }
    }

    fn render_failed(&self, msg: String, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_4()
            .bg(p.editor)
            .child(
                icon("triangle-alert")
                    .with_size(px(28.))
                    .text_color(p.warning),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_size(px(13.))
                    .text_center()
                    .child(msg),
            )
            .child(
                div().text_size(px(11.)).text_color(p.muted).child(
                    self.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                ),
            )
            .child(
                button("media-open", "Open With Default App", p)
                    .on_click(cx.listener(|this, _, _, _| this.open_externally())),
            )
            .into_any_element()
    }

    fn render_image(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let entity = cx.entity();
        let Content::Image(state) = &mut self.content else {
            return div().into_any_element();
        };
        let Some(source) = state.source.clone() else {
            return div().into_any_element();
        };
        let (iw, ih) = state.size.unwrap_or((512., 512.));
        let view = state.view;
        let (vw, vh) = (f32::from(view.width), f32::from(view.height));
        let fit = if vw > 0. && vh > 0. {
            (vw / iw).min(vh / ih).min(1.0)
        } else {
            1.0
        };
        let scale = match state.zoom {
            Zoom::Fit => fit,
            Zoom::Factor(f) => f,
        };
        let (dw, dh) = (iw * scale, ih * scale);
        let zoomed_in = dw > vw || dh > vh;
        // Panning is limited so that the picture cannot be lost off screen.
        let (max_x, max_y) = (((dw - vw) / 2.).max(0.), ((dh - vh) / 2.).max(0.));
        state.pan = point(
            px(f32::from(state.pan.x).clamp(-max_x, max_x)),
            px(f32::from(state.pan.y).clamp(-max_y, max_y)),
        );
        let left = (vw - dw) / 2. + f32::from(state.pan.x);
        let top = (vh - dh) / 2. + f32::from(state.pan.y);
        let info = format!(
            "{}×{} · {} · {}%",
            iw as u32,
            ih as u32,
            state.format,
            (scale * 100.).round() as u32
        );
        let note = state.note.clone();
        let alpha = state.alpha;
        let is_fit = state.zoom == Zoom::Fit;
        let failed = state.decode_failed;
        let zoom_btn =
            |id: &'static str, name: &'static str, cx: &mut Context<Self>, f: fn(f32) -> f32| {
                icon_button(id, name, name).on_click(cx.listener(move |this, _, _, cx| {
                    if let Content::Image(s) = &mut this.content {
                        let cur = match s.zoom {
                            Zoom::Fit => fit,
                            Zoom::Factor(v) => v,
                        };
                        s.zoom = Zoom::Factor(f(cur).clamp(0.02, 32.));
                    }
                    cx.notify();
                }))
            };
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .h(px(30.))
            .px_2()
            .bg(p.panel)
            .border_b_1()
            .border_color(p.border)
            .child(zoom_btn("zoom-out", "zoom-out", cx, |z| z / 1.25))
            .child(zoom_btn("zoom-in", "zoom-in", cx, |z| z * 1.25))
            .child(
                (if is_fit {
                    primary_button("zoom-fit", "Fit", p)
                } else {
                    button("zoom-fit", "Fit", p)
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Content::Image(s) = &mut this.content {
                        s.zoom = Zoom::Fit;
                        s.pan = Point::default();
                    }
                    cx.notify();
                })),
            )
            .child(
                button("zoom-100", "100%", p).on_click(cx.listener(|this, _, _, cx| {
                    if let Content::Image(s) = &mut this.content {
                        s.zoom = Zoom::Factor(1.0);
                        s.pan = Point::default();
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .pl_2()
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(info),
            )
            .when_some(note, |d, n| {
                d.child(
                    div()
                        .pl_2()
                        .text_size(px(11.))
                        .text_color(p.warning)
                        .child(n),
                )
            });
        let fail_entity = entity.clone();
        let picture = img(source)
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(dw))
            .h(px(dh))
            .object_fit(ObjectFit::Fill)
            .with_fallback(move || {
                let e = fail_entity.clone();
                div()
                    .size_full()
                    .child(
                        canvas(
                            move |_, _, cx| {
                                e.update(cx, |this: &mut MediaView, cx| {
                                    if let Content::Image(s) = &mut this.content
                                        && !s.decode_failed
                                    {
                                        s.decode_failed = true;
                                        cx.notify();
                                    }
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .into_any_element()
            })
            // Animation state (the GIF frame) lives in the element's state, which needs an id.
            .id("media-picture");
        if failed {
            return self.render_failed(
                "This image file is damaged or cannot be decoded.".to_string(),
                cx,
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.editor)
            .child(toolbar)
            .child(
                div()
                    .id("image-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .when(zoomed_in, |d| d.cursor_grab())
                    .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                        let Content::Image(s) = &mut this.content else {
                            return;
                        };
                        let delta = ev.delta.pixel_delta(px(20.));
                        if ev.modifiers.platform || ev.modifiers.control {
                            let cur = match s.zoom {
                                Zoom::Fit => fit,
                                Zoom::Factor(v) => v,
                            };
                            s.zoom = Zoom::Factor(
                                (cur * (1. + f32::from(delta.y) * 0.005)).clamp(0.02, 32.),
                            );
                        } else {
                            s.pan += delta;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, _| {
                            if let Content::Image(s) = &mut this.content {
                                s.last_drag = Some(ev.position);
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            if let Content::Image(s) = &mut this.content {
                                s.last_drag = None;
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if let Content::Image(s) = &mut this.content
                            && let (Some(last), true) = (s.last_drag, ev.dragging())
                        {
                            s.pan += ev.position - last;
                            s.last_drag = Some(ev.position);
                            cx.notify();
                        }
                    }))
                    .child(
                        canvas(
                            {
                                let entity = entity.clone();
                                move |bounds, _, cx| {
                                    entity.update(cx, |this: &mut MediaView, cx| {
                                        if let Content::Image(s) = &mut this.content
                                            && s.view != bounds.size
                                        {
                                            s.view = bounds.size;
                                            cx.notify();
                                        }
                                    });
                                    bounds
                                }
                            },
                            move |_, bounds, window, _| {
                                if alpha {
                                    paint_checker(bounds, left, top, dw, dh, p, window);
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(picture),
            )
            .into_any_element()
    }

    #[cfg(target_os = "macos")]
    fn seek_from(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Content::Video(v) = &mut self.content {
            let w = f32::from(v.bar.size.width);
            if w > 0. && v.duration > 0. {
                let f = (f32::from(x - v.bar.origin.x) / w).clamp(0., 1.) as f64;
                v.player.seek(f * v.duration);
                v.current = f * v.duration;
                cx.notify();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn render_video(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let entity = cx.entity();
        let Content::Video(v) = &mut self.content else {
            return div().into_any_element();
        };
        let playing = v.player.is_playing();
        let muted = v.player.muted();
        let volume = v.player.volume();
        let (cur, dur) = (v.current, v.duration);
        let frame = v.frame.clone();
        let progress = if dur > 0. {
            (cur / dur).clamp(0., 1.) as f32
        } else {
            0.
        };
        let vol_fill = if muted { 0. } else { volume };
        let e1 = entity.clone();
        let e2 = entity.clone();
        let track = |id: &'static str, fill: f32| {
            div()
                .id(id)
                .relative()
                .h(px(16.))
                .flex()
                .items_center()
                .cursor_pointer()
                .child(div().h(px(4.)).w_full().rounded(px(2.)).bg(p.active))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .h(px(4.))
                        .w(relative(fill))
                        .rounded(px(2.))
                        .bg(p.accent_fill),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui_kit::black())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if let Content::Video(v) = &mut this.content {
                        v.scrubbing = false;
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let scrubbing = matches!(&this.content, Content::Video(v) if v.scrubbing);
                if scrubbing && ev.dragging() {
                    this.seek_from(ev.position.x, cx);
                }
            }))
            .child(
                div()
                    .id("video-area")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Content::Video(v) = &this.content {
                            if v.player.is_playing() {
                                v.player.pause()
                            } else {
                                v.player.play()
                            }
                        }
                        cx.notify();
                    }))
                    .when_some(frame, |d, f| {
                        d.child(surface(f).object_fit(ObjectFit::Contain).size_full())
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .h(px(36.))
                    .px_3()
                    .bg(p.panel)
                    .border_t_1()
                    .border_color(p.border)
                    .child(
                        icon_button(
                            "vid-play",
                            if playing { "pause" } else { "play" },
                            if playing { "Pause" } else { "Play" },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Content::Video(v) = &this.content {
                                if v.player.is_playing() {
                                    v.player.pause()
                                } else {
                                    v.player.play()
                                }
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .font_family("Menlo")
                            .text_color(p.fg)
                            .child(format!("{} / {}", clock(cur), clock(dur))),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w(px(60.))
                            .child(track("vid-seek", progress).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    if let Content::Video(v) = &mut this.content {
                                        v.scrubbing = true;
                                    }
                                    this.seek_from(ev.position.x, cx);
                                }),
                            ))
                            .child(
                                canvas(
                                    move |b, _, cx| {
                                        e1.update(cx, |this: &mut MediaView, _| {
                                            if let Content::Video(v) = &mut this.content {
                                                v.bar = b;
                                            }
                                        });
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
                    .child(
                        icon_button(
                            "vid-mute",
                            if muted { "volume-x" } else { "volume-2" },
                            if muted { "Unmute" } else { "Mute" },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Content::Video(v) = &this.content {
                                v.player.set_muted(!v.player.muted());
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(80.))
                            .flex_shrink_0()
                            .child(track("vid-vol", vol_fill).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                    if let Content::Video(v) = &this.content {
                                        let w = f32::from(v.volume_bar.size.width);
                                        if w > 0. {
                                            v.player.set_muted(false);
                                            v.player.set_volume(
                                                f32::from(ev.position.x - v.volume_bar.origin.x)
                                                    / w,
                                            );
                                        }
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(
                                canvas(
                                    move |b, _, cx| {
                                        e2.update(cx, |this: &mut MediaView, _| {
                                            if let Content::Video(v) = &mut this.content {
                                                v.volume_bar = b;
                                            }
                                        });
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(target_os = "macos")]
fn clock(sec: f64) -> String {
    let s = sec.max(0.) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// A checkerboard under the visible part of the picture, showing transparency.
fn paint_checker(
    bounds: Bounds<Pixels>,
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    p: Palette,
    window: &mut Window,
) {
    const CELL: f32 = 10.;
    let (a, b): (Hsla, Hsla) = if p.dark {
        (rgb(0x2a2a30).into(), rgb(0x38383f).into())
    } else {
        (rgb(0xe9e9ee).into(), rgb(0xffffff).into())
    };
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (x0, y0) = (left.max(0.), top.max(0.));
    let (x1, y1) = ((left + w).min(bw), (top + h).min(bh));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    window.paint_quad(fill(
        Bounds::new(
            point(px(ox + x0), px(oy + y0)),
            size(px(x1 - x0), px(y1 - y0)),
        ),
        a,
    ));
    let mut y = y0;
    let mut row = 0;
    while y < y1 {
        let mut x = x0;
        let mut col = 0;
        while x < x1 {
            if (row + col) % 2 == 1 {
                let cw = CELL.min(x1 - x);
                let ch = CELL.min(y1 - y);
                window.paint_quad(fill(
                    Bounds::new(point(px(ox + x), px(oy + y)), size(px(cw), px(ch))),
                    b,
                ));
            }
            x += CELL;
            col += 1;
        }
        y += CELL;
        row += 1;
    }
}

/// A complete JPEG ends with the EOI marker, possibly followed by padding.
fn jpeg_is_complete(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let tail = len.min(4096);
    let mut buf = vec![0; tail as usize];
    if f.seek(SeekFrom::Start(len - tail)).is_err() || f.read_exact(&mut buf).is_err() {
        return false;
    }
    buf.windows(2).any(|w| w == [0xFF, 0xD9])
}

struct Probed {
    size: (f32, f32),
    format: String,
    alpha: bool,
    note: Option<String>,
    svg_bytes: Option<Vec<u8>>,
}

/// Reads the header only. Pixels are decoded later by the image loader, in the background.
fn probe_image(path: &Path, kind: MediaKind) -> Result<Probed, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Cannot read this file: {e}"))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "This image is too large to preview ({} MB).",
            meta.len() / 1024 / 1024
        ));
    }
    if kind == MediaKind::Svg {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("Cannot read this SVG: {e}"))?;
        if !text.contains("<svg") {
            return Err("This file is not a valid SVG.".into());
        }
        let (clean, blocked) = sanitize_svg(&text);
        let size = svg_size(&clean).unwrap_or((512., 512.));
        return Ok(Probed {
            size,
            format: "SVG".into(),
            alpha: true,
            note: (blocked > 0).then(|| format!("{blocked} external reference(s) not loaded")),
            svg_bytes: Some(clean.into_bytes()),
        });
    }
    let reader = image::ImageReader::open(path)
        .map_err(|e| format!("Cannot read this file: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("Cannot read this file: {e}"))?;
    let format = reader
        .format()
        .ok_or("This file is not a recognized image.")?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| "This image file is damaged or cannot be decoded.".to_string())?;
    if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
        return Err(format!("This image is too large to preview ({w}×{h})."));
    }
    // The header can be fine while the data is cut short. Decoding here, off the UI
    // thread, turns that into a clear message instead of an empty picture. JPEG is
    // excepted: its decoder shows the part it has, which is checked separately below.
    if format != image::ImageFormat::Jpeg
        && meta.len() <= MAX_VALIDATED_BYTES
        && image::ImageReader::open(path)
            .and_then(|r| r.with_guessed_format())
            .map_err(|_| ())
            .and_then(|r| r.decode().map_err(|_| ()))
            .is_err()
    {
        return Err("This image file is damaged or cannot be decoded.".to_string());
    }
    let name = format!("{format:?}").to_uppercase();
    // The JPEG decoder shows what it has and fills the rest with gray, silently.
    let note = (format == image::ImageFormat::Jpeg && !jpeg_is_complete(path))
        .then(|| "This file looks truncated: part of the picture may be missing".to_string());
    Ok(Probed {
        size: (w as f32, h as f32),
        alpha: !matches!(format, image::ImageFormat::Jpeg),
        format: name,
        note,
        svg_bytes: None,
    })
}

impl Render for MediaView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        match &self.content {
            Content::Loading => div().size_full().bg(p.editor).into_any_element(),
            Content::Failed(msg) => {
                let msg = msg.clone();
                self.render_failed(msg, cx)
            }
            Content::Image(_) => self.render_image(cx),
            #[cfg(target_os = "macos")]
            Content::Video(_) => self.render_video(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn kinds() {
        for (f, k) in [
            ("a.PNG", MediaKind::Raster),
            ("a.jpeg", MediaKind::Raster),
            ("a.gif", MediaKind::Raster),
            ("a.webp", MediaKind::Raster),
            ("a.svg", MediaKind::Svg),
            ("a.MP4", MediaKind::Video),
            ("a.mov", MediaKind::Video),
        ] {
            assert_eq!(kind_for(Path::new(f)), Some(k), "{f}");
        }
        assert_eq!(kind_for(Path::new("a.rs")), None);
    }

    #[::core::prelude::v1::test]
    fn svg_external_references_are_blanked() {
        let svg = r##"<svg><image href="http://evil/x.png"/><image xlink:href='file:///etc/passwd'/><use href="#local"/><image href="data:image/png;base64,AA"/></svg>"##;
        let (clean, blocked) = sanitize_svg(svg);
        assert_eq!(blocked, 2);
        assert!(!clean.contains("evil") && !clean.contains("passwd"));
        assert!(clean.contains("#local") && clean.contains("data:image"));
    }

    #[::core::prelude::v1::test]
    fn svg_sizes() {
        assert_eq!(
            svg_size(r#"<svg width="24" height="48"></svg>"#),
            Some((24., 48.))
        );
        assert_eq!(
            svg_size(r#"<svg viewBox="0 0 100 50"></svg>"#),
            Some((100., 50.))
        );
    }

    #[::core::prelude::v1::test]
    fn truncated_png_is_an_error_and_a_whole_one_is_not() {
        let d = tempfile::tempdir().unwrap();
        let whole = d.path().join("whole.png");
        image::RgbaImage::from_fn(300, 300, |x, y| {
            image::Rgba([x as u8, y as u8, (x * y) as u8, 200])
        })
        .save(&whole)
        .unwrap();
        let Ok(p) = probe_image(&whole, MediaKind::Raster) else {
            panic!("whole file")
        };
        assert_eq!(p.size, (300., 300.));
        let bytes = std::fs::read(&whole).unwrap();
        let cut = d.path().join("cut.png");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        let Err(err) = probe_image(&cut, MediaKind::Raster) else {
            panic!("cut file")
        };
        assert!(err.contains("damaged"), "{err}");
    }

    #[::core::prelude::v1::test]
    fn truncated_jpeg_is_noticed() {
        let d = tempfile::tempdir().unwrap();
        let ok = d.path().join("ok.jpg");
        std::fs::write(&ok, [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9]).unwrap();
        assert!(jpeg_is_complete(&ok));
        let cut = d.path().join("cut.jpg");
        std::fs::write(&cut, [0xFF, 0xD8, 1, 2, 3]).unwrap();
        assert!(!jpeg_is_complete(&cut));
    }

    #[::core::prelude::v1::test]
    fn damaged_images_are_errors_not_panics() {
        let d = tempfile::tempdir().unwrap();
        let bad = d.path().join("bad.png");
        std::fs::write(&bad, b"not an image at all").unwrap();
        assert!(probe_image(&bad, MediaKind::Raster).is_err());
        let empty = d.path().join("empty.jpg");
        std::fs::write(&empty, b"").unwrap();
        assert!(probe_image(&empty, MediaKind::Raster).is_err());
        assert!(probe_image(&d.path().join("none.png"), MediaKind::Raster).is_err());
    }
}
