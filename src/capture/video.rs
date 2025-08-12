use std::{sync::Arc, time::Instant};

use crossbeam::channel::{SendError, Sender};

#[cfg(unix)]
use crate::capture::linux::video::VideoCaptureApi as LinuxCaptureApi;
#[cfg(unix)]
pub use crate::capture::linux::video::{VideoBuffer, VideoCaptureApi};

#[cfg(windows)]
use crate::capture::windows::video::VideoCaptureApi as WindowsCaptureApi;
#[cfg(windows)]
pub use crate::capture::windows::video::{VideoBuffer, VideoCaptureApi};

struct VideoCapture {
    stop_tx: Sender<bool>,
}

impl VideoCapture {
    #[cfg(windows)]
    pub fn new(instant: Arc<Instant>) -> WindowsCaptureApi {
        WindowsCaptureApi::new(instant)
    }

    #[cfg(unix)]
    pub fn new(instant: Arc<Instant>) -> LinuxCaptureApi {
        LinuxCaptureApi::new(instant)
    }

    pub fn start(&mut self) -> Result<(), SendError<bool>> {
        self.stop_tx.send(true)?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SendError<bool>> {
        self.stop_tx.send(false)?;
        Ok(())
    }
}
