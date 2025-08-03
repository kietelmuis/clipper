mod capture;
mod config;

use capture::*;

fn main() {
    let config = config::Config::new();

    capture::CaptureMuxer::new(CaptureSettings {
        resolution: config.resolution,
        fps: config.fps,
    })
    .start_muxer();
}
