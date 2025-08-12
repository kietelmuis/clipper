use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

use crossbeam::channel::{self, Receiver, Sender, SendError};
use pipewire::{self as pw, properties::properties};
use pipewire::main_loop::MainLoop;
use pipewire::context::Context;

#[derive(Debug)]
pub struct AudioBuffer {
    pub buffer: Vec<f32>,
    pub time: Duration,
}

// this is going to the muxer
pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    audio_tx: Sender<AudioBuffer>,
    running: Arc<Mutex<bool>>,
    instant: Arc<Instant>,
}

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (tx, rx) = channel::unbounded::<AudioBuffer>();
        Self {
            audio_rx: rx,
            audio_tx: tx,
            running: Arc::new(Mutex::new(false)),
            instant,
        }
    }

    pub fn start(&mut self) -> Result<(), SendError<bool>> {
        let running = self.running.clone();
        let tx = self.audio_tx.clone();
        let instant = self.instant.clone();

        *running.lock().unwrap() = true;

        thread::spawn(move || {
            // Initialize PipeWire
            pw::init();
            let main_loop = MainLoop::new(None).unwrap();
            let context = Context::new(&main_loop).unwrap();
            let core = context.connect(None).unwrap();

            let stream = pw::stream::Stream::new(
                &core,
                "audio-capture",
                properties! {
                    "media.role" => "Capture",
                },
            ).unwrap();

            // TODO: Setup Pipewire stream parameters for audio format and direction
            // For now, this is a stub to allow compilation

            // TODO: Set process callback for Pipewire stream
            // For now, simulate audio data every 100ms
            loop {
                if !*running.lock().unwrap() {
                    break;
                }
                let time = instant.elapsed();
                let buffer = AudioBuffer {
                    buffer: vec![0.0; 48000 * 2 / 10], // Dummy audio data for 100ms
                    time,
                };
                let _ = tx.send(buffer);
                thread::sleep(Duration::from_millis(100));
            }

            main_loop.run();
        });

        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SendError<bool>> {
        *self.running.lock().unwrap() = false;
        Ok(())
    }

    fn event_loop(&mut self) {
        // Not needed, handled by PipeWire main loop
    }

    fn init(&mut self) {
        // Not needed, handled in start()
    }
}
