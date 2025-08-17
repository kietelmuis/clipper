use std::{
    collections::VecDeque,
    ptr::NonNull,
    time::{Duration, Instant},
};

use rusty_ffmpeg::ffi::{AVPacket, av_packet_free};

struct Frame {
    pkt: NonNull<AVPacket>,
    time: Instant,
    size: usize,
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            let mut pkt = self.pkt.as_ptr();
            av_packet_free(&mut pkt);
        }
    }
}

pub struct ReplayBuffer {
    pub bytes: usize,
    frames: VecDeque<Frame>,
    buffer_duration: Duration,
}

impl ReplayBuffer {
    pub fn new(duration: Duration) -> Self {
        Self {
            frames: VecDeque::new(),
            buffer_duration: duration,
            bytes: 0,
        }
    }

    pub fn add_frame(&mut self, frame: *mut AVPacket) {
        let pkt = unsafe { &*frame };
        let mut total = pkt.size.max(0) as usize;

        // calculate size of side data and add to total
        if pkt.side_data_elems > 0 && !pkt.side_data.is_null() {
            for i in 0..(pkt.side_data_elems as isize) {
                unsafe {
                    let sd = &*pkt.side_data.offset(i);
                    total += sd.size.max(0) as usize;
                }
            }
        }

        let time = Instant::now();

        self.frames.push_back(Frame {
            pkt: unsafe { NonNull::new_unchecked(frame) },
            time,
            size: total,
        });

        self.bytes += total;
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

        println!(
            "[replay] collected {} frames for clip from buffer",
            result.len()
        );
        result
    }

    fn cleanup(&mut self, time: Instant) {
        let cutoff = time - self.buffer_duration;

        while let Some(frame) = self.frames.front() {
            if frame.time < cutoff {
                let old = self.frames.pop_front().unwrap();
                self.bytes = self.bytes.saturating_sub(old.size);
            } else {
                break;
            }
        }
    }
}
