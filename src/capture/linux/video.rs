use crossbeam::channel::{self, Receiver};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct VideoBuffer {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp: Duration,
}

impl Drop for VideoCaptureApi {
    fn drop(&mut self) {
        println!("cleaning video api");
        _ = self.stop();
    }
}

pub struct VideoCaptureApi {
    pub video_rx: Receiver<VideoBuffer>,

    instant: Arc<Instant>,
    callback: Arc<Sender<VideoBuffer>>,
}

impl VideoCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (video_tx, video_rx) = channel::unbounded::<VideoBuffer>();

        let mut capture_api = Self {
            video_rx,

            instant,
            callback: Arc::new(video_tx),
        };

        capture_api.init();
        capture_api
    }

    pub fn start(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn init(&mut self) {}
    fn handle_frame() {}
}
