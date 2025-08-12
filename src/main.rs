use rdev::{Event, EventType, listen};
use std::time::Duration;

mod capture;
mod config;

use capture::{CaptureMuxer, CaptureSettings};

fn callback(event: Event) {
    if event.event_type == EventType::KeyPress(rdev::Key::F9) {
        println!("clipping!");
    }
}

fn main() {
    let config = config::Config::new();

    // input thread
    std::thread::spawn(|| {
        if let Err(error) = listen(callback) {
            eprintln!("Error: {:?}", error);
        }
    });

    let mut muxer = CaptureMuxer::new(CaptureSettings {
        resolution: config.resolution,
        fps: config.fps,
    });
    muxer.init();
}
