use notify_rust::Notification;
use rdev::{Event, EventType, listen};

use crate::capture::muxer::{CaptureMuxer, CaptureSettings, MuxerCommand};

mod capture;
mod config;
mod util;

fn main() {
    let (tx, rx) = crossbeam::channel::unbounded::<MuxerCommand>();

    let config = config::Config::new();
    let mut muxer = CaptureMuxer::new(CaptureSettings {
        resolution: config.resolution,
        fps: config.fps,
    });

    muxer.init();

    std::thread::spawn(move || {
        if let Err(error) = listen(move |event: Event| {
            if event.event_type == EventType::KeyPress(rdev::Key::F9) {
                println!("[main] clipping!");

                Notification::new()
                    .appname("clipper")
                    .summary("video clipped!")
                    .body("saved last 3s to disk")
                    .show()
                    .expect("failed to show notification");

                tx.send(MuxerCommand::Clip)
                    .expect("failed to send clip command");
            }
        }) {
            eprintln!("Error: {:?}", error);
        }
    });

    // blocking btw
    muxer.start(rx);
}
