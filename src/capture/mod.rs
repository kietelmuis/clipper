use rusty_ffmpeg::ffi::{
    self as ffmpeg, AV_CODEC_ID_H264, AV_PIX_FMT_BGRA, AVChannelLayout, av_channel_layout_default,
};

use crossbeam::channel;

use std::{sync::Arc, time::Instant};

pub mod audio;
pub mod video;

pub struct CaptureSettings {
    pub resolution: [u32; 2],
    pub fps: u32,
    pub debug: bool,
}
pub struct CaptureMuxer {
    // stop channels
    stop_video: channel::Sender<()>,
    stop_audio: channel::Sender<()>,

    recv_audio: channel::Receiver<audio::AudioBuffer>,
    recv_video: channel::Receiver<video::VideoBuffer>,

    instant: Arc<Instant>,
}

impl CaptureMuxer {
    pub fn new(settings: CaptureSettings) -> Self {
        let (audio_tx, audio_rx) = channel::unbounded::<audio::AudioBuffer>();
        let (video_tx, video_rx) = channel::unbounded::<video::VideoBuffer>();

        let audio_clone = audio_rx.clone();
        let video_clone = video_rx.clone();

        if settings.debug {
            std::thread::spawn(move || {
                let mut frames = 0;
                let mut last_reset = std::time::Instant::now();

                loop {
                    video_clone.recv().expect("failed to receive frame");
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
                    audio_clone.recv().expect("failed to receive audio");
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

            recv_video: video_rx,
            recv_audio: audio_rx,

            instant: instant, // move into self
        };

        muxer.mux();
        muxer
    }

    // ffmpeg
    // result < 0 == error
    fn mux(&mut self) {
        let codec = unsafe { ffmpeg::avcodec_find_encoder(AV_CODEC_ID_H264) };
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

        println!("codec is ready!");

        // allocate and init sws context
        let sws_context = unsafe {
            ffmpeg::sws_getContext(
                1920,
                1080,
                ffmpeg::AV_PIX_FMT_BGRA,
                1920,
                1080,
                ffmpeg::AV_PIX_FMT_YUV420P,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if sws_context.is_null() {
            panic!("failed to allocate and init sws context");
        }

        println!("sws context is ready!");

        loop {
            let mut video_buffer = self
                .recv_video
                .recv()
                .expect("failed to receive video bufer");

            // create empty bgra avframe
            let mut bgra_frame = unsafe { ffmpeg::av_frame_alloc() };
            if bgra_frame.is_null() {
                panic!("Failed to allocate AVFrame!");
            }

            // fill bgra avframe data
            unsafe {
                (*bgra_frame).format = ffmpeg::AV_PIX_FMT_BGRA;
                (*bgra_frame).width = 1920;
                (*bgra_frame).height = 1080;
                (*bgra_frame).data[0] = video_buffer.bgra.as_mut_ptr();
                (*bgra_frame).linesize[0] = 1080 * 4;
            };

            // create empty yuv avframe
            let mut yuv_frame = unsafe { ffmpeg::av_frame_alloc() };
            if yuv_frame.is_null() {
                unsafe { ffmpeg::av_frame_free(&mut bgra_frame) };
                panic!("Failed to allocate AVFrame!");
            }

            // fill required base avframe data
            unsafe {
                (*yuv_frame).format = ffmpeg::AV_PIX_FMT_YUV420P;
                (*yuv_frame).height = 1920;
                (*yuv_frame).width = 1080;
            };

            // free if failed to allocate buffer data
            if unsafe { ffmpeg::av_frame_get_buffer(yuv_frame, 32) } < 0 {
                unsafe {
                    ffmpeg::av_frame_free(&mut bgra_frame);
                    ffmpeg::av_frame_free(&mut yuv_frame);
                }
                panic!("could not allocate frame buffer!");
            }

            let src_data = [(unsafe { *bgra_frame }).data[0] as *const u8];
            let src_linesize = [unsafe { (*bgra_frame).linesize[0] }];

            if unsafe {
                ffmpeg::sws_scale(
                    sws_context,
                    src_data.as_ptr(),
                    src_linesize.as_ptr(),
                    0,
                    1080,
                    (*yuv_frame).data.as_mut_ptr(),
                    (*yuv_frame).linesize.as_mut_ptr(),
                )
            } < 0
            {
                unsafe {
                    ffmpeg::av_frame_free(&mut bgra_frame);
                    ffmpeg::av_frame_free(&mut yuv_frame);
                }
                panic!("could not allocate frame buffer!");
            }

            // clean up after usage
            unsafe {
                ffmpeg::av_frame_free(&mut bgra_frame);
                ffmpeg::av_frame_free(&mut yuv_frame);
            }
        }
    }
}
