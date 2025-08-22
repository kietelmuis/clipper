#[cfg(not(windows))]
mod linux;
#[cfg(windows)]
mod windows;

mod audio;
mod framepool;
pub mod muxer;
mod replay;
mod video;
