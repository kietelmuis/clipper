use rusty_ffmpeg::ffi::{
    self as ffmpeg, AV_CODEC_ID_H264, AV_PIX_FMT_YUV420P, AVCodecContext, AVFormatContext, AVFrame,
    AVMEDIA_TYPE_VIDEO, AVStream,
};

use crossbeam::channel;

use std::{ffi::CString, sync::Arc, time::Instant};

pub mod audio;
pub mod video;

pub struct CaptureSettings {
    pub resolution: [u32; 2],
    pub fps: u32,
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

        let instant = Arc::new(Instant::now());

        let mut muxer = Self {
            stop_video: video::VideoCaptureApi::new(instant.clone(), video_tx),
            stop_audio: audio::AudioCaptureApi::new(instant.clone(), audio_tx),

            recv_video: video_rx,
            recv_audio: audio_rx,

            instant: instant, // move into self
        };

        muxer.start_muxer();
        muxer
    }

    // ffmpeg
    // result < 0 == error
    fn start_muxer(&mut self) {
        // init all yo shi
        unsafe { ffmpeg::avdevice_register_all() };

        let codec_format = AV_CODEC_ID_H264;
        let pixel_format = AV_PIX_FMT_YUV420P;

        let codec = unsafe { ffmpeg::avcodec_find_encoder(codec_format) };
        if codec.is_null() {
            panic!("failed to find h264 encoder");
        }

        let mut codec_options: *mut ffmpeg::AVDictionary = std::ptr::null_mut();
        unsafe {
            ffmpeg::av_dict_set(
                &mut codec_options,
                b"preset\0".as_ptr() as *const _,
                b"veryfast\0".as_ptr() as *const _,
                0,
            );
            ffmpeg::av_dict_set(
                &mut codec_options,
                b"crf\0".as_ptr() as *const _,
                b"23\0".as_ptr() as *const _,
                0,
            );
            ffmpeg::av_dict_set(
                &mut codec_options,
                b"bitrate\0".as_ptr() as *const _,
                b"5000000\0".as_ptr() as *const _,
                0,
            );
        };

        // allocate encoder
        let encoder = unsafe { ffmpeg::avcodec_alloc_context3(codec) };
        if encoder.is_null() {
            panic!("failed to allocate codec context");
        }

        // configure encoder
        unsafe {
            (*encoder).width = 1920;
            (*encoder).height = 1080;
            (*encoder).pix_fmt = pixel_format;
            (*encoder).time_base = ffmpeg::AVRational { num: 1, den: 75 }; // for my 75hz

            (*encoder).bit_rate = 5_000_000;
            (*encoder).rc_max_rate = 5_000_000;
            (*encoder).rc_min_rate = 5_000_000;
            (*encoder).rc_buffer_size = 10_000_000;
        }

        if unsafe { ffmpeg::avcodec_open2(encoder, codec, &mut codec_options) } < 0 {
            panic!("Failed to open encoder");
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

        self.encode_frames(encoder, output, stream);
    }

    fn encode_frames(
        &mut self,
        mut encoder: *mut AVCodecContext,
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

        let start = std::time::Instant::now();

        // attempt to get video frames
        loop {
            let video_buffer = self
                .recv_video
                .recv()
                .expect("failed to receive video bufer");

            frames += 1;

            let seconds_elapsed = std::time::Instant::now().duration_since(start).as_secs();
            if seconds_elapsed != last_second {
                println!("encoder: recording for {}s", seconds_elapsed);
                last_second = seconds_elapsed;
            }

            // test: 15 second recording
            if seconds_elapsed >= 15 {
                println!("stopping test recording");
                break;
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

            self.write_frame(encoder, output, stream, yuv_frame);

            unsafe {
                ffmpeg::av_frame_free(&mut yuv_frame);
            }
        }

        self.stop_video.send(()).expect("failed to stop video");

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

            // free the encoder
            ffmpeg::avcodec_free_context(&mut encoder);

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
