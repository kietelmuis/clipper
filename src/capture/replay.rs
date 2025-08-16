use std::{
    collections::VecDeque,
    ptr::NonNull,
    time::{Duration, Instant},
};

use rusty_ffmpeg::ffi::{AVPacket, av_packet_free};

struct Frame {
    pkt: NonNull<AVPacket>,
    time: Instant,
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            let mut pkt = self.pkt.as_ptr();
            if !pkt.is_null() {
                av_packet_free(&mut pkt);
            }
        }
    }
}

pub struct ReplayBuffer {
    frames: VecDeque<Frame>,
    buffer_duration: Duration,
}

impl ReplayBuffer {
    pub fn new(duration: Duration) -> Self {
        Self {
            frames: VecDeque::new(),
            buffer_duration: duration,
        }
    }

    pub fn add_frame(&mut self, frame: *mut AVPacket) {
        if frame.is_null() {
            println!("frame given is invalid");
            return;
        }

        let time = Instant::now();

        self.frames.push_back(Frame {
            pkt: unsafe { NonNull::new_unchecked(frame) },
            time,
        });
        self.cleanup(time);
    }

    pub fn get_frames(&self) -> Vec<*mut AVPacket> {
        if self.frames.is_empty() {
            println!("no frames in replay buffer for clip");
            return Vec::new();
        }

        let mut result = Vec::with_capacity(self.frames.len());

        // collect frame pointers
        for frame in &self.frames {
            result.push(frame.pkt.as_ptr());
        }

        println!("Retrieved {} frames for clip from buffer", result.len());
        result
    }

    fn cleanup(&mut self, time: Instant) {
        let cutoff = time - self.buffer_duration;

        while let Some(frame) = self.frames.front() {
            if frame.time < cutoff {
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }
}
