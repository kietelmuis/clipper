use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use windows::Win32::System::Com::CoUninitialize;
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{CloseHandle, HANDLE},
        Media::Audio::{
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_LOOPBACK, EDataFlow, ERole, IAudioCaptureClient, IAudioClient,
            IMMDeviceEnumerator, MMDeviceEnumerator,
        },
        System::{
            Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STGM_READ},
            Threading::{CreateEventW, INFINITE, ResetEvent, WaitForSingleObject},
        },
    },
    core::IUnknown,
};

#[derive(Debug)]
pub struct AudioBuffer {
    pub buffer: Vec<u8>,
    pub time: Duration,
}

pub struct AudioCaptureApi {
    event_handle: HANDLE,
    audio_capture_client: IAudioCaptureClient,
    callback: Sender<AudioBuffer>,
    instant: Arc<Instant>,
    stop_rx: Receiver<()>,
}

struct ComThread;

// initialize multithrading within thread
impl ComThread {
    fn new() -> Self {
        unsafe {
            _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        ComThread
    }
}

// uninitiliaze when dropped within thread
impl Drop for ComThread {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

// close event handle upon captureapi drop
impl Drop for AudioCaptureApi {
    fn drop(&mut self) {
        unsafe {
            _ = CloseHandle(self.event_handle);
        }
    }
}

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>, callback: Sender<AudioBuffer>) -> Sender<()> {
        let (stop_tx, stop_rx) = channel::<()>();

        std::thread::spawn(move || {
            let _com_guard = ComThread::new();

            let enumerator = unsafe {
                CoCreateInstance::<Option<&IUnknown>, IMMDeviceEnumerator>(
                    &MMDeviceEnumerator,
                    None,
                    CLSCTX_ALL,
                )
            }
            .expect("couldn't create enumerator");

            let device = unsafe {
                enumerator.GetDefaultAudioEndpoint(EDataFlow::default(), ERole::default())
            }
            .expect("couldn't get audio device");

            let device_friendly_name = unsafe {
                device
                    .OpenPropertyStore(STGM_READ)
                    .expect("couldn't get device properties")
                    .GetValue(&PKEY_Device_FriendlyName)
                    .expect("couldn't get device name")
                    .to_string()
            };
            println!("using device: {}", device_friendly_name);

            let audio_client = unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }
                .expect("couldn't activate audio device");
            unsafe {
                audio_client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK, // wait for event and
                    1000000,                                                          // 100ns
                    0,
                    audio_client
                        .GetMixFormat()
                        .expect("couldn't get audio mix format"),
                    None,
                )
            }
            .expect("couldn't initialize audio client");

            let event = unsafe { CreateEventW(None, true, false, None) }
                .expect("couldn't create event handle");
            unsafe { audio_client.SetEventHandle(event) }.expect("couldn't set audio event handle");

            let buffer_size =
                unsafe { audio_client.GetBufferSize() }.expect("couldn't get buffer size");
            println!("buffer size: {} Hz", buffer_size);

            let audio_capture_client = unsafe { audio_client.GetService::<IAudioCaptureClient>() }
                .expect("couldn't get audio capture client");

            unsafe { audio_client.Start() }.expect("failed to start audio client");

            AudioCaptureApi {
                event_handle: event,
                audio_capture_client: audio_capture_client,
                callback: callback,
                instant: instant,

                stop_rx: stop_rx,
            }
            .audio_loop();
        });

        stop_tx
    }

    fn audio_loop(&mut self) {
        loop {
            unsafe {
                WaitForSingleObject(self.event_handle, INFINITE);
            }

            loop {
                match self.stop_rx.try_recv() {
                    Ok(()) => {
                        println!("paused");
                        break;
                    }
                    _ => {}
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;
                let mut device_pos: u64 = 0;
                let mut qpc_pos: u64 = 0;

                unsafe {
                    self.audio_capture_client.GetBuffer(
                        &mut data_ptr,
                        &mut num_frames,
                        &mut flags,
                        Some(&mut device_pos),
                        Some(&mut qpc_pos),
                    )
                }
                .expect("failed to get buffer!");

                // no more data to read right now
                if num_frames == 0 {
                    break;
                }

                // process audio data here
                // let sample_count = num_frames as usize * 2;
                // let samples =
                //     unsafe { std::slice::from_raw_parts(data_ptr as *const i16, sample_count) };

                // let sum: i32 = samples.iter().map(|&s| (s as i32).abs()).sum();
                // if sum > 0 {
                //     println!("audio detected");
                // }

                let bytes_per_frame = 2 * 2; // 16-bit + 2 channels = 4 bytes
                let buffer_len = (num_frames as usize) * bytes_per_frame; // number of frames to encode * bytes per frame

                let buffer_slice = unsafe { std::slice::from_raw_parts(data_ptr, buffer_len) };
                if let Err(e) = self.callback.send(AudioBuffer {
                    buffer: buffer_slice.into(),
                    time: self.instant.elapsed(),
                }) {
                    eprintln!("failed to send audio callback: {}", e);
                }

                unsafe { self.audio_capture_client.ReleaseBuffer(num_frames) }
                    .expect("failed to release buffer");
            }

            unsafe {
                ResetEvent(self.event_handle).expect("failed to reset event");
            }
        }
    }
}
