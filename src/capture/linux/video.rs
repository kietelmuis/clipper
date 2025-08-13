use crossbeam::channel::{self, Receiver, Sender};
use std::{
    sync::Arc,
    thread,
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
    running: Arc<std::sync::atomic::AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}


impl VideoCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (video_tx, video_rx) = channel::unbounded::<VideoBuffer>();
        let callback = Arc::new(video_tx);
        let instant_clone = instant.clone();
        let callback_clone = callback.clone();
        // Spawn thread to send dummy video data (always running) at ~30 fps
        std::thread::spawn(move || {
            let width = 640;
            let height = 480;
            let mut color: u8 = 0;
            let frame_duration = Duration::from_nanos(1_000_000_000u64 / 30);
            let mut next = std::time::Instant::now();
            loop {
                let mut bgra = vec![0u8; (width * height * 4) as usize];
                for i in 0..(width * height) {
                    let offset = (i * 4) as usize;
                    bgra[offset] = color; // B
                    bgra[offset + 1] = 255 - color; // G
                    bgra[offset + 2] = (color.wrapping_mul(2)) % 255; // R
                    bgra[offset + 3] = 255; // A
                }
                let buffer = VideoBuffer {
                    bgra,
                    width,
                    height,
                    timestamp: instant_clone.elapsed(),
                };
                let _ = callback_clone.send(buffer);
                color = color.wrapping_add(8);
                next += frame_duration;
                let now = std::time::Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    // If we're behind, skip to now to avoid drift build-up
                    next = now;
                }
            }
        });
        Self {
            video_rx,
            instant,
            callback,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            thread_handle: None,
        }
    }

    pub fn start(&mut self) -> Result<(), ()> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        let running = self.running.clone();
        let callback = self.callback.clone();
        let instant = self.instant.clone();
        self.thread_handle = Some(thread::spawn(move || {
            let width = 640;
            let height = 480;
            let mut color: u8 = 0;
            let frame_duration = Duration::from_nanos(1_000_000_000u64 / 30);
            let mut next = std::time::Instant::now();
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let mut bgra = vec![0u8; (width * height * 4) as usize];
                for i in 0..(width * height) {
                    let offset = (i * 4) as usize;
                    bgra[offset] = color; // B
                    bgra[offset + 1] = 255 - color; // G
                    bgra[offset + 2] = (color.wrapping_mul(2)) % 255; // R
                    bgra[offset + 3] = 255; // A
                }
                let buffer = VideoBuffer {
                    bgra,
                    width,
                    height,
                    timestamp: instant.elapsed(),
                };
                let _ = callback.send(buffer);
                color = color.wrapping_add(8);
                next += frame_duration;
                let now = std::time::Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        }));
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), ()> {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        Ok(())
    }
}
