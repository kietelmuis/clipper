#[cfg(not(windows))]
use crate::capture::linux::audio::AudioCaptureApi as LinuxCaptureApi;
#[cfg(not(windows))]
pub use crate::capture::linux::audio::{AudioBuffer, AudioCaptureApi};

#[cfg(windows)]
use crate::capture::windows::audio::AudioCaptureApi as WindowsCaptureApi;
#[cfg(windows)]
pub use crate::capture::windows::audio::{AudioBuffer, AudioCaptureApi};
