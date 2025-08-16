#[cfg(not(windows))]
use crate::capture::linux::video::VideoCaptureApi as LinuxCaptureApi;
#[cfg(not(windows))]
pub use crate::capture::linux::video::{VideoBuffer, VideoCaptureApi};

#[cfg(windows)]
use crate::capture::windows::video::VideoCaptureApi as WindowsCaptureApi;
#[cfg(windows)]
pub use crate::capture::windows::video::{VideoBuffer, VideoCaptureApi};
