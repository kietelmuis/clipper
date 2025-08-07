use crossbeam::channel::{self, Receiver, SendError, Sender};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{
            Direct3D11::{IDirect3DDevice, IDirect3DSurface},
            DirectXPixelFormat,
        },
        DisplayId,
    },
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::IDXGIDevice,
        },
        System::WinRT::Direct3D11::{
            CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
        },
    },
    core::{IInspectable, Interface, Ref},
};

#[derive(Debug)]
pub struct VideoBuffer {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp: Duration,
}

impl Drop for VideoCaptureApi {
    fn drop(&mut self) {
        println!("cleaning video api");
        _ = self.stop();
    }
}

pub struct VideoCaptureApi {
    pub video_rx: Receiver<VideoBuffer>,

    instant: Arc<Instant>,
    callback: Arc<Sender<VideoBuffer>>,

    frame_pool: Option<Arc<Direct3D11CaptureFramePool>>,
    frame_handler: Option<TypedEventHandler<Direct3D11CaptureFramePool, IInspectable>>,
    capture_session: Option<GraphicsCaptureSession>,
}

impl VideoCaptureApi {
    pub fn new(instant: Arc<Instant>) -> Self {
        let (video_tx, video_rx) = channel::unbounded::<VideoBuffer>();

        let mut capture_api = Self {
            video_rx,

            instant,
            callback: Arc::new(video_tx),

            frame_pool: None,
            frame_handler: None,
            capture_session: None,
        };

        capture_api.init();
        capture_api
    }

    pub fn start(&mut self) -> windows::core::Result<()> {
        if let Some(session) = &self.capture_session {
            session.StartCapture()?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> windows::core::Result<()> {
        if let Some(session) = &self.capture_session {
            session.Close()?;
        }
        Ok(())
    }

    fn init(&mut self) {
        let mut device_option = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device_option),
                None,
                None,
            )
        }
        .expect("failed to create device");

        let device = Arc::new(device_option.expect("failed to unwrap device"));
        let device_context = Arc::new(
            unsafe { device.GetImmediateContext() }.expect("failed to get device context"),
        );

        let dxgi_device: IDXGIDevice = device.cast().expect("failed to cast dxgi device");

        let directx_device: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
                .expect("failed to get direct3d device")
                .cast()
                .expect("failed to cast d3d device");

        // create capture item from primary display
        let capture_item =
            GraphicsCaptureItem::TryCreateFromDisplayId(DisplayId { Value: 0 }).expect("ok");

        // create frame pool with d3d devie and monitor size
        let frame_pool = Arc::new(
            Direct3D11CaptureFramePool::CreateFreeThreaded(
                &directx_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                3,
                capture_item.Size().expect("failed to get monitor size"),
            )
            .expect("failed to create frame pool"),
        );

        // save framepool in struct to avoid out of scope
        self.frame_pool = Some(frame_pool.clone());

        let instant_clone = self.instant.clone();
        let callback_clone = self.callback.clone();

        // bind to frame
        println!("[video] event bound");
        let handler = TypedEventHandler::new(
            move |fpool: Ref<Direct3D11CaptureFramePool>, _: Ref<IInspectable>| {
                if let Some(pool) = fpool.as_ref() {
                    if let Ok(frame) = pool.TryGetNextFrame() {
                        VideoCaptureApi::handle_frame(
                            frame,
                            device.clone(),
                            device_context.clone(),
                            instant_clone.clone(),
                            callback_clone.clone(),
                        );
                    }
                }
                Ok(())
            },
        );
        self.frame_handler = Some(handler);

        frame_pool
            .FrameArrived(self.frame_handler.as_ref().expect("handler not set"))
            .expect("failed to set frame arrived");

        // create and start capture session
        let capture_session = frame_pool
            .CreateCaptureSession(&capture_item)
            .expect("failed to create capture session");

        capture_session
            .StartCapture()
            .expect("failed to start capturing");

        // store capture session to keep it alive
        self.capture_session = Some(capture_session);
    }

    fn handle_frame(
        frame: Direct3D11CaptureFrame,
        device: Arc<ID3D11Device>,
        device_context: Arc<ID3D11DeviceContext>,
        instant: Arc<Instant>,
        callback: Arc<Sender<VideoBuffer>>,
    ) {
        let d3d_surface: IDirect3DSurface = frame.Surface().expect("failed to get frame surface");

        let dxgi_interface_access: IDirect3DDxgiInterfaceAccess = d3d_surface
            .cast()
            .expect("failed to cast to interface access");

        let frame_texture: ID3D11Texture2D =
            unsafe { dxgi_interface_access.GetInterface::<ID3D11Texture2D>() }
                .expect("could not get texture interface")
                .cast()
                .expect("could not cast texture interface");

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            frame_texture.GetDesc(&mut desc);
        }

        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;

        let mut staging_texture: Option<ID3D11Texture2D> = None;

        unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging_texture)) }
            .expect("couldn't create staging texture");

        let texture: ID3D11Texture2D = staging_texture.clone().expect("failed to unwrap texture");

        unsafe { device_context.CopyResource(&texture, &frame_texture) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            device_context
                .Map(&texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .expect("failed to map texture");
        }

        let content_size = frame.ContentSize().expect("failed to get frame size");

        let buffer_size = (mapped.RowPitch * content_size.Height as u32) as usize;
        let buffer = VideoBuffer {
            bgra: unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, buffer_size) }
                .to_vec(),
            height: content_size.Height as u32,
            width: content_size.Width as u32,
            timestamp: instant.elapsed(),
        };

        unsafe {
            device_context.Unmap(
                &staging_texture.expect("failed to unwrap staging texture"),
                0,
            )
        };

        if let Err(e) = callback.send(buffer) {
            println!("Failed to send video buffer: {:?}", e)
        }
    }
}
