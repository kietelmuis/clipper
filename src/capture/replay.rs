use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use ffmpeg::Packet;
use ffmpeg_next::{self as ffmpeg, packet::Mut};
use ffmpeg_sys_next::{self as sys, AVPacket};

pub struct ReplayBuffer {
    pub bytes: usize,
    frames: VecDeque<(Packet, Instant)>,
    duration: Duration,
}

// unref and free all packets using drain to take ownership of pointers
impl Drop for ReplayBuffer {
    fn drop(&mut self) {
        for (mut packet, _) in self.frames.drain(..) {
            unsafe {
                self.bytes = self.bytes.saturating_sub(packet.size().max(0) as usize);

                let mut raw: *mut ffmpeg_sys_next::AVPacket =
                    &mut packet as *mut _ as *mut ffmpeg_sys_next::AVPacket;

                sys::av_packet_unref(raw);
                sys::av_packet_free(&mut raw);
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
    pub fn add_frame(&mut self, packet: Packet) {
        let now = Instant::now();

        self.bytes = self.bytes.saturating_add(packet.size().max(0) as usize);
        self.frames.push_back((packet, now));

        // evict memory hungry old frames
        while let Some((mut oldest_packet, oldest_instant)) = self.frames.pop_front() {
            if now.duration_since(oldest_instant) > self.duration {
                unsafe {
                    self.bytes = self
                        .bytes
                        .saturating_sub(oldest_packet.size().max(0) as usize);

                    let mut packet_ptr: *mut AVPacket = oldest_packet.as_mut_ptr();
                    let raw: *mut *mut AVPacket = &mut packet_ptr as *mut *mut AVPacket;

                    sys::av_packet_unref(packet_ptr);
                    sys::av_packet_free(raw);
                }
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    // simply clone the frames and into to write them
    pub fn get_frames(&self) -> Vec<Arc<Packet>> {
        self.frames
            .iter()
            .map(|(packet, _)| unsafe {
                let cloned = unsafe {
                    let raw: *mut sys::AVPacket = sys::av_packet_clone(packet.as_ptr());
                    Packet { 0: raw }
                };
                Arc::new(cloned)
            })
            .collect()
    }
}
