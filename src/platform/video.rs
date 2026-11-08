//! Video playback through the system media stack. AVPlayer plays and owns the
//! audio; frames come as CVPixelBuffers from AVPlayerItemVideoOutput and are drawn
//! by the app's own surface element, so they sit in the normal layout.
//! Only used on the main thread.
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::path::Path;
use std::ptr::NonNull;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    fn CACurrentMediaTime() -> f64;
}

/// '420f': biplanar full-range YCbCr, what the surface shader expects.
const PIXEL_FORMAT_420F: u32 = 0x3432_3066;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Loading,
    Ready,
    Failed(String),
}

/// Players alive right now: a closed tab must leave none behind.
static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn live_players() -> usize {
    LIVE.load(std::sync::atomic::Ordering::SeqCst)
}

pub struct VideoPlayer {
    player: Retained<AnyObject>,
    item: Retained<AnyObject>,
    output: Retained<AnyObject>,
    seeded: bool,
}

fn seconds(t: CMTime) -> Option<f64> {
    if !t.flags.contains(CMTimeFlags::Valid)
        || t.flags.contains(CMTimeFlags::Indefinite)
        || t.timescale == 0
    {
        return None;
    }
    Some(t.value as f64 / t.timescale as f64)
}

fn cmtime(sec: f64) -> CMTime {
    CMTime {
        value: (sec * 600.0) as i64,
        timescale: 600,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

impl VideoPlayer {
    pub fn open(path: &Path) -> Result<Self, String> {
        let class = |name: &std::ffi::CStr| {
            AnyClass::get(name).ok_or_else(|| "AVFoundation is not available".to_string())
        };
        unsafe {
            let path_str = NSString::from_str(&path.to_string_lossy());
            let url: Retained<NSURL> = NSURL::fileURLWithPath(&path_str);
            let item: Retained<AnyObject> =
                msg_send![class(c"AVPlayerItem")?, playerItemWithURL: &*url];

            let keys = [
                NSString::from_str("PixelFormatType"),
                NSString::from_str("IOSurfaceProperties"),
                NSString::from_str("MetalCompatibility"),
            ];
            let surface_props: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
            let format = NSNumber::new_u32(PIXEL_FORMAT_420F);
            let metal = NSNumber::new_bool(true);
            let values: [&AnyObject; 3] = [&format, &surface_props, &metal];
            let key_refs: [&NSString; 3] = [&keys[0], &keys[1], &keys[2]];
            let attrs: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_slices(&key_refs, &values);

            let alloc: Allocated<AnyObject> = msg_send![class(c"AVPlayerItemVideoOutput")?, alloc];
            let output: Option<Retained<AnyObject>> =
                msg_send![alloc, initWithPixelBufferAttributes: &*attrs];
            let output = output.ok_or("cannot create a video output")?;
            let _: () = msg_send![&*item, addOutput: &*output];

            let player: Retained<AnyObject> =
                msg_send![class(c"AVPlayer")?, playerWithPlayerItem: &*item];
            let _: () = msg_send![&*player, pause];
            LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Self {
                player,
                item,
                output,
                seeded: false,
            })
        }
    }

    pub fn status(&self) -> Status {
        unsafe {
            let status: isize = msg_send![&*self.item, status];
            match status {
                1 => Status::Ready,
                2 => {
                    let err: Option<Retained<AnyObject>> = msg_send![&*self.item, error];
                    let text = err.map(|e| {
                        let d: Retained<NSString> = msg_send![&*e, localizedDescription];
                        let reason: Option<Retained<NSString>> =
                            msg_send![&*e, localizedFailureReason];
                        match reason {
                            Some(r) => format!("{d}: {r}"),
                            None => d.to_string(),
                        }
                    });
                    Status::Failed(text.unwrap_or_else(|| "The file cannot be played".into()))
                }
                _ => Status::Loading,
            }
        }
    }

    /// The first frame only appears after a seek, as the player starts paused.
    pub fn seed_first_frame(&mut self) {
        if !self.seeded {
            self.seeded = true;
            self.seek(0.0);
        }
    }

    pub fn natural_size(&self) -> (f64, f64) {
        let s: CGSize = unsafe { msg_send![&*self.item, presentationSize] };
        (s.width, s.height)
    }

    pub fn is_playing(&self) -> bool {
        let rate: f32 = unsafe { msg_send![&*self.player, rate] };
        rate != 0.0
    }

    pub fn play(&self) {
        // Pressing play at the end starts over.
        if let (Some(cur), Some(dur)) = (self.current_time(), self.duration())
            && cur >= dur - 0.05
        {
            self.seek(0.0);
        }
        let _: () = unsafe { msg_send![&*self.player, play] };
    }

    pub fn pause(&self) {
        let _: () = unsafe { msg_send![&*self.player, pause] };
    }

    pub fn seek(&self, sec: f64) {
        let zero = cmtime(0.0);
        let _: () = unsafe {
            msg_send![&*self.player, seekToTime: cmtime(sec.max(0.0)), toleranceBefore: zero, toleranceAfter: zero]
        };
    }

    pub fn current_time(&self) -> Option<f64> {
        let t: CMTime = unsafe { msg_send![&*self.player, currentTime] };
        seconds(t)
    }

    pub fn duration(&self) -> Option<f64> {
        let t: CMTime = unsafe { msg_send![&*self.item, duration] };
        seconds(t)
    }

    pub fn volume(&self) -> f32 {
        unsafe { msg_send![&*self.player, volume] }
    }

    pub fn set_volume(&self, v: f32) {
        let _: () = unsafe { msg_send![&*self.player, setVolume: v.clamp(0.0, 1.0)] };
    }

    pub fn muted(&self) -> bool {
        let m: Bool = unsafe { msg_send![&*self.player, isMuted] };
        m.as_bool()
    }

    pub fn set_muted(&self, m: bool) {
        let _: () = unsafe { msg_send![&*self.player, setMuted: Bool::new(m)] };
    }

    /// A new decoded frame, if there is one for the current time.
    pub fn poll_frame(&self) -> Option<CFRetained<CVPixelBuffer>> {
        unsafe {
            let host = CACurrentMediaTime();
            let time: CMTime = msg_send![&*self.output, itemTimeForHostTime: host];
            let has: Bool = msg_send![&*self.output, hasNewPixelBufferForItemTime: time];
            if !has.as_bool() {
                return None;
            }
            let none: *mut CMTime = std::ptr::null_mut();
            let raw: *mut CVPixelBuffer = msg_send![&*self.output, copyPixelBufferForItemTime: time, itemTimeForDisplay: none];
            // "copy" returns an owned reference.
            NonNull::new(raw).map(|p| CFRetained::from_raw(p))
        }
    }
}

impl Drop for VideoPlayer {
    /// Stop the sound and release the decoder when a tab closes.
    fn drop(&mut self) {
        LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        unsafe {
            let _: () = msg_send![&*self.player, pause];
            let _: () = msg_send![&*self.item, removeOutput: &*self.output];
            let none: Option<&AnyObject> = None;
            let _: () = msg_send![&*self.player, replaceCurrentItemWithPlayerItem: none];
        }
    }
}
