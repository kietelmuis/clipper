use rusty_ffmpeg::ffi::{
    self as ffmpeg, AV_CHANNEL_ORDER_UNSPEC, AV_CODEC_ID_AAC, AV_CODEC_ID_H264, AV_CODEC_ID_HEVC,
    AV_PIX_FMT_YUV420P, AV_SAMPLE_FMT_FLT, AV_SAMPLE_FMT_FLTP, AV_SAMPLE_FMT_U8, AVChannelLayout,
    AVChannelLayout__bindgen_ty_1, AVCodec, AVCodecContext, AVCodecID, AVFormatContext, AVFrame,
    AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO, AVPacket, AVRational, AVSampleFormat, AVStream,
    SwrContext, SwsContext, av_channel_layout_copy, av_channel_layout_default, av_frame_copy,
    av_frame_get_buffer, av_packet_alloc, avcodec_alloc_context3, avcodec_parameters_from_context,
    avformat_new_stream, swr_alloc, swr_get_out_samples,
};

use crossbeam::channel::{self, SendError};
use windows::Win32::UI::Shell::PAI_EXPIRETIME;

use std::{ffi::CString, ptr::NonNull, sync::Arc, thread::JoinHandle, time::Instant};

use crate::capture::{
    audio::{AudioBuffer, AudioCaptureApi},
    video::{VideoBuffer, VideoCaptureApi},
};

pub mod audio;
pub mod video;

pub struct CaptureSettings {
    pub resolution: [u32; 2],
    pub fps: u32,
}

// thanks https://ffmpeg.org/doxygen/trunk/mux_8c-example.html
pub struct MuxStream {
    stream: NonNull<AVStream>,
    encoder: NonNull<AVCodecContext>,

    // one can be null
    sws_context: Option<NonNull<SwsContext>>, // used for video
    swr_context: Option<NonNull<SwrContext>>, // used for audio
}

pub struct CaptureMuxer {
    // communication channels
    video_api: VideoCaptureApi,
    audio_api: AudioCaptureApi,

    audio_stream: Option<MuxStream>,
    video_stream: Option<MuxStream>,
    format_context: Option<NonNull<AVFormatContext>>,

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

            audio_stream: None,
            video_stream: None,
            format_context: None,
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

    pub fn init(&mut self) {
        // init all yo shi
        unsafe { ffmpeg::avdevice_register_all() };

        // todo: get from windows api
        let frame_rate = 75;
        let sample_rate = 48000;

        // choose them formats bruh
        let audio_format = AV_CODEC_ID_AAC;

        let sample_format_in = AV_SAMPLE_FMT_FLT;
        let sample_format_out = AV_SAMPLE_FMT_FLTP;

        let video_format = AV_CODEC_ID_H264;
        let pixel_format = AV_PIX_FMT_YUV420P;

        let video_timebase = ffmpeg::AVRational {
            num: 1,
            den: frame_rate,
        };
        let audio_timebase = ffmpeg::AVRational {
            num: 1,
            den: sample_rate,
        };

        // share avformatcontext for video and audio
        let mut format_context: *mut AVFormatContext = std::ptr::null_mut();
        let file_name = CString::new("video.mp4").expect("cstring fail");

        if unsafe {
            ffmpeg::avformat_alloc_output_context2(
                &mut format_context,
                std::ptr::null(),
                std::ptr::null(),
                file_name.as_ptr(),
            )
        } < 0
        {
            panic!("format ctx fail");
        }

        // open aviocontext within avformatcontext for writing
        if unsafe {
            ffmpeg::avio_open(
                &mut (*format_context).pb,
                file_name.as_ptr(),
                2, // cooked
            )
        } < 0
        {
            panic!("failed to open avio for writing");
        }

        self.format_context = NonNull::new(format_context);

        // create video and audio muxstreams
        self.video_stream = Some(self.create_video_muxstream(
            format_context,
            video_format,
            pixel_format,
            video_timebase,
        ));

        self.audio_stream = Some(self.create_audio_muxstream(
            format_context,
            audio_format,
            sample_format_out,
            sample_rate,
            audio_timebase,
        ));

        self.encode_frames(
            format_context,
            sample_format_in,
            sample_format_out,
            sample_rate,
        );
    }

    // creates the audio muxstream
    // with specific settings, adjust later
    fn create_audio_muxstream(
        &mut self,
        format_context: *mut AVFormatContext,
        codec_id: i32,
        sample_format: i32,
        sample_rate: i32,
        timebase: AVRational,
    ) -> MuxStream {
        let codec = unsafe { ffmpeg::avcodec_find_encoder(codec_id) };
        if codec.is_null() {
            panic!("failed to find codec");
        }

        // alloc encoder
        let mut encoder = NonNull::new(unsafe { avcodec_alloc_context3(codec) })
            .expect("failed to create encoder");

        // make a boilerplate channel layout to apply defualt later
        let mut channel_layout = AVChannelLayout {
            order: AV_CHANNEL_ORDER_UNSPEC,
            nb_channels: 0,
            opaque: std::ptr::null_mut(),
            u: AVChannelLayout__bindgen_ty_1 { mask: 0 },
        };

        // get encoder pointer to adjust settings
        let encoder_ptr = unsafe { encoder.as_mut() };

        // set default layout and adjust settings
        unsafe {
            av_channel_layout_default(&mut channel_layout, 2);
            (*encoder_ptr).ch_layout = channel_layout;
            (*encoder_ptr).sample_fmt = sample_format;
            (*encoder_ptr).sample_rate = sample_rate;
            (*encoder_ptr).codec_id = codec_id;
            (*encoder_ptr).codec_type = AVMEDIA_TYPE_AUDIO;
            (*encoder_ptr).time_base = timebase;
        }

        // open the encoder
        if unsafe { ffmpeg::avcodec_open2(encoder_ptr, codec, std::ptr::null_mut()) } < 0 {
            panic!("failed to open encoder");
        }

        // create and set new avstream
        let mut stream =
            NonNull::new(unsafe { avformat_new_stream(format_context, std::ptr::null()) })
                .expect("failed to create stream");

        // set stream id as format context stream index
        if unsafe { avcodec_parameters_from_context((*stream.as_mut()).codecpar, encoder.as_ptr()) }
            < 0
        {
            panic!("could not set encoder parameters");
        }

        MuxStream {
            stream,
            encoder,
            sws_context: None,
            swr_context: None,
        }
    }

    // creates the video muxstream
    // with specific settings, adjust later
    fn create_video_muxstream(
        &mut self,
        format_context: *mut AVFormatContext,
        codec_id: AVCodecID,
        pixel_format: i32,
        timebase: AVRational,
    ) -> MuxStream {
        let codec = unsafe { ffmpeg::avcodec_find_encoder(codec_id) };
        if codec.is_null() {
            panic!("failed to find codec");
        }

        // alloc encoder
        let mut encoder = NonNull::new(unsafe { avcodec_alloc_context3(codec) })
            .expect("failed to allocate encoder");

        // get encoder pointer to adjust settings
        let encoder_ptr = unsafe { encoder.as_mut() };

        // adjust settings
        (*encoder_ptr).width = 1920;
        (*encoder_ptr).height = 1080;
        (*encoder_ptr).pix_fmt = pixel_format;
        (*encoder_ptr).time_base = timebase;
        (*encoder_ptr).bit_rate = 9_000_000;
        (*encoder_ptr).codec_id = codec_id;
        (*encoder_ptr).codec_type = AVMEDIA_TYPE_VIDEO;

        // open the encoder
        if unsafe { ffmpeg::avcodec_open2(encoder_ptr, codec, std::ptr::null_mut()) } < 0 {
            panic!("failed to open encoder");
        }

        // create and set new avstream
        let mut stream =
            NonNull::new(unsafe { avformat_new_stream(format_context, std::ptr::null()) })
                .expect("failed to create stream");

        // set stream id as format context stream index
        if unsafe { avcodec_parameters_from_context((*stream.as_mut()).codecpar, encoder.as_ptr()) }
            < 0
        {
            panic!("could not set encoder parameters");
        }

        MuxStream {
            stream,
            encoder,
            sws_context: None,
            swr_context: None,
        }
    }

    fn process_video_frame(&mut self, frames: i64, video_buffer: VideoBuffer) {
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
                self.video_stream
                    .as_ref()
                    .unwrap()
                    .sws_context
                    .unwrap()
                    .as_ptr(),
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

        self.write_frame(yuv_frame);

        unsafe {
            ffmpeg::av_frame_free(&mut yuv_frame);
        }
    }

    fn process_audio_frame(
        &mut self,
        sample_format_in: AVSampleFormat,
        sample_format_out: AVSampleFormat,
        sample_rate: i32,
        frames: i64,
        audio_buffer: AudioBuffer,
    ) {
        // allocate output frame
        let output_frame = unsafe { ffmpeg::av_frame_alloc() };
        if output_frame.is_null() {
            panic!("Failed to allocate AVFrame!");
        }

        let total_bytes = audio_buffer.buffer.len() as i32;
        let channels = unsafe {
            &(*self.audio_stream.as_ref().unwrap().encoder.as_ptr())
                .ch_layout
                .nb_channels
        }
        .clone();
        assert_eq!(
            total_bytes % channels,
            0,
            "Buffer length {} is not a multiple of channel count {}",
            total_bytes,
            channels
        );
        let sample_amount = (total_bytes / channels) as i32;

        let max_out = unsafe {
            swr_get_out_samples(
                self.audio_stream
                    .as_mut()
                    .unwrap()
                    .swr_context
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                sample_amount,
            )
        } as i32;

        // set its options
        unsafe {
            av_channel_layout_copy(
                &mut (*output_frame).ch_layout,
                &(*self.audio_stream.as_ref().unwrap().encoder.as_ptr()).ch_layout,
            );
            (*output_frame).format = sample_format_out;
            (*output_frame).sample_rate = sample_rate;
            (*output_frame).nb_samples = max_out;
            (*output_frame).pts = frames;
        }

        // allocate output buffer
        if unsafe { av_frame_get_buffer(output_frame, 0) } < 0 {
            panic!("failed to allocate frame data buffers");
        }

        // allocate input frame
        let input_frame = unsafe { ffmpeg::av_frame_alloc() };
        if input_frame.is_null() {
            panic!("Failed to allocate AVFrame!");
        }

        // copy and set format option
        unsafe {
            av_channel_layout_copy(
                &mut (*input_frame).ch_layout,
                &(*self.audio_stream.as_ref().unwrap().encoder.as_ptr()).ch_layout,
            );
            (*input_frame).format = sample_format_in;
            (*input_frame).sample_rate = sample_rate;
            (*input_frame).nb_samples = sample_amount;
            (*input_frame).pts = frames;
        };

        // allocate input buffer
        if unsafe { av_frame_get_buffer(input_frame, 0) } < 0 {
            panic!("failed to allocate frame data buffers");
        }

        // write
        unsafe {
            let data_ptr = (*input_frame).data[0] as *mut f32;
            let dst = std::slice::from_raw_parts_mut(data_ptr, (sample_amount * channels) as usize);
            dst.copy_from_slice(&audio_buffer.buffer);
        }

        // convert the input format to output format
        if unsafe {
            ffmpeg::swr_convert_frame(
                self.audio_stream
                    .as_mut()
                    .unwrap()
                    .swr_context
                    .unwrap()
                    .as_mut(),
                output_frame,
                input_frame,
            )
        } < 0
        {
            panic!("fuck");
        }

        self.write_audio(output_frame);
    }

    fn create_sws(&mut self) {
        let sws_context = NonNull::new(unsafe {
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
        });

        // check if context created
        if sws_context.is_none() {
            panic!("failed to allocate and init sws context");
        }

        // set the context
        self.video_stream
            .as_mut()
            .expect("video stream not initiliazed!")
            .sws_context = sws_context;
    }

    fn create_swr(
        &mut self,
        sample_format_in: AVSampleFormat,
        sample_format_out: AVSampleFormat,
        sample_rate: i32,
    ) {
        let channel_layout =
            &((unsafe { *self.audio_stream.as_ref().unwrap().encoder.as_ptr() }).ch_layout);

        let mut swr_context = std::ptr::null_mut::<SwrContext>();
        if unsafe {
            ffmpeg::swr_alloc_set_opts2(
                &mut swr_context,
                channel_layout,
                sample_format_out,
                sample_rate,
                channel_layout,
                sample_format_in,
                sample_rate,
                0,
                std::ptr::null_mut(),
            )
        } < 0
        {
            panic!("swr context is fucked");
        };

        // Initialize the context - THIS IS THE CRITICAL MISSING LINE
        if unsafe { ffmpeg::swr_init(swr_context) } < 0 {
            panic!("Failed to initialize swr context");
        }

        // set the context
        self.audio_stream
            .as_mut()
            .expect("audio stream not initiliazed!")
            .swr_context = NonNull::new(swr_context);
    }

    fn encode_frames(
        &mut self,
        format_context: *mut AVFormatContext,
        sample_format_in: AVSampleFormat,
        sample_format_out: AVSampleFormat,
        sample_rate: i32,
    ) {
        // init sws context
        self.create_sws();
        self.create_swr(sample_format_in, sample_format_out, sample_rate);

        // write header for video
        if unsafe { ffmpeg::avformat_write_header(format_context, std::ptr::null_mut()) } < 0 {
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

            let audio_buffer = self
                .audio_api
                .audio_rx
                .recv()
                .expect("failed to receive audio bufer");

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
                println!("[encoder] recording for {}s", seconds_elapsed);
                last_second = seconds_elapsed;
            }

            // process av
            self.process_audio_frame(
                sample_format_in,
                sample_format_out,
                sample_rate,
                frames,
                audio_buffer,
            );
            self.process_video_frame(frames, video_buffer);
        }

        self.stop_recording();

        println!("[encoder] attempting to write");
        unsafe {
            if format_context.is_null() {
                panic!("output is null");
            }

            // try to write to internal io
            if ffmpeg::av_write_trailer(format_context) > 0 {
                panic!("epic fail");
            };

            // close the internal io context inside output
            ffmpeg::avio_close((*format_context).pb);

            // free the encoders
            ffmpeg::avcodec_free_context(&mut self.video_stream.as_mut().unwrap().encoder.as_ptr());
            ffmpeg::avcodec_free_context(&mut self.audio_stream.as_mut().unwrap().encoder.as_ptr());

            // free its streams
            ffmpeg::avformat_free_context(format_context);

            // free the converter context
            ffmpeg::sws_freeContext(
                self.video_stream
                    .as_ref()
                    .unwrap()
                    .sws_context
                    .unwrap()
                    .as_ptr(),
            );
        }
        println!("encoder: success!");
    }

    fn write_frame(&mut self, frame: *mut AVFrame) {
        let mut packet = unsafe { ffmpeg::av_packet_alloc() };
        if packet.is_null() {
            panic!("failed to allocate packet!");
        }

        let encoder = unsafe { self.video_stream.as_mut().unwrap().encoder.as_mut() };
        let stream = unsafe { self.video_stream.as_mut().unwrap().stream.as_ref() };
        let format_context = unsafe { self.format_context.unwrap().as_mut() };

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

            if unsafe { ffmpeg::av_interleaved_write_frame(format_context, packet) } < 0 {
                panic!("write fail");
            }

            unsafe { ffmpeg::av_packet_unref(packet) };
        }

        unsafe {
            ffmpeg::av_packet_free(&mut packet);
        }
    }

    fn write_audio(&mut self, frame: *mut AVFrame) {
        // Use your original encoding/writing logic exactly as it was
        let mut packet = unsafe { ffmpeg::av_packet_alloc() };
        if packet.is_null() {
            panic!("failed to allocate packet!");
        }

        let encoder = unsafe { self.audio_stream.as_mut().unwrap().encoder.as_mut() };
        let stream = unsafe { self.audio_stream.as_mut().unwrap().stream.as_ref() };
        let format_context = unsafe { self.format_context.unwrap().as_mut() };

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

            if unsafe { ffmpeg::av_interleaved_write_frame(format_context, packet) } < 0 {
                panic!("write fail");
            }

            unsafe { ffmpeg::av_packet_unref(packet) };
        }

        unsafe {
            ffmpeg::av_packet_free(&mut packet);
        }
    }
}
