use std::{
    collections::VecDeque,
    ptr::NonNull,
    time::{Duration, Instant},
};

use rusty_ffmpeg::ffi::{self as ffmpeg, AVPacket};

pub struct ReplayBuffer {
    pub bytes: usize,
    frames: VecDeque<*mut AVPacket>,
    max_frames: usize,
}

// unref and free all packets using drain to take ownership of pointers
impl Drop for ReplayBuffer {
    fn drop(&mut self) {
        for mut packet in self.frames.drain(..) {
            unsafe {
                ffmpeg::av_packet_unref(packet);
                ffmpeg::av_packet_free(&mut packet);
            }
        }
        self.frames.clear();
    }
}

impl ReplayBuffer {
    // calculate the frame cutoff amount upon cleaning
    pub fn new(duration: Duration, fps: u64) -> Self {
        let max_frames = (duration.as_secs() * fps) as usize;
        Self {
            frames: VecDeque::with_capacity(max_frames),
            max_frames,
            bytes: 0,
        }
    }

    // cutoff older frames outside of duration
    pub fn add_frame(&mut self, packet: *mut AVPacket) {
        if self.frames.len() >= self.max_frames {
            if let Some(mut oldest_packet) = self.frames.pop_front() {
                unsafe {
                    ffmpeg::av_packet_unref(oldest_packet);
                    ffmpeg::av_packet_free(&mut oldest_packet);
                }
            }
        }

        self.frames.push_back(packet);
    }

    // simply clone the frames and into to write them
    pub fn get_frames(&self) -> Vec<*mut AVPacket> {
        self.frames.clone().into()
    }
}
