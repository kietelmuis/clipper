use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam::channel::{self, Receiver, SendError};

use windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::GUID;
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
    pub timestamp: Instant,
}

struct InternalCaptureApi {
    event_handle: Option<HANDLE>,
    capture_client: Option<IAudioCaptureClient>,
    stop_rx: channel::Receiver<bool>,
    instant: Arc<Instant>,
    callback: channel::Sender<AudioBuffer>,
    channels: Option<u16>,
}

unsafe impl Send for InternalCaptureApi {}
unsafe impl Sync for InternalCaptureApi {}

pub struct AudioCaptureApi {
    pub audio_rx: Receiver<AudioBuffer>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub bit_rate: Option<usize>,

    inner: Arc<Mutex<InternalCaptureApi>>,
    stop_tx: channel::Sender<bool>,
}

const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

const WAVE_FLOAT: u16 = 3;
const WAVE_EXTENSIBLE: u16 = 65534;

// close event handle upon captureapi drop
impl Drop for InternalCaptureApi {
    fn drop(&mut self) {
        println!("cleaning internal audio api");
        unsafe {
            if let Some(handle) = self.event_handle {
                _ = CloseHandle(handle);
            }
        }
    }
}

impl Drop for AudioCaptureApi {
    fn drop(&mut self) {
        println!("cleaning audio api");
    }
}

impl AudioCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (audio_tx, audio_rx) = channel::unbounded::<AudioBuffer>();
        let (stop_tx, stop_rx) = channel::unbounded::<bool>();

        let internal_api = Arc::new(Mutex::new(InternalCaptureApi {
            event_handle: None,
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
            bit_rate: None,
            inner: internal_api.clone(),
            stop_tx,
        };

        capture_api.init();

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

    fn init(&mut self) {
        // initialize COM
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).unwrap();
        }

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
        let format_ptr = unsafe {
            audio_client
                .GetMixFormat()
                .expect("could not get wave format")
        };
        let wave_format = unsafe { format_ptr.as_ref().expect("could not get wave format") };

        // Check format details
        let sample_rate = wave_format.nSamplesPerSec;
        let channels = wave_format.nChannels;
        let bits_rate = wave_format.nSamplesPerSec
            * wave_format.wBitsPerSample as u32
            * wave_format.nChannels as u32;
        println!(
            "[audio] channels: {}, sample rate: {}hz, bits rate: {}bps",
            channels, sample_rate, bits_rate
        );

        self.sample_rate = Some(sample_rate as i32);
        self.channels = Some(channels as i32);
        self.bit_rate = Some(bits_rate as usize);

        self.inner.lock().unwrap().channels = Some(channels);

        match wave_format.wFormatTag {
            WAVE_FLOAT => true,
            WAVE_EXTENSIBLE => unsafe {
                // get format guid
                let sub_format_ptr =
                    std::ptr::addr_of!((*format_ptr.cast::<WAVEFORMATEXTENSIBLE>()).SubFormat);

                std::ptr::read_unaligned(sub_format_ptr) == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            },
            _ => panic!("unsupported audio format"),
        };

        unsafe {
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK, // wait for event instead of polling
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
}

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

        'audio_loop: loop {
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

                // convert data pointer to rust f32 slice
                let samples = unsafe {
                    std::slice::from_raw_parts(
                        data_ptr as *const f32,
                        num_frames as usize * self.channels.unwrap() as usize,
                    )
                };

                // reduce volume by 20%
                let processed_samples: Vec<f32> =
                    samples.iter().map(|sample| sample * 0.8).collect();

                staging_buf.extend_from_slice(&processed_samples);

                // Release buffer
                unsafe { capture_client.ReleaseBuffer(num_frames) }
                    .expect("failed to release buffer");

                // send complete frames
                while staging_buf.len() >= samples_per_frame {
                    let frame = AudioBuffer {
                        buffer: staging_buf.drain(..samples_per_frame).collect(),
                        timestamp: Instant::now(),
                    };

                    if let Err(e) = self.callback.send(frame) {
                        eprintln!("Failed to send audio frame: {}", e);
                        break 'audio_loop;
                    }
                }
            }

            unsafe {
                ResetEvent(handle).expect("failed to reset event");
            }
        }
    }
}
