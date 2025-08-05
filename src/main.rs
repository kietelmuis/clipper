use rdev::{Event, EventType, listen};

mod capture;
mod config;

use capture::*;

fn callback(event: Event) {
    if event.event_type == EventType::KeyPress(rdev::Key::F9) {
        println!("clipping!");
    }
}

fn main() {
    let config = config::Config::new();

    std::thread::spawn(|| {
        if let Err(error) = listen(callback) {
            eprintln!("Error: {:?}", error);
        }
    });

    capture::CaptureMuxer::new(CaptureSettings {
        resolution: config.resolution,
        fps: config.fps,
    })
    .init();
}
