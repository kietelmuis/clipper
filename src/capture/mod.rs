use rusty_ffmpeg::ffi as ffmpeg;

use std::{
    ffi::CString,
    str::FromStr,
    sync::{
        Arc,
        mpsc::{Sender, channel},
    },
    time::Instant,
};

pub mod audio;
pub mod video;

pub struct CaptureSettings {
    pub resolution: [u32; 2],
    pub fps: u32,
    pub debug: bool,
}
pub struct CaptureMuxer {
    // stop channels
    stop_video: Sender<()>,
    stop_audio: Sender<()>,

    instant: Arc<Instant>,
}

impl CaptureMuxer {
    pub fn new(settings: CaptureSettings) -> Self {
        let (audio_tx, audio_rx) = channel::<audio::AudioBuffer>();
        let (video_tx, video_rx) = channel::<video::VideoBuffer>();

        if settings.debug {
            std::thread::spawn(move || {
                let mut frames = 0;
                let mut last_reset = std::time::Instant::now();

                loop {
                    video_rx.recv().expect("failed to receive frame");
                    frames += 1;

                    let now = std::time::Instant::now();
                    if now.duration_since(last_reset).as_secs() >= 1 {
                        println!("capture: {} fps", frames);
                        frames = 0;
                        last_reset = now;
                    }
                }
            });

            std::thread::spawn(move || {
                let mut samples = 0;
                let mut last_reset = std::time::Instant::now();

                loop {
                    audio_rx.recv().expect("failed to receive audio");
                    samples += 1;

                    let now = std::time::Instant::now();
                    if now.duration_since(last_reset).as_secs() >= 1 {
                        println!("capture: {} samples/sec", samples);
                        samples = 0;
                        last_reset = now;
                    }
                }
            });
        }

        let instant = Arc::new(Instant::now());

        let mut muxer = Self {
            stop_video: video::VideoCaptureApi::new(instant.clone(), video_tx),
            stop_audio: audio::AudioCaptureApi::new(instant.clone(), audio_tx),

            instant: instant, // move into self
        };

        muxer.mux();
        muxer
    }

    fn mux(&mut self) {
        println!("libavformat v{}", ffmpeg::LIBAVFORMAT_VERSION_MAJOR);

        let codec_name = CString::new("libx264").expect("CString::new failed");
        let codec = unsafe { ffmpeg::avcodec_find_encoder_by_name(codec_name.as_ptr()) };
        if codec.is_null() {
            panic!("failed to find h264 encoder");
        }

        // allocate encoder
        let encoder = unsafe { ffmpeg::avcodec_alloc_context3(codec) };
        if encoder.is_null() {
            panic!("failed to allocate codec context");
        }

        // configure encoder
        unsafe {
            (*encoder).width = 1920;
            (*encoder).height = 1080;
            (*encoder).pix_fmt = ffmpeg::AV_PIX_FMT_YUV420P; // encoder expects YUV420p
            (*encoder).time_base = ffmpeg::AVRational { num: 1, den: 75 }; // for my 75hz monitor
        }

        if unsafe { ffmpeg::avcodec_open2(encoder, codec, std::ptr::null_mut()) } < 0 {
            panic!("Failed to open encoder");
        }
    }
}
