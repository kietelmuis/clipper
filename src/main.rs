use capture::*;

mod capture;
mod config;
mod updater;

fn main() {
    let config = config::Config::new();
    let muxer = capture::CaptureMuxer::new(CaptureSettings {
        resolution: config.resolution,
        fps: config.fps,
        debug: true,
    });

    loop {}
}
