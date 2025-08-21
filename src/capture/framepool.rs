use rusty_ffmpeg::ffi::{AVFrame, av_frame_alloc};

struct FramePool {
    frames: Vec<*mut AVFrame>,
}

impl FramePool {
    fn get_frame(&mut self) -> *mut AVFrame {
        self.frames
            .pop()
            .unwrap_or_else(|| unsafe { av_frame_alloc() })
    }

    fn return_frame(&mut self, frame: *mut AVFrame) {
        self.frames.push(frame);
    }
}
