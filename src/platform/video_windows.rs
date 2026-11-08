//! Video playback through Media Foundation's Media Engine. The engine decodes,
//! keeps picture and sound in step and plays the sound. KERN asks it for every
//! new frame ("frame server" mode), gets it as BGRA pixels in a WIC bitmap and
//! draws it in the tab like any other image. Only used on the UI thread.
use std::path::Path;
use std::sync::OnceLock;
use windows::Win32::Foundation::{RECT, S_OK};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad, WICBitmapLockRead, WICRect,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{BSTR, IUnknown, Interface, implement};

/// DXGI_FORMAT_B8G8R8A8_UNORM: the pixel order GPUI images use.
const BGRA: u32 = 87;
/// Frames wider than this are scaled down while copying: enough for a tab or a
/// screen, and it keeps the per-frame copy small.
const MAX_WIDTH: u32 = 1920;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Loading,
    Ready,
    Failed(String),
}

/// One decoded picture, top row first, 4 bytes per pixel in B, G, R, A order.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The engine wants someone to tell about its events; KERN asks for the state
/// it needs instead, so nothing happens here.
#[implement(IMFMediaEngineNotify)]
struct Notify;

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, _event: u32, _param1: usize, _param2: u32) -> windows::core::Result<()> {
        Ok(())
    }
}

pub struct VideoPlayer {
    engine: IMFMediaEngine,
    wic: IWICImagingFactory,
    bitmap: Option<(u32, u32, IWICBitmap)>,
    seeded: bool,
}

fn startup() -> Result<(), String> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
                .map_err(|e| format!("Media Foundation is not available: {}", e.message()))
        })
        .clone()
}

fn why(e: windows::core::Error) -> String {
    let m = e.message();
    if m.is_empty() {
        format!("{:?}", e.code())
    } else {
        m.to_string()
    }
}

impl VideoPlayer {
    pub fn open(path: &Path) -> Result<Self, String> {
        startup()?;
        unsafe {
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| format!("Media Foundation is not available: {}", why(e)))?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2).map_err(why)?;
            let attrs = attrs.ok_or("cannot create Media Foundation attributes")?;
            let notify: IMFMediaEngineNotify = Notify.into();
            let unknown: IUnknown = notify.cast().map_err(why)?;
            attrs
                .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &unknown)
                .map_err(why)?;
            attrs
                .SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, BGRA)
                .map_err(why)?;
            let engine = factory.CreateInstance(0, &attrs).map_err(why)?;
            engine.SetAutoPlay(false).map_err(why)?;
            engine
                .SetPreload(MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC)
                .map_err(why)?;
            let url = BSTR::from(crate::lsp::path_to_uri(path));
            engine.SetSource(&url).map_err(why)?;
            engine.Load().map_err(why)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(why)?;
            Ok(Self {
                engine,
                wic,
                bitmap: None,
                seeded: false,
            })
        }
    }

    pub fn status(&self) -> Status {
        unsafe {
            if let Ok(err) = self.engine.GetError() {
                let what = match err.GetErrorCode() {
                    1 => "Loading was stopped",
                    2 => "The file could not be read",
                    3 => "The video could not be decoded",
                    4 => "Windows has no decoder for this format or codec",
                    _ => "The file cannot be played",
                };
                return Status::Failed(match err.GetExtendedErrorCode() {
                    Err(e) if !e.message().is_empty() => format!("{what}: {}", e.message()),
                    _ => what.to_string(),
                });
            }
            // HAVE_METADATA or more: the size and the length are known.
            if self.engine.GetReadyState() >= 1 {
                Status::Ready
            } else {
                Status::Loading
            }
        }
    }

    /// The first picture appears after a seek, as the engine starts paused.
    pub fn seed_first_frame(&mut self) {
        if !self.seeded {
            self.seeded = true;
            self.seek(0.0);
        }
    }

    pub fn natural_size(&self) -> (f64, f64) {
        let (mut w, mut h) = (0u32, 0u32);
        unsafe {
            if self.engine.HasVideo().as_bool() {
                let _ = self.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h));
            }
        }
        (w as f64, h as f64)
    }

    pub fn is_playing(&self) -> bool {
        unsafe { !self.engine.IsPaused().as_bool() && !self.engine.IsEnded().as_bool() }
    }

    pub fn play(&self) {
        // Pressing play at the end starts over.
        if let (Some(cur), Some(dur)) = (self.current_time(), self.duration())
            && cur >= dur - 0.05
        {
            self.seek(0.0);
        }
        unsafe {
            let _ = self.engine.Play();
        }
    }

    pub fn pause(&self) {
        unsafe {
            let _ = self.engine.Pause();
        }
    }

    pub fn seek(&self, sec: f64) {
        unsafe {
            let _ = self.engine.SetCurrentTime(sec.max(0.0));
        }
    }

    pub fn current_time(&self) -> Option<f64> {
        let t = unsafe { self.engine.GetCurrentTime() };
        t.is_finite().then_some(t)
    }

    pub fn duration(&self) -> Option<f64> {
        let d = unsafe { self.engine.GetDuration() };
        (d.is_finite() && d > 0.0).then_some(d)
    }

    pub fn volume(&self) -> f32 {
        unsafe { self.engine.GetVolume() as f32 }
    }

    pub fn set_volume(&self, v: f32) {
        unsafe {
            let _ = self.engine.SetVolume(v.clamp(0.0, 1.0) as f64);
        }
    }

    pub fn muted(&self) -> bool {
        unsafe { self.engine.GetMuted().as_bool() }
    }

    pub fn set_muted(&self, m: bool) {
        unsafe {
            let _ = self.engine.SetMuted(m);
        }
    }

    /// The frame for the current time, if the engine has a new one.
    pub fn poll_frame(&mut self) -> Option<Frame> {
        unsafe {
            // S_FALSE means "no new frame": the generated wrapper would call
            // that a success too, so ask the vtable for the raw result.
            let mut pts = 0i64;
            let hr = (Interface::vtable(&self.engine).OnVideoStreamTick)(
                Interface::as_raw(&self.engine),
                &mut pts,
            );
            if hr != S_OK {
                return None;
            }
            let (w, h) = self.natural_size();
            let (w, h) = (w as u32, h as u32);
            if w == 0 || h == 0 {
                return None;
            }
            let (tw, th) = if w > MAX_WIDTH {
                (MAX_WIDTH, ((h as u64 * MAX_WIDTH as u64) / w as u64).max(1) as u32)
            } else {
                (w, h)
            };
            if !matches!(&self.bitmap, Some((bw, bh, _)) if *bw == tw && *bh == th) {
                let b = self
                    .wic
                    .CreateBitmap(tw, th, &GUID_WICPixelFormat32bppBGRA, WICBitmapCacheOnLoad)
                    .ok()?;
                self.bitmap = Some((tw, th, b));
            }
            let (_, _, bitmap) = self.bitmap.as_ref()?;
            let dst = RECT {
                left: 0,
                top: 0,
                right: tw as i32,
                bottom: th as i32,
            };
            self.engine
                .TransferVideoFrame(bitmap, None, &dst, None)
                .ok()?;
            let area = WICRect {
                X: 0,
                Y: 0,
                Width: tw as i32,
                Height: th as i32,
            };
            let lock = bitmap.Lock(&area, WICBitmapLockRead.0 as u32).ok()?;
            let stride = lock.GetStride().ok()? as usize;
            let (mut len, mut data) = (0u32, std::ptr::null_mut::<u8>());
            lock.GetDataPointer(&mut len, &mut data).ok()?;
            if data.is_null() || (len as usize) < stride * (th as usize - 1) + tw as usize * 4 {
                return None;
            }
            let row = tw as usize * 4;
            let mut bgra = Vec::with_capacity(row * th as usize);
            for y in 0..th as usize {
                bgra.extend_from_slice(std::slice::from_raw_parts(data.add(y * stride), row));
            }
            // Video has no transparency; some decoders leave alpha at 0, which
            // would draw nothing.
            for a in bgra.iter_mut().skip(3).step_by(4) {
                *a = 255;
            }
            Some(Frame {
                width: tw,
                height: th,
                bgra,
            })
        }
    }
}

impl Drop for VideoPlayer {
    /// Stop the sound and release the decoder when a tab closes.
    fn drop(&mut self) {
        unsafe {
            let _ = self.engine.Pause();
            let _ = self.engine.Shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes the test clip without a window: frames come, they have the
    /// clip's size and are not empty, seeking and the clock work.
    #[test]
    fn plays_the_h264_aac_clip() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let clip = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clip-h264-aac.mp4");
        let mut p = VideoPlayer::open(&clip).expect("open");
        let start = std::time::Instant::now();
        while p.status() == Status::Loading && start.elapsed().as_secs() < 20 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(p.status(), Status::Ready, "status");
        assert_eq!(p.natural_size(), (640.0, 360.0));
        let dur = p.duration().expect("duration");
        assert!((dur - 6.0).abs() < 0.2, "duration {dur}");
        p.seed_first_frame();
        p.set_volume(0.0);
        p.play();
        let mut frames = 0;
        let mut lit = false;
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 4 {
            if let Some(f) = p.poll_frame() {
                assert_eq!((f.width, f.height), (640, 360));
                lit |= f.bgra.chunks(4).any(|px| px[0] > 40 || px[1] > 40 || px[2] > 40);
                frames += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("frames {frames} at t={:?} status {:?}", p.current_time(), p.status());
        assert!(frames > 20, "only {frames} frames");
        assert!(lit, "the frames are black");
        assert!(p.current_time().unwrap_or(0.0) > 1.0);
        p.pause();
        p.seek(5.0);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!((p.current_time().unwrap() - 5.0).abs() < 0.3);
    }
}
