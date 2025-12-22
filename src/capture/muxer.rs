use crossbeam::channel::Receiver;

use ffmpeg::util::rational::Rational;
use ffmpeg_next::{
    self as ffmpeg, ChannelLayout, Codec,
    codec::Id,
    encoder,
    format::{Sample, sample::Type},
    frame,
    packet::packet,
    software::{
        resampling,
        scaling::{self, Flags},
    },
    util::format::Pixel,
};

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::capture::{
    audio::{AudioBuffer, AudioCaptureApi},
    video::{Resolution, VideoBuffer, VideoCaptureApi},
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

pub struct CaptureMuxer {
    // communication channels
    video_api: VideoCaptureApi,
    audio_api: AudioCaptureApi,

    // sub structs
    replay_buffer: ReplayBuffer,

    // data structs for audio/video
    audio_encoder: Option<encoder::Audio>,
    video_encoder: Option<encoder::Video>,

    sws: Option<scaling::Context>,
    swr: Option<resampling::Context>,

    target_resolution: [u32; 2],
    channel_layout: Option<ChannelLayout>,
    audio_channels: Option<usize>,

    instant: Arc<Instant>,

    video_pts: i64,
    audio_pts: i64,
}

const SAMPLE_FORMAT_IN: Sample = Sample::F32(Type::Packed);
const SAMPLE_FORMAT_OUT: Sample = Sample::F32(Type::Planar);

const FRAME_RATE: i32 = 30;

impl CaptureMuxer {
    pub async fn new(_settings: CaptureSettings) -> Self {
        let instant = Arc::new(Instant::now());

        let video_api = VideoCaptureApi::new(instant.clone()).await;
        let audio_api = AudioCaptureApi::new(instant.clone());

        Self {
            video_api,
            audio_api,
            instant,

            target_resolution: _settings.resolution,
            replay_buffer: ReplayBuffer::new(Duration::from_secs(3)),

            swr: None,
            sws: None,

            channel_layout: None,
            audio_channels: None,

            audio_encoder: None,
            video_encoder: None,

            video_pts: 0,
            audio_pts: 0,
        }
    }

    /*pub fn write_clip(&mut self) {
        let file_name = CString::new("clip.mp4").expect("cstring fail");
        let mut format_context: *mut AVFormatContext = std::ptr::null_mut();

        // share avformatcontext for video and audio
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

        let movflag = CString::new("movflags").unwrap();
        let fastflag = CString::new("faststart").unwrap();

        if unsafe {
            ffmpeg::av_opt_set(
                (*format_context).priv_data,
                movflag.as_ptr(),
                fastflag.as_ptr(),
                0,
            )
        } < 0
        {
            eprintln!("[encoder] warning: could not set movflags");
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
            unsafe { ffmpeg::avformat_free_context(format_context) };
            panic!("failed to open avio for writing");
        }

        let packets = self.replay_buffer.get_frames();

        for (i, &pkt) in packets.iter().enumerate() {
            unsafe {
                println!(
                    "[debug] pkt {} size={} flags=0x{:X} pts={} dts={}",
                    i,
                    (*pkt).size,
                    (*pkt).flags,
                    (*pkt).pts,
                    (*pkt).dts
                );
            }
        }

        println!(
            "[encoder] writing {} frames from replay buffer",
            packets.len()
        );

        self.create_stream(format_context, unsafe {
            self.video_encoder.as_ref().unwrap().encoder.as_ref()
        });
        self.create_stream(format_context, unsafe {
            self.audio_encoder.as_ref().unwrap().encoder.as_ref()
        });

        // write file header
        if unsafe { ffmpeg::avformat_write_header(format_context, std::ptr::null_mut()) } < 0 {
            unsafe {
                ffmpeg::avio_close((*format_context).pb);
                ffmpeg::avformat_free_context(format_context);
            }
            panic!("failed to write header");
        }

        // loop over packet pointers and write them to context
        for &og_packet in packets.iter() {
            unsafe {
                // reference the packet so it doesnt get freed yet
                let mut write_packet = ffmpeg::av_packet_alloc();
                if write_packet.is_null() {
                    eprintln!("failed to allocate write packet");
                    continue;
                }

                if ffmpeg::av_packet_ref(write_packet, og_packet) < 0 {
                    eprintln!("failed to ref packet");
                    ffmpeg::av_packet_free(&mut write_packet);
                    continue;
                }

                if ffmpeg::av_interleaved_write_frame(format_context, write_packet) != 0 {
                    eprintln!("error writing frame from replay buffer");
                }

                // free it
                ffmpeg::av_packet_free(&mut write_packet);
            }
        }

        println!("[encoder] flushing packet streams");
        self.video_encoder
            .as_mut()
            .unwrap()
            .flush_stream(format_context);
        self.audio_encoder
            .as_mut()
            .unwrap()
            .flush_stream(format_context);

        println!("[encoder] attempting to write clip");
        unsafe {
            if format_context.is_null() {
                panic!("output context is null");
            }

            // try to write video with internal io
            if ffmpeg::av_write_trailer(format_context) != 0 {
                panic!("clip failed to write");
            };

            // clean up resources for clip
            ffmpeg::avio_close((*format_context).pb);
            ffmpeg::avformat_free_context(format_context);
        }
        println!("[encoder] success!");
    }*/

    pub fn init(&mut self) {
        ffmpeg::init().unwrap();

        // find codecs
        let video_codec = ffmpeg::encoder::find(Id::HEVC).expect("could not find video codec");
        let audio_codec = ffmpeg::encoder::find(Id::AAC).expect("could not find audio codec");

        // create video encoder
        let mut video_enc = ffmpeg::encoder::new()
            .video()
            .expect("failed to create video encoder");

        let resolution = self
            .video_api
            .resolution
            .as_ref()
            .expect("could not find resolution");

        let video_timebase = Rational::new(1, FRAME_RATE);
        video_enc.set_width(resolution.width as u32);
        video_enc.set_height(resolution.height as u32);
        video_enc.set_format(Pixel::YUV420P);
        video_enc.set_time_base(video_timebase);
        video_enc.set_gop(1); // no b-frames
        video_enc.set_max_b_frames(0);
        video_enc.set_bit_rate(10_000_000); // 10 mbps
        video_enc.set_max_bit_rate(10_000_000);

        let video_enc = video_enc
            .open_as(video_codec)
            .expect("failed to open video encoder");

        // create audio encoder
        let mut audio_enc = ffmpeg::encoder::new()
            .audio()
            .expect("failed to create audio encoder");

        let api_sample_rate = self
            .audio_api
            .sample_rate
            .expect("failed to get audio sample rate");

        let api_bit_rate = self
            .audio_api
            .bit_rate
            .expect("failed to get audio bit rate");

        let api_channels = self
            .audio_api
            .channels
            .expect("failed to get audio channels");

        let channel_layout = ChannelLayout::default(api_channels);

        audio_enc.set_channel_layout(channel_layout);
        audio_enc.set_format(SAMPLE_FORMAT_OUT);
        audio_enc.set_time_base(Rational::new(1, api_sample_rate));
        audio_enc.set_rate(api_sample_rate);
        audio_enc.set_bit_rate(api_bit_rate);

        let audio_enc = audio_enc
            .open_as(audio_codec)
            .expect("failed to open audio encoder");

        let api_resolution = self
            .video_api
            .resolution
            .expect("failed to get audio bit rate");

        // create video and audio converters
        let sws_input = (api_resolution.width as u32, api_resolution.height as u32);
        let sws_output = (self.target_resolution[0], self.target_resolution[1]);

        let swr_input = (SAMPLE_FORMAT_IN, channel_layout, api_sample_rate as u32);
        let swr_output = (SAMPLE_FORMAT_OUT, channel_layout, api_sample_rate as u32);

        self.swr = Some(ffmpeg::software::resampler(swr_input, swr_output).unwrap());
        self.sws = Some(
            ffmpeg::software::scaling::Context::get(
                Pixel::BGRA,
                sws_input.0,
                sws_input.1,
                Pixel::YUV420P,
                sws_output.0,
                sws_output.1,
                Flags::empty(),
            )
            .expect("failed to get video scaler"),
        );

        self.audio_encoder = Some(audio_enc);
        self.video_encoder = Some(video_enc);
    }

    fn encode_video_frame(&mut self, video_buffer: VideoBuffer) {
        let buffer_width = video_buffer.resolution.width as u32;
        let buffer_height = video_buffer.resolution.height as u32;

        // create bgra frame
        let mut source_frame = frame::Video::new(Pixel::BGRA, buffer_width, buffer_height);
        source_frame.set_pts(Some(self.video_pts));

        // copy data to frame
        source_frame.data_mut(0).copy_from_slice(&video_buffer.bgra);

        // create yuv frame
        let mut output_frame = frame::Video::new(
            Pixel::YUV420P,
            self.target_resolution[0],
            self.target_resolution[1],
        );
        output_frame.set_pts(Some(self.video_pts));

        // transcode bgra to yuv
        self.sws
            .as_mut()
            .expect("failed to get video scaler")
            .run(&source_frame, &mut output_frame)
            .expect("failed to transcode video frame");
        self.encode_video(output_frame);
    }

    fn encode_audio_frame(&mut self, audio_buffer: AudioBuffer) {
        let total_bytes = audio_buffer.buffer.len();
        let channels = self.audio_channels.unwrap();
        if total_bytes % channels != 0 {
            panic!(
                "Buffer length {} is not a multiple of channel count {}",
                total_bytes, channels
            );
        }

        let samples_per_channel = total_bytes / channels;
        let channel_layout = self.channel_layout.unwrap();

        // create input frame
        let mut input_frame =
            frame::Audio::new(SAMPLE_FORMAT_IN, samples_per_channel, channel_layout);
        input_frame.set_format(SAMPLE_FORMAT_IN);
        input_frame.set_pts(Some(self.audio_pts));
        unsafe {
            input_frame.alloc(SAMPLE_FORMAT_IN, samples_per_channel, channel_layout);
        }

        // fill planar f32 data
        let data = input_frame.data_mut(0);
        let buffer: &mut [f32] = unsafe {
            std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut f32, audio_buffer.buffer.len())
        };
        buffer.copy_from_slice(&audio_buffer.buffer);

        // create output frame
        let mut output_frame =
            frame::Audio::new(SAMPLE_FORMAT_OUT, samples_per_channel, channel_layout);
        output_frame.set_format(SAMPLE_FORMAT_OUT);
        output_frame.set_pts(Some(self.audio_pts));
        unsafe {
            input_frame.alloc(SAMPLE_FORMAT_IN, samples_per_channel, channel_layout);
        }

        self.swr
            .as_mut()
            .unwrap()
            .run(&input_frame, &mut output_frame)
            .expect("failed to resample audio");
        self.encode_audio(output_frame);
    }

    pub fn start(&mut self, rx: Receiver<MuxerCommand>) {
        let start_time = Instant::now();
        let mut last_print = Instant::now();

        loop {
            if let Ok(cmd) = rx.try_recv() {
                match cmd {
                    MuxerCommand::Clip => todo!("rewrite write_clip for safe ffmpeg"),
                }
            }

            while let Ok(video_buf) = self.video_api.video_rx.try_recv() {
                self.encode_video_frame(video_buf);
            }

            while let Ok(audio_buf) = self.audio_api.audio_rx.try_recv() {
                self.encode_audio_frame(audio_buf);
            }

            if last_print.elapsed() >= Duration::from_secs(1) {
                println!("[muxer] Recording... {}s", start_time.elapsed().as_secs());
                last_print = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn encode_video(&mut self, frame: frame::Video) {
        let video_encoder = match &mut self.video_encoder {
            Some(enc) => enc,
            None => return,
        };

        video_encoder.send_frame(&frame).unwrap();

        // receive packets from encoder
        loop {
            let mut packet = packet::Packet::empty();
            if video_encoder.receive_packet(&mut packet).is_err() {
                break;
            }

            packet.set_stream(0);

            // add packet to replay buffer (buffer takes ownership)
            self.replay_buffer.add_frame(packet);
        }

        self.video_pts += 1;
    }

    fn encode_audio(&mut self, frame: frame::Audio) {
        let audio_encoder = match &mut self.audio_encoder {
            Some(enc) => enc,
            None => return,
        };

        audio_encoder.send_frame(&frame).unwrap();

        // receive packets from encoder
        loop {
            let mut packet = packet::Packet::empty();
            audio_encoder.receive_packet(&mut packet).unwrap();

            packet.set_stream(1);

            // add packet to replay buffer (buffer takes ownership)
            self.replay_buffer.add_frame(packet);
        }
    }
}
