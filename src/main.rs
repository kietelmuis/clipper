#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod config;

use capture::*;

fn main() {
    let config = config::Config::new();

    std::thread::spawn(move || {
        capture::CaptureMuxer::new(CaptureSettings {
            resolution: config.resolution,
            fps: config.fps,
        })
        .start_muxer();
    });

    tauri_app_lib::run();
}
