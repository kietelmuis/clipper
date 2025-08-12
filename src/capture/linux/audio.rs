use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam::channel::{self, Receiver, SendError};

#[derive(Debug)]
pub struct AudioBuffer {
    pub buffer: Vec<f32>,
    pub time: Duration,
}

// handle all pointers because we are gonna give the external to the muxer
struct InternalCaptureApi {}

// required (to send), but since inner is private the unsafe pointers inside wont be accessed
unsafe impl Send for InternalCaptureApi {}
unsafe impl Sync for InternalCaptureApi {}

// this is going to the muxer
pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    pub sample_rate: Option<u16>,
    pub channels: Option<u32>,

    inner: Arc<Mutex<InternalCaptureApi>>,
    stop_tx: channel::Sender<bool>,
}

impl Drop for InternalCaptureApi {
    fn drop(&mut self) {
        println!("cleaning audio api");
    }
}

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {}

    pub fn start(&mut self) -> Result<(), SendError<bool>> {
        self.stop_tx.send(true)?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SendError<bool>> {
        self.stop_tx.send(false)?;
        Ok(())
    }

    fn init(&mut self) {}
}

impl InternalCaptureApi {
    fn event_loop(&mut self) {}
}
