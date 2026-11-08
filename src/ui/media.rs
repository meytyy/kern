//! Read-only image and video tabs. Decoding and probing happen off the UI thread.
use super::assets::icon;
use super::theme::{Palette, pal};
use super::widgets::*;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(any(target_os = "macos", windows))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", windows))]
use crate::platform::video::{Status, VideoPlayer};

/// Pictures zoom between 2% and 3200%.
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.0;
/// The arrow keys in the player skip this far.
#[cfg(any(target_os = "macos", windows))]
const SKIP_SECONDS: f64 = 5.0;
/// In full screen the controls go away after this long without the mouse moving.
#[cfg(any(target_os = "macos", windows))]
const CONTROLS_LINGER: Duration = Duration::from_millis(2500);

/// Above this, a picture is not decoded: it could take gigabytes of memory.
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 268_000_000;
const MAX_VALIDATED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Raster,
    Svg,
    Video,
    /// .zip, .tar, .tar.gz, .tgz: the list of files inside, read only.
    Archive,
    /// Anything else that is not text: what it is and how big.
    Binary,
}

pub fn kind_for(path: &Path) -> Option<MediaKind> {
    if crate::workspace::archive::archive_kind(path).is_some() {
        return Some(MediaKind::Archive);
    }
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
    /// Offset of the picture's centre from the centre of the view.
    pan: Point<Pixels>,
    last_drag: Option<Point<Pixels>>,
    /// The picture area, in window coordinates.
    view: Size<Pixels>,
    origin: Point<Pixels>,
    decode_failed: bool,
}

impl ImageState {
    fn image_size(&self) -> (f32, f32) {
        self.size.unwrap_or((512., 512.))
    }

    /// The scale that shows the whole picture, never above 100%.
    fn fit(&self) -> f32 {
        let (iw, ih) = self.image_size();
        let (vw, vh) = (f32::from(self.view.width), f32::from(self.view.height));
        if vw > 0. && vh > 0. {
            (vw / iw).min(vh / ih).min(1.0)
        } else {
            1.0
        }
    }

    fn scale(&self) -> f32 {
        match self.zoom {
            Zoom::Fit => self.fit(),
            Zoom::Factor(f) => f,
        }
    }

    /// Zoom to `scale`, keeping the point of the picture under `at` (window
    /// coordinates; the centre of the view if `None`) where it is.
    fn zoom_at(&mut self, scale: f32, at: Option<Point<Pixels>>) {
        let (iw, ih) = self.image_size();
        let (vw, vh) = (f32::from(self.view.width), f32::from(self.view.height));
        let old = self.scale();
        let new = scale.clamp(MIN_ZOOM, MAX_ZOOM);
        let (ax, ay) = match at {
            Some(p) => (
                f32::from(p.x - self.origin.x),
                f32::from(p.y - self.origin.y),
            ),
            None => (vw / 2., vh / 2.),
        };
        let left = (vw - iw * old) / 2. + f32::from(self.pan.x);
        let top = (vh - ih * old) / 2. + f32::from(self.pan.y);
        let (ix, iy) = ((ax - left) / old, (ay - top) / old);
        self.pan = point(
            px(ax - ix * new - (vw - iw * new) / 2.),
            px(ay - iy * new - (vh - ih * new) / 2.),
        );
        self.zoom = Zoom::Factor(new);
        self.clamp_pan();
    }

    /// A picture smaller than the view stays centred; a bigger one cannot be
    /// moved so far that its edge comes inside the view.
    fn clamp_pan(&mut self) {
        let (iw, ih) = self.image_size();
        let s = self.scale();
        let (vw, vh) = (f32::from(self.view.width), f32::from(self.view.height));
        let max_x = ((iw * s - vw) / 2.).max(0.);
        let max_y = ((ih * s - vh) / 2.).max(0.);
        self.pan = point(
            px(f32::from(self.pan.x).clamp(-max_x, max_x)),
            px(f32::from(self.pan.y).clamp(-max_y, max_y)),
        );
    }
}

/// A decoded picture as the platform hands it over: an IOSurface-backed pixel
/// buffer drawn directly on macOS, BGRA pixels uploaded as an image on Windows.
#[cfg(target_os = "macos")]
type VideoFrame = SurfaceSource;
#[cfg(windows)]
type VideoFrame = Arc<RenderImage>;

#[cfg(any(target_os = "macos", windows))]
struct VideoState {
    player: VideoPlayer,
    frame: Option<VideoFrame>,
    /// Frames already replaced; their GPU copies are freed at the next render.
    #[cfg(windows)]
    retired: Vec<Arc<RenderImage>>,
    status: Status,
    opened_at: Instant,
    frames_seen: u64,
    scrubbing: bool,
    volume_dragging: bool,
    bar: Bounds<Pixels>,
    volume_bar: Bounds<Pixels>,
    current: f64,
    duration: f64,
    /// Shown over the whole window; the app makes the window full screen.
    fullscreen: bool,
    last_motion: Instant,
    controls_shown: bool,
    _ticker: Task<()>,
}

#[cfg(any(target_os = "macos", windows))]
impl VideoState {
    /// Outside full screen the controls stay. In it they show while the mouse
    /// moves, while paused and while a slider is dragged.
    fn controls_wanted(&self) -> bool {
        !self.fullscreen
            || !self.player.is_playing()
            || self.scrubbing
            || self.volume_dragging
            || self.last_motion.elapsed() < CONTROLS_LINGER
    }

    fn toggle_play(&self) {
        if self.player.is_playing() {
            self.player.pause()
        } else {
            self.player.play()
        }
    }

    fn skip(&mut self, by: f64) {
        let mut to = (self.current + by).max(0.);
        if self.duration > 0. {
            to = to.min(self.duration);
        }
        self.player.seek(to);
        self.current = to;
    }
}

enum Content {
    Loading,
    Image(ImageState),
    #[cfg(any(target_os = "macos", windows))]
    Video(VideoState),
    Failed(String),
    Archive {
        kind: crate::workspace::archive::ArchiveKind,
        listing: Result<crate::workspace::archive::Listing, String>,
    },
    Binary(String),
}

pub struct MediaView {
    pub path: PathBuf,
    pub kind: MediaKind,
    content: Content,
    /// The player's keys (Space, arrows, F, Esc) work while this is focused.
    focus: FocusHandle,
}

impl MediaView {
    pub fn new(path: PathBuf, kind: MediaKind, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            path,
            kind,
            content: Content::Loading,
            focus: cx.focus_handle(),
        };
        match kind {
            MediaKind::Raster | MediaKind::Svg => view.load_image(cx),
            MediaKind::Video => view.load_video(cx),
            MediaKind::Archive | MediaKind::Binary => view.load_listing(cx),
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
                        origin: Point::default(),
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

    #[cfg(not(any(target_os = "macos", windows)))]
    fn load_video(&mut self, _cx: &mut Context<Self>) {
        self.content = Content::Failed(
            "This build plays video inside KERN only on macOS and Windows. Open it in the system player."
                .into(),
        );
    }

    #[cfg(any(target_os = "macos", windows))]
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
                    #[cfg(windows)]
                    retired: Vec::new(),
                    status: Status::Loading,
                    opened_at: Instant::now(),
                    frames_seen: 0,
                    scrubbing: false,
                    volume_dragging: false,
                    bar: Bounds::default(),
                    volume_bar: Bounds::default(),
                    current: 0.0,
                    duration: 0.0,
                    fullscreen: false,
                    last_motion: Instant::now(),
                    controls_shown: true,
                    _ticker: ticker,
                });
            }
            Err(e) => self.content = Content::Failed(format!("KERN cannot play this file: {e}")),
        }
    }

    #[cfg(any(target_os = "macos", windows))]
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
            #[cfg(target_os = "macos")]
            {
                v.frame = Some(SurfaceSource::from(frame));
            }
            #[cfg(windows)]
            if let Some(pixels) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
            {
                // The buffer holds BGRA, which is what a RenderImage stores.
                let picture = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                    pixels
                )]));
                if let Some(old) = v.frame.replace(picture) {
                    v.retired.push(old);
                }
            }
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
        if v.controls_wanted() != v.controls_shown {
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Called when the tab is no longer the visible one.
    pub fn on_hidden(&mut self) {
        #[cfg(any(target_os = "macos", windows))]
        if let Content::Video(v) = &mut self.content {
            v.player.pause();
            v.fullscreen = false;
        }
    }

    /// The player covers the whole window.
    pub fn is_fullscreen(&self) -> bool {
        #[cfg(any(target_os = "macos", windows))]
        if let Content::Video(v) = &self.content {
            return v.fullscreen;
        }
        false
    }

    /// Full screen on or off. Playback, position and volume stay as they are:
    /// it is the same player, drawn somewhere else.
    pub fn set_fullscreen(&mut self, on: bool, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "macos", windows))]
        if let Content::Video(v) = &mut self.content
            && v.fullscreen != on
        {
            v.fullscreen = on;
            v.last_motion = Instant::now();
            cx.notify();
        }
        let _ = (on, cx);
    }

    #[cfg(feature = "autotest")]
    pub fn describe(&self) -> String {
        match &self.content {
            Content::Loading => "loading".into(),
            Content::Failed(e) => format!("failed: {e}"),
            Content::Archive { .. } | Content::Binary(_) => self.summary(),
            Content::Image(i) => format!(
                "image {:?} {} failed={} note={:?} zoom={:.0}% fit={} pan={:.0},{:.0} area={}",
                i.size,
                i.format,
                i.decode_failed,
                i.note,
                i.scale() * 100.,
                i.zoom == Zoom::Fit,
                f32::from(i.pan.x) + 0.,
                f32::from(i.pan.y) + 0.,
                rect(Bounds::new(i.origin, i.view))
            ),
            #[cfg(any(target_os = "macos", windows))]
            Content::Video(v) => {
                let (w, h) = v.player.natural_size();
                format!(
                    "video {}x{} frames={} playing={} t={:.2}/{:.2} vol={:.2} muted={} fullscreen={} controls={} bar={} vbar={}",
                    w,
                    h,
                    v.frames_seen,
                    v.player.is_playing(),
                    v.current,
                    v.duration,
                    v.player.volume(),
                    v.player.muted(),
                    v.fullscreen,
                    v.controls_shown,
                    rect(v.bar),
                    rect(v.volume_bar)
                )
            }
        }
    }

    #[cfg(feature = "autotest")]
    pub fn test_action(&mut self, action: &str, cx: &mut Context<Self>) {
        match (&mut self.content, action) {
            #[cfg(any(target_os = "macos", windows))]
            (Content::Video(v), "play") => v.player.play(),
            #[cfg(any(target_os = "macos", windows))]
            (Content::Video(v), "pause") => v.player.pause(),
            #[cfg(any(target_os = "macos", windows))]
            (Content::Video(v), "seekmid") => v.player.seek(v.duration / 2.0),
            #[cfg(any(target_os = "macos", windows))]
            (Content::Video(v), "mute") => v.player.set_muted(true),
            #[cfg(any(target_os = "macos", windows))]
            (Content::Video(v), "vol50") => v.player.set_volume(0.5),
            (Content::Image(i), "100") => i.zoom_at(1.0, None),
            (Content::Image(i), "fit") => {
                i.zoom = Zoom::Fit;
                i.pan = Point::default();
            }
            (Content::Image(i), "in") => i.zoom_at(4.0, None),
            (Content::Image(i), "pan") => {
                i.pan = point(px(-40.), px(-30.));
                i.clamp_pan();
            }
            _ => {}
        }
        cx.notify();
    }

    fn open_externally(&self) {
        if let Err(e) = open::that_detached(&self.path) {
            log::warn!("cannot open {}: {e}", self.path.display());
        }
    }

    /// The archive's table of contents (or the binary file's type), off the UI thread.
    fn load_listing(&mut self, cx: &mut Context<Self>) {
        use crate::workspace::archive;
        let path = self.path.clone();
        let kind = self.kind;
        cx.spawn(async move |this, cx| {
            // Plain data crosses threads; the view's content is built on the UI thread.
            let result = cx
                .background_spawn(async move {
                    match (kind, archive::archive_kind(&path)) {
                        (MediaKind::Archive, Some(k)) => Ok((k, archive::list(&path, k))),
                        _ => Err(archive::describe_binary(&path)),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.content = match result {
                    Ok((kind, listing)) => Content::Archive { kind, listing },
                    Err(desc) => Content::Binary(desc),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What the tab shows, in words (tests and accessibility).
    pub fn summary(&self) -> String {
        match &self.content {
            Content::Loading => "loading".into(),
            Content::Failed(m) => format!("failed: {m}"),
            Content::Image(_) => "image".into(),
            #[cfg(any(target_os = "macos", windows))]
            Content::Video(_) => "video".into(),
            Content::Binary(desc) => format!("binary: {desc}"),
            Content::Archive { kind, listing } => match listing {
                Ok(l) => format!(
                    "archive: {} · {} entries · {}",
                    kind.label(),
                    l.entries.len(),
                    l.entries
                        .iter()
                        .map(|e| e.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Err(e) => format!("archive error: {e}"),
            },
        }
    }

    fn file_size(&self) -> String {
        std::fs::metadata(&self.path)
            .map(|m| crate::workspace::archive::human_size(m.len()))
            .unwrap_or_default()
    }

    /// Archive or binary file: what it is, how big, and the system's ways to open it.
    fn render_contents(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::workspace::archive::human_size;
        let p = pal(cx);
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (kind_label, listing) = match &self.content {
            Content::Archive { kind, listing } => (kind.label().to_string(), Some(listing.clone())),
            Content::Binary(desc) => (desc.clone(), None),
            _ => (String::new(), None),
        };
        let reveal = if cfg!(target_os = "macos") {
            "Reveal in Finder"
        } else {
            "Show in File Explorer"
        };
        let path = self.path.clone();
        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .pb_3()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        icon(if listing.is_some() {
                            "ft-archive"
                        } else {
                            "ft-file"
                        })
                        .with_size(px(18.))
                        .text_color(p.muted),
                    )
                    .child(
                        div()
                            .id("bin-name")
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(format!("{kind_label} · {} · read only", self.file_size())),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .pt_1()
                    .child(
                        button("bin-open", "Open With Default App", p)
                            .on_click(cx.listener(|this, _, _, _| this.open_externally())),
                    )
                    .child(button("bin-reveal", reveal, p).on_click(move |_, _, _| {
                        crate::platform::reveal(&path).ok();
                    })),
            );
        let body: AnyElement = match listing {
            None => div()
                .pt_3()
                .text_size(px(12.))
                .text_color(p.muted)
                .child("KERN does not show this kind of file. Open it with the app made for it.")
                .into_any_element(),
            Some(Err(e)) => div()
                .pt_3()
                .text_size(px(12.))
                .text_color(p.danger)
                .child(format!("Cannot read the archive: {e}"))
                .into_any_element(),
            Some(Ok(l)) => {
                let files = l.entries.iter().filter(|e| !e.dir).count();
                let summary = format!(
                    "{files} files{} · {} unpacked{}",
                    if l.entries.len() > files {
                        format!(", {} folders", l.entries.len() - files)
                    } else {
                        String::new()
                    },
                    human_size(l.unpacked),
                    if l.truncated {
                        format!(" · first {} entries shown", l.entries.len())
                    } else {
                        String::new()
                    }
                );
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .py_2()
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .child(summary),
                    )
                    .child(
                        div()
                            .id("archive-entries")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(l.entries.into_iter().enumerate().map(|(i, e)| {
                                div()
                                    .id(("entry", i))
                                    .flex()
                                    .gap_3()
                                    .h(px(ROW_H))
                                    .items_center()
                                    .text_size(px(12.))
                                    .font_family(crate::core::settings::MONO_FONT)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_color(if e.dir { p.muted } else { p.fg })
                                            .child(e.path),
                                    )
                                    .child(div().flex_shrink_0().text_color(p.muted).child(
                                        if e.dir {
                                            String::new()
                                        } else {
                                            human_size(e.size)
                                        },
                                    ))
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .p_4()
            .bg(p.editor)
            .child(header)
            .child(body)
            .into_any_element()
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
                button(
                    "media-open",
                    if self.kind == MediaKind::Video {
                        "Open in External Player"
                    } else {
                        "Open With Default App"
                    },
                    p,
                )
                .on_click(cx.listener(|this, _, _, _| this.open_externally())),
            )
            .into_any_element()
    }

    fn render_image(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let entity = cx.entity();
        if matches!(&self.content, Content::Image(s) if s.decode_failed) {
            return self.render_failed(
                "This image file is damaged or cannot be decoded.".to_string(),
                cx,
            );
        }
        let Content::Image(state) = &mut self.content else {
            return div().into_any_element();
        };
        let Some(source) = state.source.clone() else {
            return div().into_any_element();
        };
        state.clamp_pan();
        let (iw, ih) = state.image_size();
        let (vw, vh) = (f32::from(state.view.width), f32::from(state.view.height));
        let scale = state.scale();
        let (dw, dh) = (iw * scale, ih * scale);
        let movable = dw > vw + 0.5 || dh > vh + 0.5;
        let dragging = state.last_drag.is_some();
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
        let is_100 = state.zoom == Zoom::Factor(1.0);
        let step = |id: &'static str, label: &'static str, hint: &'static str, by: f32| {
            button(id, label, p)
                .tooltip(hint)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Content::Image(s) = &mut this.content {
                        let to = s.scale() * by;
                        s.zoom_at(to, None);
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
            .child(step("zoom-out", "−", "Zoom Out", 1. / 1.25))
            .child(step("zoom-in", "+", "Zoom In", 1.25))
            .child(
                (if is_100 {
                    primary_button("zoom-100", "100%", p)
                } else {
                    button("zoom-100", "100%", p)
                })
                .tooltip("Actual Size")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Content::Image(s) = &mut this.content {
                        s.zoom_at(1.0, None);
                    }
                    cx.notify();
                })),
            )
            .child(
                (if is_fit {
                    primary_button("zoom-fit", "Fit", p)
                } else {
                    button("zoom-fit", "Fit", p)
                })
                .tooltip("Fit to the View")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Content::Image(s) = &mut this.content {
                        s.zoom = Zoom::Fit;
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
                    .when(movable && !dragging, |d| d.cursor_grab())
                    .when(dragging, |d| d.cursor_grabbing())
                    // Wheel: up and down. Alt (Option) + wheel: sideways. Ctrl (or
                    // Cmd) + wheel: zoom around the pointer. A trackpad's two-finger
                    // moves come as both axes at once.
                    .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                        let Content::Image(s) = &mut this.content else {
                            return;
                        };
                        let d = ev.delta.pixel_delta(px(40.));
                        if ev.modifiers.control || ev.modifiers.platform {
                            // One notch of a mouse wheel is about 10%.
                            let by = (f32::from(d.y) * 0.0025).exp();
                            let to = s.scale() * by;
                            s.zoom_at(to, Some(ev.position));
                        } else if ev.modifiers.alt {
                            s.pan.x += if d.x == px(0.) { d.y } else { d.x };
                        } else {
                            s.pan += d;
                        }
                        s.clamp_pan();
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_pinch(cx.listener(|this, ev: &PinchEvent, _, cx| {
                        if let Content::Image(s) = &mut this.content {
                            let to = s.scale() * (1. + ev.delta);
                            s.zoom_at(to, Some(ev.position));
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            if let Content::Image(s) = &mut this.content {
                                s.last_drag = Some(ev.position);
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if let Content::Image(s) = &mut this.content {
                            drag_to(s, ev);
                            cx.notify();
                        }
                    }))
                    .child(
                        canvas(
                            {
                                let entity = entity.clone();
                                move |bounds, _, cx| {
                                    entity.update(cx, |this: &mut MediaView, cx| {
                                        if let Content::Image(s) = &mut this.content {
                                            s.origin = bounds.origin;
                                            if s.view != bounds.size {
                                                s.view = bounds.size;
                                                cx.notify();
                                            }
                                        }
                                    });
                                    bounds
                                }
                            },
                            {
                                let entity = entity.clone();
                                move |_, bounds, window, _| {
                                    if alpha {
                                        paint_checker(bounds, left, top, dw, dh, p, window);
                                    }
                                    if dragging {
                                        follow_drag(&entity, bounds, window);
                                    }
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

    #[cfg(any(target_os = "macos", windows))]
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

    #[cfg(any(target_os = "macos", windows))]
    fn volume_from(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Content::Video(v) = &mut self.content {
            let w = f32::from(v.volume_bar.size.width);
            if w > 0. {
                v.player.set_muted(false);
                v.player
                    .set_volume((f32::from(x - v.volume_bar.origin.x) / w).clamp(0., 1.));
                cx.notify();
            }
        }
    }

    /// Space, the arrows, F and Esc while the player has the focus. Anything
    /// else (and these keys anywhere else) keeps its usual meaning.
    #[cfg(any(target_os = "macos", windows))]
    fn player_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        // Fn is not checked: macOS reports it for the arrow keys and for
        // keys of some keyboards.
        let m = &ev.keystroke.modifiers;
        if m.control || m.platform || m.alt {
            return;
        }
        let Content::Video(v) = &mut self.content else {
            return;
        };
        match ev.keystroke.key.as_str() {
            "space" => v.toggle_play(),
            "left" => v.skip(-SKIP_SECONDS),
            "right" => v.skip(SKIP_SECONDS),
            "f" => v.fullscreen = !v.fullscreen,
            "escape" if v.fullscreen => v.fullscreen = false,
            _ => return,
        }
        v.last_motion = Instant::now();
        cx.stop_propagation();
        cx.notify();
    }

    #[cfg(any(target_os = "macos", windows))]
    fn render_video(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let focus = self.focus.clone();
        let Content::Video(v) = &mut self.content else {
            return div().into_any_element();
        };
        let playing = v.player.is_playing();
        let muted = v.player.muted();
        let volume = v.player.volume();
        let (cur, dur) = (v.current, v.duration);
        let fullscreen = v.fullscreen;
        let sliding = v.scrubbing || v.volume_dragging;
        let show_controls = v.controls_wanted();
        v.controls_shown = show_controls;
        let progress = if dur > 0. {
            (cur / dur).clamp(0., 1.) as f32
        } else {
            0.
        };
        let vol_fill = if muted { 0. } else { volume.clamp(0., 1.) };
        let picture = v.frame.clone().map(|f| {
            #[cfg(target_os = "macos")]
            let el = surface(f).object_fit(ObjectFit::Contain).size_full();
            #[cfg(windows)]
            let el = img(ImageSource::Render(f))
                .object_fit(ObjectFit::Contain)
                .size_full();
            el.into_any_element()
        });

        // The player looks the same in every theme: light controls on black.
        let white = gpui_kit::white();
        let accent = pal(cx).accent_fill;
        let player_button = |id: &'static str, name: &'static str, hint: String| {
            div()
                .id(id)
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(white.opacity(0.14)))
                .child(icon(name).with_size(px(18.)).text_color(white))
                .tooltip(super::widgets::tip(hint))
        };
        let slider = |id: &'static str, fill: f32, height: f32| {
            div()
                .id(id)
                .relative()
                .h(px(14.))
                .w_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .h(px(height))
                        .w_full()
                        .rounded(px(2.))
                        .bg(white.opacity(0.3)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .h(px(height))
                        .w(relative(fill))
                        .rounded(px(2.))
                        .bg(accent),
                )
                .child(
                    div()
                        .absolute()
                        .left(relative(fill))
                        .ml(px(-6.))
                        .size(px(12.))
                        .rounded_full()
                        .bg(white),
                )
        };
        let bounds_probe = |which: u8| {
            let e = entity.clone();
            canvas(
                move |b, _, cx| {
                    e.update(cx, |this: &mut MediaView, _| {
                        if let Content::Video(v) = &mut this.content {
                            if which == 0 {
                                v.bar = b;
                            } else {
                                v.volume_bar = b;
                            }
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let seek_bar = div().w_full().px_3().pt_1().child(
            slider("vid-seek", progress, 4.)
                .child(bounds_probe(0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        if let Content::Video(v) = &mut this.content {
                            v.scrubbing = true;
                        }
                        this.seek_from(ev.position.x, cx);
                        cx.stop_propagation();
                    }),
                ),
        );
        let row = div()
            .flex()
            .items_center()
            .gap_1()
            .h(px(38.))
            .px_2()
            .child(
                player_button(
                    "vid-play",
                    if playing { "pause" } else { "play" },
                    format!("{} (Space)", if playing { "Pause" } else { "Play" }),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Content::Video(v) = &this.content {
                        v.toggle_play();
                    }
                    cx.notify();
                })),
            )
            .child(
                player_button(
                    "vid-mute",
                    if muted || volume == 0. {
                        "volume-x"
                    } else if volume < 0.5 {
                        "volume-1"
                    } else {
                        "volume-2"
                    },
                    if muted { "Unmute" } else { "Mute" }.to_string(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Content::Video(v) = &this.content {
                        v.player.set_muted(!v.player.muted());
                    }
                    cx.notify();
                })),
            )
            .child(
                div().w(px(88.)).px(px(6.)).flex_shrink_0().child(
                    slider("vid-vol", vol_fill, 3.)
                        .child(bounds_probe(1))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                if let Content::Video(v) = &mut this.content {
                                    v.volume_dragging = true;
                                }
                                this.volume_from(ev.position.x, cx);
                                cx.stop_propagation();
                            }),
                        ),
                ),
            )
            .child(
                div()
                    .ml_2()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .font_family(crate::core::settings::MONO_FONT)
                    .text_color(white)
                    .child(format!("{} / {}", clock(cur), clock(dur))),
            )
            .child(div().flex_1())
            .child(
                player_button(
                    "vid-fullscreen",
                    if fullscreen {
                        "minimize-2"
                    } else {
                        "maximize-2"
                    },
                    if fullscreen {
                        "Exit Full Screen (Esc)".to_string()
                    } else {
                        "Full Screen (F)".to_string()
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let on = !this.is_fullscreen();
                    this.set_fullscreen(on, cx);
                    window.focus(&this.focus, cx);
                })),
            );
        let controls = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w_full()
            .bg(if fullscreen {
                gpui_kit::black().opacity(0.6)
            } else {
                hsla(0., 0., 0.09, 1.)
            })
            .child(seek_bar)
            .child(row);
        let area = div()
            .id("video-area")
            .flex()
            .items_center()
            .justify_center()
            .when(fullscreen, |d| d.absolute().inset_0())
            .when(!fullscreen, |d| d.flex_1().min_h_0().w_full())
            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                if let Content::Video(v) = &mut this.content {
                    // A double click goes full screen, as in web players; its
                    // first click already toggled playback, the second undoes it.
                    v.toggle_play();
                    if ev.click_count() == 2 {
                        v.fullscreen = !v.fullscreen;
                    }
                    v.last_motion = Instant::now();
                }
                cx.notify();
            }))
            .children(picture);
        div()
            .id("video-player")
            .key_context("KernMedia")
            .track_focus(&focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui_kit::black())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.player_key(ev, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                if let Content::Video(v) = &mut this.content {
                    v.last_motion = Instant::now();
                    if !v.controls_shown {
                        cx.notify();
                    }
                }
            }))
            .when(sliding, |d| {
                let e = entity.clone();
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| follow_slider(&e, window),
                    )
                    .absolute()
                    .size_0(),
                )
            })
            .child(area)
            .map(|d| match (fullscreen, show_controls) {
                (false, _) => d.child(controls),
                (true, true) => d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(controls),
                ),
                (true, false) => d,
            })
            .into_any_element()
    }
}

impl Focusable for MediaView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Where something is in the window, "x,y,w,h", for tests that click it from outside.
#[cfg(feature = "autotest")]
fn rect(b: Bounds<Pixels>) -> String {
    format!(
        "{:.0},{:.0},{:.0},{:.0}",
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height)
    )
}

/// One step of dragging the picture with the mouse.
fn drag_to(s: &mut ImageState, ev: &MouseMoveEvent) {
    let Some(last) = s.last_drag else {
        return;
    };
    if ev.dragging() {
        s.pan += ev.position - last;
        s.last_drag = Some(ev.position);
        s.clamp_pan();
    } else {
        s.last_drag = None;
    }
}

/// While a picture is dragged, the mouse is followed over the whole window: the
/// drag goes on when the pointer leaves the picture and ends wherever the button
/// is released.
fn follow_drag(entity: &Entity<MediaView>, area: Bounds<Pixels>, window: &mut Window) {
    let e = entity.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
        // Over the picture its own listener has it.
        if phase != DispatchPhase::Bubble || area.contains(&ev.position) {
            return;
        }
        e.update(cx, |this, cx| {
            if let Content::Image(s) = &mut this.content {
                drag_to(s, ev);
                cx.notify();
            }
        });
    });
    let e = entity.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            e.update(cx, |this, cx| {
                if let Content::Image(s) = &mut this.content {
                    s.last_drag = None;
                    cx.notify();
                }
            });
        }
    });
}

/// The same for the player's sliders: seeking and the volume follow the mouse
/// anywhere in the window until the button is released.
#[cfg(any(target_os = "macos", windows))]
fn follow_slider(entity: &Entity<MediaView>, window: &mut Window) {
    let e = entity.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        e.update(cx, |this, cx| {
            let Content::Video(v) = &mut this.content else {
                return;
            };
            v.last_motion = Instant::now();
            let (scrub, vol) = (v.scrubbing, v.volume_dragging);
            if !ev.dragging() {
                v.scrubbing = false;
                v.volume_dragging = false;
                cx.notify();
            } else if scrub {
                this.seek_from(ev.position.x, cx);
            } else if vol {
                this.volume_from(ev.position.x, cx);
            }
        });
    });
    let e = entity.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            e.update(cx, |this, cx| {
                if let Content::Video(v) = &mut this.content {
                    v.scrubbing = false;
                    v.volume_dragging = false;
                    cx.notify();
                }
            });
        }
    });
}

#[cfg(any(target_os = "macos", windows))]
fn clock(sec: f64) -> String {
    let s = sec.max(0.) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        // Replaced video frames leave the GPU atlas, or every frame would stay there.
        #[cfg(windows)]
        if let Content::Video(v) = &mut self.content {
            for old in v.retired.drain(..) {
                let _ = window.drop_image(old);
            }
        }
        let _ = &window;
        match &self.content {
            Content::Loading => div().size_full().bg(p.editor).into_any_element(),
            Content::Failed(msg) => {
                let msg = msg.clone();
                self.render_failed(msg, cx)
            }
            Content::Image(_) => self.render_image(cx),
            Content::Archive { .. } | Content::Binary(_) => self.render_contents(cx),
            #[cfg(any(target_os = "macos", windows))]
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
