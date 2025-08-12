use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use crossbeam::channel::{self, Receiver, Sender, SendError};
use std::f32::consts::PI;


#[derive(Debug)]
pub struct AudioBuffer {
    pub buffer: Vec<f32>,
    pub time: Duration,
}

pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    pub sample_rate: u32,
    pub channels: u32,
    stop_tx: Sender<bool>,
    instant: Arc<Instant>,
}

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (audio_tx, audio_rx) = channel::unbounded::<AudioBuffer>();
        let (stop_tx, stop_rx) = channel::unbounded::<bool>();

        let sample_rate = 48000;
        let channels = 1;

        // Spawn thread to send dummy beep data (always running)
        let instant_clone = instant.clone();
        std::thread::spawn(move || {
            let mut t: f32 = 0.0;
            let mut freq: f32 = 440.0;
            let frame_size = 1024;
            loop {
                // Generate a simple beep that changes frequency
                let mut buffer = Vec::with_capacity(frame_size);
                for _ in 0..frame_size {
                    let sample = (t * freq * 2.0 * PI).sin() as f32 * 0.2;
                    buffer.push(sample);
                    t += 1.0 / sample_rate as f32;
                }
                // Change frequency every second
                freq = 440.0 + ((instant_clone.elapsed().as_secs() % 5) as f32) * 110.0;
                let audio_buffer = AudioBuffer {
                    buffer,
                    time: instant_clone.elapsed(),
                };
                if audio_tx.send(audio_buffer).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        AudioCaptureApi {
            audio_rx,
            sample_rate,
            channels,
            stop_tx,
            instant,
        }
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
