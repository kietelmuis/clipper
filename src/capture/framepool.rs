use rusty_ffmpeg::ffi::{AVFrame, av_frame_alloc, av_frame_unref};

pub struct FramePool {
    frames: Vec<*mut AVFrame>,
    total_allocated: usize, // Track total frames
    max_frames: usize,      // Limit total frames
}

impl FramePool {
    pub fn new() -> Self {
        FramePool {
            frames: Vec::new(),
            total_allocated: 0,
            max_frames: 30, // Cap total frames at 30
        }
    }

    pub fn get_frame(&mut self) -> *mut AVFrame {
        if let Some(frame) = self.frames.pop() {
            frame
        } else if self.total_allocated < self.max_frames {
            self.total_allocated += 1;
            unsafe { av_frame_alloc() }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.frames
                .pop()
                .unwrap_or_else(|| unsafe { av_frame_alloc() })
        }
    }

    pub fn return_frame(&mut self, frame: *mut AVFrame) {
        unsafe {
            av_frame_unref(frame);
        }
        self.frames.push(frame);
    }
}
