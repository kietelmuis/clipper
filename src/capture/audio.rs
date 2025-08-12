use std::{sync::Arc, time::Instant};

use crossbeam::channel::{SendError, Sender};

#[cfg(not(windows))]
use crate::capture::linux::audio::AudioCaptureApi as LinuxCaptureApi;
#[cfg(not(windows))]
pub use crate::capture::linux::audio::{AudioBuffer, AudioCaptureApi};

#[cfg(windows)]
use crate::capture::windows::audio::AudioCaptureApi as WindowsCaptureApi;
#[cfg(windows)]
pub use crate::capture::windows::audio::{AudioBuffer, AudioCaptureApi};

struct AudioCapture {
    stop_tx: Sender<bool>,
}

impl AudioCapture {
    #[cfg(windows)]
    pub fn new(instant: Arc<Instant>) -> WindowsCaptureApi {
        WindowsCaptureApi::new(instant)
    }

    #[cfg(not(windows))]
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
