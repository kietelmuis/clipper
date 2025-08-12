#[cfg(not(windows))]
mod linux;
#[cfg(windows)]
mod windows;

mod audio;
pub mod muxer;
mod video;
