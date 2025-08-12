use std::env::consts::OS;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam::channel::{self, Receiver, SendError};

#[cfg(windows)]
use windows::Win32::Media::Audio::{PKEY_AudioEngine_DeviceFormat, WAVEFORMATEXTENSIBLE};
#[cfg(windows)]
use windows::Win32::System::Com::CoUninitialize;
#[cfg(windows)]
use windows::core::GUID;
#[cfg(windows)]
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
            Com::{CLSCTX_ALL, CoCreateInstance, STGM_READ},
            Threading::{CreateEventW, INFINITE, ResetEvent, WaitForSingleObject},
        },
    },
    core::IUnknown,
};

#[derive(Debug)]
pub struct AudioBuffer {
    pub buffer: Vec<f32>,
    pub time: Duration,
}

struct InternalCaptureApi {
    #[cfg(windows)]
    event_handle: Option<HANDLE>,
    #[cfg(windows)]
    capture_client: Option<IAudioCaptureClient>,
    stop_rx: channel::Receiver<bool>,
    instant: Arc<Instant>,
    callback: channel::Sender<AudioBuffer>,
    channels: Option<u16>,
}

#[cfg(windows)]
unsafe impl Send for InternalCaptureApi {}
#[cfg(windows)]
unsafe impl Sync for InternalCaptureApi {}

pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    pub sample_rate: Option<u16>,
    pub channels: Option<u32>,

    inner: Arc<Mutex<InternalCaptureApi>>,
    stop_tx: channel::Sender<bool>,
}

#[cfg(windows)]
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

// close event handle upon captureapi drop
#[cfg(windows)]
impl Drop for InternalCaptureApi {
    fn drop(&mut self) {
        println!("cleaning audio api");
        unsafe {
            CoUninitialize();

            if let Some(handle) = self.event_handle {
                _ = CloseHandle(handle);
            }
        }
    }
}

#[cfg(not(windows))]
impl Drop for InternalCaptureApi {
    fn drop(&mut self) {
        println!("cleaning audio api (linux stub)");
    }
}

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (audio_tx, audio_rx) = channel::unbounded::<AudioBuffer>();
        let (stop_tx, stop_rx) = channel::unbounded::<bool>();

        let internal_api = Arc::new(Mutex::new(InternalCaptureApi {
            #[cfg(windows)]
            event_handle: None,
            #[cfg(windows)]
            capture_client: None,
            stop_rx: stop_rx,
            instant: instant,
            callback: audio_tx,
            channels: None,
        }));

        let mut capture_api = AudioCaptureApi {
            audio_rx,
            sample_rate: None,
            channels: None,
            inner: internal_api.clone(),
            stop_tx,
        };

        // Use OS constant from main.rs
        if OS == "windows" {
            capture_api.init();

            let inner = internal_api.clone();
            std::thread::spawn(move || {
                let mut guard = inner.lock().unwrap();
                guard.event_loop();
            });
        } else {
            println!("Audio capture not supported on this platform (Linux stub)");
        }

        capture_api
    }

    pub fn start(&mut self) -> Result<(), SendError<bool>> {
        if OS == "windows" {
            self.stop_tx.send(true)?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SendError<bool>> {
        if OS == "windows" {
            self.stop_tx.send(false)?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn init(&mut self) {
        // create audio device enumerator
        let enumerator = unsafe {
            CoCreateInstance::<Option<&IUnknown>, IMMDeviceEnumerator>(
                &MMDeviceEnumerator,
                None,
                CLSCTX_ALL,
            )
        }
        .expect("couldn't create enumerator");

        // create audio device from default endpoint
        let device =
            unsafe { enumerator.GetDefaultAudioEndpoint(EDataFlow::default(), ERole::default()) }
                .expect("couldn't get audio device");

        // get device data
        let property_store = unsafe {
            device
                .OpenPropertyStore(STGM_READ)
                .expect("failed to open properties")
        };

        let name = unsafe { property_store.GetValue(&PKEY_Device_FriendlyName) }
            .expect("failed to get audio device name");
        println!("[audio] using device: {}", name);

        // create and activate audio client on device
        let audio_client = unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }
            .expect("couldn't activate audio device");

        // Get format directly from audio client
        let format_ptr = unsafe { audio_client.GetMixFormat().unwrap() };
        let wave_format = unsafe { format_ptr.as_ref().expect("Null format pointer") };

        // Check format details
        let sample_rate = wave_format.nSamplesPerSec;
        let channels = wave_format.nChannels;
        println!(
            "[Audio] Channels: {}, Sample Rate: {}",
            channels, sample_rate
        );

        self.inner.lock().unwrap().channels = Some(channels);

        // Validate format type
        let is_float = match wave_format.wFormatTag {
            wave_format_ieee_float => true,
            wave_format_extensible => {
                // Cast to WAVEFORMATEXTENSIBLE and read unaligned
                let wave_format_ext_ptr =
                    unsafe { format_ptr.cast::<WAVEFORMATEXTENSIBLE>().as_ref() }.unwrap();

                // SAFETY: We know the pointer is valid and we're using read_unaligned
                let sub_format = unsafe {
                    // Get pointer to SubFormat field without creating a reference
                    let sub_format_ptr = std::ptr::addr_of!((*wave_format_ext_ptr).SubFormat);
                    std::ptr::read_unaligned(sub_format_ptr)
                };

                sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            }
            _ => false,
        };

        if !is_float {
            panic!("Unsupported audio format - expected IEEE Float");
        }

        unsafe {
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK, // wait for event and
                1000000,                                                          // 100ms buffer
                0,
                format_ptr,
                None,
            )
        }
        .expect("couldn't initialize audio client");

        // create event handle for audio events
        let event =
            unsafe { CreateEventW(None, true, false, None) }.expect("couldn't create event handle");
        unsafe { audio_client.SetEventHandle(event) }.expect("couldn't set audio event handle");

        self.inner.lock().unwrap().event_handle = Some(event);

        let audio_capture_client = unsafe { audio_client.GetService::<IAudioCaptureClient>() }
            .expect("couldn't get audio capture client");

        unsafe { audio_client.Start() }.expect("failed to start audio client");

        self.inner.lock().unwrap().capture_client = Some(audio_capture_client);
    }

    #[cfg(not(windows))]
    fn init(&mut self) {
        // Linux stub
        println!("AudioCaptureApi::init() called on non-Windows platform");
    }
}

#[cfg(windows)]
impl InternalCaptureApi {
    fn event_loop(&mut self) {
        let capture_client = self
            .capture_client
            .as_mut()
            .expect("audio capture client not ready!");

        let handle = self.event_handle.expect("event handle missing");

        const FRAME_SIZE: usize = 1024; // AAC requires 1024 samples per frame
        let samples_per_frame = FRAME_SIZE * self.channels.unwrap() as usize;

        let mut staging_buf = Vec::with_capacity(samples_per_frame * 2);
        let mut last_time = self.instant.elapsed();

        loop {
            unsafe {
                WaitForSingleObject(handle, INFINITE);
            }

            // break inner loop to wait for next audio
            // outerloop should never break
            loop {
                if let Ok(true) = self.stop_rx.try_recv() {
                    println!("paused");
                    loop {
                        if let Ok(false) = self.stop_rx.try_recv() {
                            println!("resumed");
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;

                unsafe {
                    capture_client.GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                }
                .expect("failed to get buffer!");

                // no more data to read right now
                if num_frames == 0 {
                    break;
                }

                // process audio data here
                // SAFETY: WASAPI guarantees the buffer contains float samples
                let samples = unsafe {
                    std::slice::from_raw_parts(
                        data_ptr as *const f32,
                        num_frames as usize * self.channels.unwrap() as usize,
                    )
                };

                // Process samples (example: simple volume adjustment)
                let processed_samples: Vec<f32> = samples
                    .iter()
                    .map(|sample| sample * 0.8) // Reduce volume by 20%
                    .collect();

                staging_buf.extend_from_slice(&processed_samples);

                // Release buffer
                unsafe { capture_client.ReleaseBuffer(num_frames) }
                    .expect("failed to release buffer");

                // Send complete frames
                while staging_buf.len() >= samples_per_frame {
                    let frame = AudioBuffer {
                        buffer: staging_buf.drain(..samples_per_frame).collect(),
                        time: last_time,
                    };

                    last_time = self.instant.elapsed();

                    if let Err(e) = self.callback.send(frame) {
                        eprintln!("Failed to send audio frame: {}", e);
                    }
                }
            }

            unsafe {
                ResetEvent(handle).expect("failed to reset event");
            }
        }
    }
}

#[cfg(not(windows))]
impl InternalCaptureApi {
    fn event_loop(&mut self) {
        println!("InternalCaptureApi::event_loop() called on non-Windows platform");
    }
}
