use std::fmt;

#[cfg(not(windows))]
pub use crate::capture::linux::video::{VideoBuffer, VideoCaptureApi};

#[cfg(windows)]
pub use crate::capture::windows::video::{VideoBuffer, VideoCaptureApi};

#[derive(Clone)]
pub struct Resolution {
    pub width: i32,
    pub height: i32,
}

impl fmt::Debug for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}
