use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam::channel::{self, Receiver, SendError};
use pipewire::spa;
use pipewire::spa::sys::SPA_DIRECTION_INPUT;
use pipewire::spa::utils::Direction;
use pipewire::stream::StreamFlags;
use pipewire::sys::{PW_KEY_MEDIA_CATEGORY, PW_KEY_MEDIA_ROLE, PW_KEY_MEDIA_TYPE};

#[derive(Debug)]
pub struct AudioBuffer {
    pub buffer: Vec<f32>,
    pub time: Duration,
}

// this is going to the muxer
pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    pub sample_rate: Option<u16>,
    pub channels: Option<u32>,
    pub bit_rate: Option<usize>,

    start: Arc<Instant>,
    stop_tx: channel::Sender<bool>,
}

impl AudioCaptureApi {
    pub fn new(start: Arc<Instant>) -> AudioCaptureApi {
        let (audio_tx, audio_rx) = channel::unbounded::<AudioBuffer>();
        let (stop_tx, stop_rx) = channel::unbounded::<bool>();

        Self {
            stop_tx,
            audio_rx,
            start,
            sample_rate: None,
            channels: None,
            bit_rate: None,
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

    fn init(&mut self) {
        pipewire::init();

        let mainloop = pipewire::main_loop::MainLoop::new(None).unwrap();
        let context = pipewire::context::Context::new(&mainloop).unwrap();
        let core = context.connect(None).unwrap();
        let registry = core.get_registry().unwrap();

        let mut properties = pipewire::properties::Properties::new();
        properties.insert(PW_KEY_MEDIA_TYPE, "Video");
        properties.insert(PW_KEY_MEDIA_CATEGORY, "Capture");
        properties.insert(PW_KEY_MEDIA_ROLE, "Camer");

        let stream = pipewire::stream::Stream::new(&core, "capturer", properties).unwrap();

        let flags = StreamFlags::empty();
        flags.insert(StreamFlags::AUTOCONNECT);
        flags.insert(StreamFlags::MAP_BUFFERS);

        let params = spa::pod::builder::Builder::new(&mut Vec::new());
        stream.connect(Direction::Input, None, flags, None).unwrap();
    }
}
