use std::{
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    time::Instant,
};

pub mod audio;
pub mod video;

pub struct CaptureSettings {
    pub resolution: [u32; 2],
    pub fps: u32,
}
pub struct CaptureMuxer {
    // stop channels
    stop_video: Sender<()>,
    stop_audio: Sender<()>,

    // buffer channels
    video_buffer: Receiver<video::VideoBuffer>,
    audio_buffer: Receiver<audio::AudioBuffer>,

    instant: Arc<Instant>,
}

impl CaptureMuxer {
    pub fn new(settings: CaptureSettings) -> Self {
        let (audio_tx, audio_rx) = channel::<audio::AudioBuffer>();
        let (video_tx, video_rx) = channel::<video::VideoBuffer>();

        let instant = Arc::new(Instant::now());

        Self {
            stop_video: video::VideoCaptureApi::new(instant.clone(), video_tx)
                .expect("error creating video capturer"),
            stop_audio: audio::AudioCaptureApi::new(instant.clone(), audio_tx)
                .expect("error creating audio capturer"),

            video_buffer: video_rx,
            audio_buffer: audio_rx,

            instant: instant,
        }
    }
}
