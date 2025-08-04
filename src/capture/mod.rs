use rusty_ffmpeg::ffi::{
    self as ffmpeg, AV_CHANNEL_ORDER_UNSPEC, AV_CODEC_ID_AAC, AV_CODEC_ID_H264, AV_CODEC_ID_HEVC,
    AV_PIX_FMT_YUV420P, AV_SAMPLE_FMT_FLTP, AVChannelLayout, AVChannelLayout__bindgen_ty_1,
    AVCodecContext, AVFormatContext, AVFrame, AVMEDIA_TYPE_VIDEO, AVStream,
    av_channel_layout_default,
};

use crossbeam::channel::{self, SendError};
use windows::Win32::UI::Shell::PAI_EXPIRETIME;

use std::{ffi::CString, sync::Arc, thread::JoinHandle, time::Instant};

use crate::capture::{audio::AudioCaptureApi, video::VideoCaptureApi};

pub mod audio;
pub mod video;

pub struct CaptureSettings {
    pub resolution: [u32; 2],
    pub fps: u32,
}

pub struct CaptureMuxer {
    // communication channels
    video_api: VideoCaptureApi,
    audio_api: AudioCaptureApi,

    instant: Arc<Instant>,
}

impl CaptureMuxer {
    pub fn new(settings: CaptureSettings) -> Self {
        let instant = Arc::new(Instant::now());

        let video_api = VideoCaptureApi::new(instant.clone());
        let audio_api = AudioCaptureApi::new(instant.clone());

        Self {
            video_api,
            audio_api,
            instant,
        }
    }

    pub fn stop_recording(&mut self) {
        if let Err(e) = self.video_api.stop() {
            eprintln!("Failed to stop video capture: {:?}", e);
        }
        if let Err(e) = self.audio_api.stop() {
            eprintln!("Failed to stop audio capture: {:?}", e);
        }
    }

    // ffmpeg
    // result < 0 == error
    pub fn start_muxer(&mut self) {
        // init all yo shi
        unsafe { ffmpeg::avdevice_register_all() };

        // choose them formats bruh
        let audio_format = AV_CODEC_ID_AAC;
        let sample_rate = 48000;

        let codec_format = AV_CODEC_ID_H264;
        let pixel_format = AV_PIX_FMT_YUV420P;

        let timebase = ffmpeg::AVRational { num: 1, den: 75 };

        let video_codec = unsafe { ffmpeg::avcodec_find_encoder(codec_format) };
        if video_codec.is_null() {
            panic!("failed to find video encoder");
        }

        let audio_codec = unsafe { ffmpeg::avcodec_find_encoder(audio_format) };
        if audio_codec.is_null() {
            panic!("failed to find audio encoder");
        }

        // allocate audio encoder
        let audio_encoder = unsafe { ffmpeg::avcodec_alloc_context3(audio_codec) };
        if audio_encoder.is_null() {
            panic!("failed to allocate codec context");
        }

        // make default avchannellayout
        let mut channel_layout = AVChannelLayout {
            order: AV_CHANNEL_ORDER_UNSPEC,
            nb_channels: 0,
            opaque: std::ptr::null_mut(),
            u: AVChannelLayout__bindgen_ty_1 { mask: 0 },
        };

        // configure audio encoder
        unsafe {
            av_channel_layout_default(&mut channel_layout, 2);
            (*audio_encoder).ch_layout = channel_layout;
            (*audio_encoder).time_base = timebase;
            (*audio_encoder).sample_fmt = AV_SAMPLE_FMT_FLTP;
            (*audio_encoder).sample_rate = sample_rate;
        }

        // open audio encoder
        if unsafe { ffmpeg::avcodec_open2(audio_encoder, audio_codec, std::ptr::null_mut()) } < 0 {
            panic!("Failed to open audio encoder");
        }

        // allocate video encoder
        let video_encoder = unsafe { ffmpeg::avcodec_alloc_context3(video_codec) };
        if video_encoder.is_null() {
            panic!("failed to allocate codec context");
        }

        // configure video encoder
        unsafe {
            (*video_encoder).width = 1920;
            (*video_encoder).height = 1080;
            (*video_encoder).pix_fmt = pixel_format;
            (*video_encoder).time_base = timebase; // for my 75hz
            (*video_encoder).bit_rate = 9_000_000;
        }

        // open video encoder
        if unsafe { ffmpeg::avcodec_open2(video_encoder, video_codec, std::ptr::null_mut()) } < 0 {
            panic!("Failed to open video encoder");
        }

        let mut output = unsafe { ffmpeg::avformat_alloc_context() };

        let format_name = CString::new("mp4").expect("cstring fail");
        let file_name = CString::new("video.mp4").expect("cstring fail");

        if unsafe {
            ffmpeg::avformat_alloc_output_context2(
                &mut output,
                std::ptr::null(),
                format_name.as_ptr(),
                file_name.as_ptr(),
            )
        } > 0
        {
            panic!("output ctx fail");
        }

        // create new stream
        let stream = unsafe { ffmpeg::avformat_new_stream(output, std::ptr::null_mut()) };
        if stream.is_null() {
            panic!("failed to create stream");
        }
        unsafe { *stream }.time_base = ffmpeg::AVRational { num: 1, den: 75 };

        // edit stream's codec parameters
        let codec_params = unsafe { &mut *(*stream).codecpar };
        codec_params.codec_type = AVMEDIA_TYPE_VIDEO;
        codec_params.codec_id = codec_format;
        codec_params.width = 1920;
        codec_params.height = 1080;
        codec_params.format = pixel_format;
        codec_params.bit_rate = 5_000_000;

        // open aviocontext within avformatcontext for writing
        if unsafe {
            ffmpeg::avio_open(
                &mut (*output).pb,
                file_name.as_ptr(),
                2, // cooked
            )
        } > 0
        {
            panic!("failed to open avio for writing");
        }

        self.encode_frames(video_encoder, audio_encoder, output, stream);
    }

    fn encode_frames(
        &mut self,
        mut video_encoder: *mut AVCodecContext,
        mut audio_encoder: *mut AVCodecContext,
        output: *mut AVFormatContext,
        stream: *mut AVStream,
    ) {
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

        // write header for video
        if unsafe { ffmpeg::avformat_write_header(output, std::ptr::null_mut()) } < 0 {
            panic!("failed to write header");
        }

        let mut frames = 0;
        let mut last_second = 0;

        // different from internal instant
        let record_instant = std::time::Instant::now();

        // attempt to get video frames
        loop {
            let video_buffer = self
                .video_api
                .video_rx
                .recv()
                .expect("failed to receive video bufer");

            frames += 1;

            let seconds_elapsed = std::time::Instant::now()
                .duration_since(record_instant)
                .as_secs();

            // test: stop after 15th frame
            if seconds_elapsed >= 16 {
                println!("stopping test recording");
                break;
            }

            if seconds_elapsed != last_second {
                println!("encoder: recording for {}s", seconds_elapsed);
                last_second = seconds_elapsed;
            }

            // create empty yuv avframe
            let mut yuv_frame = unsafe { ffmpeg::av_frame_alloc() };
            if yuv_frame.is_null() {
                panic!("Failed to allocate AVFrame!");
            }

            // fill required base avframe data
            unsafe {
                (*yuv_frame).format = ffmpeg::AV_PIX_FMT_YUV420P;
                (*yuv_frame).width = 1920;
                (*yuv_frame).height = 1080;
                (*yuv_frame).pts = frames;
            };

            // free if failed to allocate buffer data
            if unsafe { ffmpeg::av_frame_get_buffer(yuv_frame, 32) } < 0 {
                unsafe {
                    ffmpeg::av_frame_free(&mut yuv_frame);
                }
                panic!("could not allocate frame buffer!");
            }

            // converting a bgraframe into a yuvframe
            if unsafe {
                ffmpeg::sws_scale(
                    sws_context,
                    [video_buffer.bgra.as_ptr()].as_ptr(),
                    [1920 * 4].as_ptr(),
                    0,
                    1080,
                    (*yuv_frame).data.as_mut_ptr(),
                    (*yuv_frame).linesize.as_mut_ptr(),
                )
            } < 0
            {
                unsafe {
                    ffmpeg::av_frame_free(&mut yuv_frame);
                }
                panic!("could not allocate frame buffer!");
            }

            self.write_frame(video_encoder, output, stream, yuv_frame);

            unsafe {
                ffmpeg::av_frame_free(&mut yuv_frame);
            }
        }

        self.stop_recording();

        println!("encoder: attempting to write");
        unsafe {
            if output.is_null() {
                panic!("output is null");
            }

            // try to write to internal io
            if ffmpeg::av_write_trailer(output) > 0 {
                panic!("epic fail");
            };

            // close the internal io context inside output
            ffmpeg::avio_close((*output).pb);

            // free the encoders
            ffmpeg::avcodec_free_context(&mut video_encoder);
            ffmpeg::avcodec_free_context(&mut audio_encoder);

            // free its streams
            ffmpeg::avformat_free_context(output);

            // free the converter context
            ffmpeg::sws_freeContext(sws_context);
        }
        println!("encoder: success!");
    }

    fn write_frame(
        &mut self,
        encoder: *mut AVCodecContext,
        output: *mut AVFormatContext,
        stream: *mut AVStream,
        frame: *mut AVFrame,
    ) {
        let mut packet = unsafe { ffmpeg::av_packet_alloc() };
        if packet.is_null() {
            panic!("failed to allocate packet!");
        }

        let mut ret = unsafe { ffmpeg::avcodec_send_frame(encoder, frame) };
        if ret < 0 {
            panic!("dang")
        }

        while ret >= 0 {
            ret = unsafe { ffmpeg::avcodec_receive_packet(encoder, packet) };
            if ret == ffmpeg::AVERROR(ffmpeg::EAGAIN) || ret == ffmpeg::AVERROR_EOF {
                break;
            } else if ret < 0 {
                panic!("fuck");
            }

            unsafe {
                ffmpeg::av_packet_rescale_ts(packet, (*encoder).time_base, (*stream).time_base);
                (*packet).stream_index = (*stream).index;
            };

            if unsafe { ffmpeg::av_interleaved_write_frame(output, packet) } < 0 {
                panic!("write fail");
            }

            unsafe { ffmpeg::av_packet_unref(packet) };
        }

        unsafe {
            ffmpeg::av_packet_free(&mut packet);
        }
    }
}
