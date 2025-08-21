#[cfg(not(windows))]
pub use crate::capture::linux::audio::{AudioBuffer, AudioCaptureApi};

#[cfg(windows)]
pub use crate::capture::windows::audio::{AudioBuffer, AudioCaptureApi};
