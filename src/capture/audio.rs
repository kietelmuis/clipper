use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam::channel::{self, Receiver, SendError};

use windows::Win32::Media::Audio::PKEY_AudioEngine_DeviceFormat;
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
            Com::{CLSCTX_ALL, CoCreateInstance, STGM_READ},
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

struct InternalCaptureApi {
    event_handle: Option<HANDLE>,
    capture_client: Option<IAudioCaptureClient>,
    stop_rx: channel::Receiver<bool>,
    instant: Arc<Instant>,
    callback: channel::Sender<AudioBuffer>,
}

// im dead
unsafe impl Send for InternalCaptureApi {}
unsafe impl Sync for InternalCaptureApi {}

pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    internal: Arc<Mutex<InternalCaptureApi>>,
    stop_tx: channel::Sender<bool>,
}

// close event handle upon captureapi drop
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

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (audio_tx, audio_rx) = channel::unbounded::<AudioBuffer>();
        let (stop_tx, stop_rx) = channel::unbounded::<bool>();

        let mut internal_api = Arc::new(Mutex::new(InternalCaptureApi {
            event_handle: None,
            capture_client: None,
            stop_rx: stop_rx,
            instant: instant,
            callback: audio_tx,
        }));

        let mut capture_api = AudioCaptureApi {
            audio_rx,
            internal: internal_api.clone(),
            stop_tx,
        };

        internal_api.lock().unwrap().init();

        let inner = internal_api.clone();
        std::thread::spawn(move || {
            let mut guard = inner.lock().unwrap();
            guard.event_loop();
        });

        capture_api
    }

    pub fn start(&mut self) -> Result<(), SendError<bool>> {
        self.stop_tx.send(true)?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SendError<bool>> {
        self.stop_tx.send(false)?;
        Ok(())
    }
}

impl InternalCaptureApi {
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

        let _audio_engine = unsafe { property_store.GetValue(&PKEY_AudioEngine_DeviceFormat) }
            .expect("failed toget audio engine");

        // create and activate audio client on device
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

        // create event handle for audio events
        let event =
            unsafe { CreateEventW(None, true, false, None) }.expect("couldn't create event handle");
        unsafe { audio_client.SetEventHandle(event) }.expect("couldn't set audio event handle");

        self.event_handle = Some(event);

        // get device buffer size
        let buffer_size =
            unsafe { audio_client.GetBufferSize() }.expect("couldn't get buffer size");
        println!("[audio] buffersize: {}Hz", buffer_size);

        let audio_capture_client = unsafe { audio_client.GetService::<IAudioCaptureClient>() }
            .expect("couldn't get audio capture client");

        unsafe { audio_client.Start() }.expect("failed to start audio client");

        self.capture_client = Some(audio_capture_client);
    }

    fn event_loop(&mut self) {
        let capture_client = self
            .capture_client
            .as_mut()
            .expect("audio capture client not ready!");

        let handle = self.event_handle.expect("event handle missing");

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

                unsafe { capture_client.ReleaseBuffer(num_frames) }
                    .expect("failed to release buffer");
            }

            unsafe {
                ResetEvent(handle).expect("failed to reset event");
            }
        }
    }
}
