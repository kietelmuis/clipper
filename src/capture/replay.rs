use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use rusty_ffmpeg::ffi::{self as ffmpeg, AVPacket};

pub struct ReplayBuffer {
    pub bytes: usize,
    frames: VecDeque<(*mut AVPacket, Instant)>,
    duration: Duration,
}

// unref and free all packets using drain to take ownership of pointers
impl Drop for ReplayBuffer {
    fn drop(&mut self) {
        for (mut packet, _) in self.frames.drain(..) {
            unsafe {
                self.bytes = self.bytes.saturating_sub((*packet).size.max(0) as usize);

                ffmpeg::av_packet_unref(packet);
                ffmpeg::av_packet_free(&mut packet);
            }
        }
        self.frames.clear();
    }
}

impl ReplayBuffer {
    // calculate the frame cutoff amount upon cleaning
    pub fn new(duration: Duration) -> Self {
        Self {
            frames: VecDeque::new(),
            duration,
            bytes: 0,
        }
    }

    // cutoff older frames outside of duration
    pub fn add_frame(&mut self, packet: *mut AVPacket) {
        let now = Instant::now();

        unsafe {
            self.bytes = self.bytes.saturating_add((*packet).size.max(0) as usize);
        }

        self.frames.push_back((packet, now));

        // evict memory hungry old frames
        while let Some((oldest_packet, oldest_instant)) = self.frames.front() {
            if now.duration_since(*oldest_instant) > self.duration {
                let mut to_free = *oldest_packet;
                unsafe {
                    self.bytes = self.bytes.saturating_sub((*to_free).size.max(0) as usize);
                    ffmpeg::av_packet_unref(to_free);
                    ffmpeg::av_packet_free(&mut to_free);
                }
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    // simply clone the frames and into to write them
    pub fn get_frames(&self) -> Vec<*mut AVPacket> {
        self.frames.iter().map(|(packet, _)| *packet).collect()
    }
}
