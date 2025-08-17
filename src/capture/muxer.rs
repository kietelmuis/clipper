use crossbeam::channel::{Receiver, TryRecvError};
use rusty_ffmpeg::ffi::{
    self as ffmpeg, AV_CHANNEL_ORDER_UNSPEC, AV_CODEC_ID_AAC, AV_CODEC_ID_H264, AV_PIX_FMT_YUV420P,
    AV_SAMPLE_FMT_FLT, AV_SAMPLE_FMT_FLTP, AVChannelLayout, AVChannelLayout__bindgen_ty_1,
    AVCodecContext, AVCodecID, AVFormatContext, AVFrame, AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO,
    AVPacket, AVRational, AVSampleFormat, AVStream, SwrContext, SwsContext, av_channel_layout_copy,
    av_channel_layout_default, av_frame_get_buffer, av_opt_set, avcodec_alloc_context3,
    avcodec_parameters_from_context, avformat_new_stream, swr_get_out_samples,
};

use std::{
    ffi::CString,
    ptr::NonNull,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::capture::{
    audio::{AudioBuffer, AudioCaptureApi},
    video::{VideoBuffer, VideoCaptureApi},
};

use super::replay::ReplayBuffer;

// ik ben genius ik weet
pub enum MuxerCommand {
    Clip,
}

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

impl Drop for MuxStream {
    fn drop(&mut self) {
        unsafe {
            // free contexts if exist
            if let Some(mut sws) = self.sws_context {
                ffmpeg::sws_freeContext(sws.as_mut());
            }

            if let Some(swr) = self.swr_context {
                ffmpeg::swr_free(&mut swr.as_ptr());
            }

            let mut encoder_ptr = self.encoder.as_ptr();
            ffmpeg::avcodec_free_context(&mut encoder_ptr);
        }
    }
}

impl MuxStream {
    fn flush_stream(&mut self, format_context: *mut AVFormatContext) {
        unsafe {
            let encoder = self.encoder.as_ptr();
            let stream = self.stream.as_ptr();

            // send a null frame to signal EOF
            if ffmpeg::avcodec_send_frame(encoder, std::ptr::null_mut()) < 0 {
                eprintln!("[encoder] failed to send null frame for flush");
                return;
            }

            let mut packet: AVPacket = std::mem::zeroed();
            ffmpeg::av_init_packet(&mut packet);

            // read all delayed packets
            loop {
                let ret = ffmpeg::avcodec_receive_packet(encoder, &mut packet);
                if ret == ffmpeg::AVERROR_EOF || ret == ffmpeg::AVERROR(ffmpeg::EAGAIN) {
                    break;
                } else if ret < 0 {
                    eprintln!("[encoder] error while flushing encoder: {}", ret);
                    break;
                }

                // rescale time from encoder to stream
                ffmpeg::av_packet_rescale_ts(
                    &mut packet,
                    (*encoder).time_base,
                    (*stream).time_base,
                );
                packet.stream_index = (*stream).index;

                // write packet to output
                if ffmpeg::av_interleaved_write_frame(format_context, &mut packet) < 0 {
                    eprintln!("[encoder] error writing packet during flush");
                }

                ffmpeg::av_packet_unref(&mut packet);
            }
        }
    }
}

pub struct CaptureMuxer {
    // communication channels
    video_api: VideoCaptureApi,
    audio_api: AudioCaptureApi,

    // replay buffer
    replay_buffer: ReplayBuffer,

    // data structs for audio/video
    audio_stream: Option<MuxStream>,
    video_stream: Option<MuxStream>,
    format_context: Option<NonNull<AVFormatContext>>,

    instant: Arc<Instant>,
}

impl Drop for CaptureMuxer {
    fn drop(&mut self) {
        unsafe {
            println!("cleaning capturemuxer for drop");

            let format_context = self.format_context.unwrap().as_ptr();

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
    }
}

const SAMPLE_FORMAT_IN: AVSampleFormat = AV_SAMPLE_FMT_FLT;
const SAMPLE_FORMAT_OUT: AVSampleFormat = AV_SAMPLE_FMT_FLTP;

const SAMPLE_RATE: i32 = 48000;

impl CaptureMuxer {
    pub fn new(_settings: CaptureSettings) -> Self {
        let instant = Arc::new(Instant::now());

        let video_api = VideoCaptureApi::new(instant.clone());
        let audio_api = AudioCaptureApi::new(instant.clone());

        Self {
            video_api,
            audio_api,
            instant,

            replay_buffer: ReplayBuffer::new(Duration::from_secs(3)),

            audio_stream: None,
            video_stream: None,
            format_context: None,
        }
    }

    pub fn write_clip(&mut self) {
        let format_context = self.format_context.unwrap().as_ptr();

        println!("[encoder] getting frames from replay buffer");

        let frames = self.replay_buffer.get_frames();

        println!(
            "[encoder] writing {} frames from replay buffer",
            frames.len()
        );

        // loop over packet pointers and write them to context
        for &packet in frames.iter() {
            unsafe {
                if ffmpeg::av_interleaved_write_frame(format_context, packet) != 0 {
                    eprintln!("error writing frame from replay buffer");
                }
            }
        }

        println!("[encoder] flushing packet streams");
        self.video_stream
            .as_mut()
            .unwrap()
            .flush_stream(format_context);
        self.audio_stream
            .as_mut()
            .unwrap()
            .flush_stream(format_context);

        println!("[encoder] attempting to write trailer");
        unsafe {
            if format_context.is_null() {
                panic!("output context is null");
            }

            // try to write video with internal io
            if ffmpeg::av_write_trailer(format_context) != 0 {
                panic!("write fail");
            };
        }
        println!("[encoder]: success!");
    }

    pub fn init(&mut self) {
        // init all yo shi
        unsafe { ffmpeg::avdevice_register_all() };

        // todo: get from api
        let frame_rate = 30;

        // choose them formats bruh
        let audio_format = AV_CODEC_ID_AAC;

        let video_format = AV_CODEC_ID_H264;
        let pixel_format = AV_PIX_FMT_YUV420P;

        let video_timebase = ffmpeg::AVRational {
            num: 1,
            den: frame_rate,
        };
        let audio_timebase = ffmpeg::AVRational {
            num: 1,
            den: SAMPLE_RATE,
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

        self.audio_stream =
            Some(self.create_audio_muxstream(format_context, audio_format as i32, audio_timebase));
    }

    // creates the audio muxstream
    // with specific settings, adjust later
    fn create_audio_muxstream(
        &mut self,
        format_context: *mut AVFormatContext,
        codec_id: i32,
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
            (*encoder_ptr).sample_fmt = SAMPLE_FORMAT_OUT;
            (*encoder_ptr).sample_rate = SAMPLE_RATE;
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

        if unsafe { avcodec_parameters_from_context((*stream.as_mut()).codecpar, encoder.as_ptr()) }
            < 0
        {
            panic!("could not set encoder parameters");
        }

        // ensure stream time_base matches encoder time_base
        unsafe {
            (*stream.as_mut()).time_base = (*encoder_ptr).time_base;
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
        (*encoder_ptr).codec_id = codec_id;
        (*encoder_ptr).codec_type = AVMEDIA_TYPE_VIDEO;

        // h264 all-i options
        // to optimize memory usage
        (*encoder_ptr).gop_size = 30;
        (*encoder_ptr).max_b_frames = 2;
        (*encoder_ptr).bit_rate = 10 * 1000 * 1000; // 50 mbps

        // rate control
        (*encoder_ptr).rc_max_rate = (*encoder_ptr).bit_rate;
        (*encoder_ptr).rc_buffer_size = (*encoder_ptr).bit_rate as i32;

        unsafe {
            av_opt_set(
                encoder.as_ptr() as *mut _,
                c"preset".as_ptr() as *const _,
                c"ultrafast".as_ptr() as *const _,
                0,
            );
            av_opt_set(
                encoder.as_ptr() as *mut _,
                c"tune".as_ptr() as *const _,
                c"zerolatency".as_ptr() as *const _,
                0,
            );
            av_opt_set(
                encoder.as_ptr() as *mut _,
                c"crf".as_ptr() as *const _,
                c"23".as_ptr() as *const _,
                0,
            );
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

        // ensure stream time_base matches encoder time_base
        unsafe {
            (*stream.as_mut()).time_base = (*encoder_ptr).time_base;
        }

        MuxStream {
            stream,
            encoder,
            sws_context: None,
            swr_context: None,
        }
    }

    fn process_video_frame(&mut self, video_buffer: VideoBuffer, frame_index: i64) {
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
            (*yuv_frame).pts = frame_index;
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

        self.encode_frame(yuv_frame);

        unsafe {
            ffmpeg::av_frame_free(&mut yuv_frame);
        }
    }

    fn process_audio_frame(
        &mut self,
        sample_format_in: AVSampleFormat,
        sample_format_out: AVSampleFormat,
        sample_rate: i32,
        audio_buffer: AudioBuffer,
        frame_index: i64,
    ) {
        // allocate output frame
        let mut output_frame = unsafe { ffmpeg::av_frame_alloc() };
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
            (*output_frame).pts = frame_index;
        }

        // allocate output buffer
        if unsafe { av_frame_get_buffer(output_frame, 0) } < 0 {
            panic!("failed to allocate frame data buffers");
        }

        // allocate input frame
        let mut input_frame = unsafe { ffmpeg::av_frame_alloc() };
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
            (*input_frame).pts = frame_index;
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
            panic!("audio convert failed");
        }

        self.encode_audio(output_frame);

        unsafe {
            ffmpeg::av_frame_free(&mut input_frame);
            ffmpeg::av_frame_free(&mut output_frame);
        }
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

    pub fn start(&mut self, rx: Receiver<MuxerCommand>) {
        // init sws context
        self.create_sws();
        self.create_swr(SAMPLE_FORMAT_IN, SAMPLE_FORMAT_OUT, SAMPLE_RATE);

        let format_context = self.format_context.unwrap().as_ptr();

        // write header for video
        if unsafe { ffmpeg::avformat_write_header(format_context, std::ptr::null_mut()) } < 0 {
            panic!("failed to write header");
        }

        // different from internal instant
        // since it starts at ffmpeg ready
        let start_time = Instant::now();
        let mut last_print = Instant::now();

        let mut frame_index = 1;

        // attempt to get video frames
        loop {
            let elapsed = start_time.elapsed();
            let seconds_elapsed = elapsed.as_secs();

            // check for commands
            match rx.try_recv() {
                Ok(cmd) => match cmd {
                    MuxerCommand::Clip => self.write_clip(),
                },
                Err(TryRecvError::Empty) => (),
                Err(_) => {
                    eprintln!("Command channel disconnected")
                }
            }

            // print status every second
            if last_print.elapsed() >= Duration::from_secs(1) {
                println!(
                    "[encoder] recording for {}s (using {} ram)",
                    seconds_elapsed,
                    crate::util::humanize_bytes(self.replay_buffer.bytes as u64)
                );

                last_print = std::time::Instant::now();
            }

            match self.video_api.video_rx.try_recv() {
                Ok(buf) => {
                    self.process_video_frame(buf, frame_index);
                    frame_index += 1;
                }
                Err(TryRecvError::Empty) => (),
                Err(_) => {
                    eprintln!("Video channel disconnected");
                    break;
                }
            };

            match self.audio_api.audio_rx.try_recv() {
                Ok(buf) => {
                    self.process_audio_frame(
                        SAMPLE_FORMAT_IN,
                        SAMPLE_FORMAT_OUT,
                        SAMPLE_RATE,
                        buf,
                        frame_index,
                    );
                }
                Err(TryRecvError::Empty) => (),
                Err(_) => {
                    eprintln!("Audio channel disconnected");
                    break;
                }
            };
        }
    }

    fn encode_frame(&mut self, frame: *mut AVFrame) {
        let encoder = unsafe { self.video_stream.as_mut().unwrap().encoder.as_mut() };

        // send frame to encoder
        let ret = unsafe { ffmpeg::avcodec_send_frame(encoder, frame) };
        if ret < 0 {
            panic!("failed to send frame to encoder")
        }

        // receive packets from encoder
        loop {
            let mut packet = unsafe { ffmpeg::av_packet_alloc() };
            if packet.is_null() {
                panic!("Failed to allocate AVPacket");
            }

            let ret = unsafe { ffmpeg::avcodec_receive_packet(encoder, packet) };

            if ret == ffmpeg::AVERROR(ffmpeg::EAGAIN) || ret == ffmpeg::AVERROR_EOF {
                unsafe {
                    ffmpeg::av_packet_free(&mut packet);
                }
                break;
            } else if ret < 0 {
                unsafe {
                    ffmpeg::av_packet_free(&mut packet);
                }
                eprintln!("[video] avcodec_receive_packet error={}", ret);
                break;
            }

            // add packet to replay buffer (buffer takes ownership)
            self.replay_buffer.add_frame(packet);
        }
    }

    fn encode_audio(&mut self, frame: *mut AVFrame) {
        let encoder = unsafe { self.audio_stream.as_mut().unwrap().encoder.as_mut() };

        // send frame to encoder
        let ret = unsafe { ffmpeg::avcodec_send_frame(encoder, frame) };
        if ret < 0 {
            panic!("failed to send frame to encoder")
        }

        // receive packets from encoder
        loop {
            let mut packet = unsafe { ffmpeg::av_packet_alloc() };
            if packet.is_null() {
                panic!("Failed to allocate AVPacket");
            }

            let ret = unsafe { ffmpeg::avcodec_receive_packet(encoder, packet) };

            if ret == ffmpeg::AVERROR(ffmpeg::EAGAIN) || ret == ffmpeg::AVERROR_EOF {
                unsafe {
                    ffmpeg::av_packet_free(&mut packet);
                }
                break;
            } else if ret < 0 {
                unsafe {
                    ffmpeg::av_packet_free(&mut packet);
                }
                eprintln!("[video] avcodec_receive_packet error={}", ret);
                break;
            }

            // add packet to replay buffer (buffer takes ownership)
            self.replay_buffer.add_frame(packet);
        }
    }
}
