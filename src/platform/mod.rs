//! Platform-specific details. Everything else in KERN is shared.
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

pub mod tools;
#[cfg(target_os = "macos")]
pub mod video;
