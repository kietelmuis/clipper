use crossbeam::channel;
use std::{sync::Arc, time::Instant};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
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
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Texture2D,
            },
            Dxgi::IDXGIDevice,
        },
        System::WinRT::Direct3D11::{
            CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
        },
    },
    core::{Interface, Ref},
};

#[derive(Debug)]
pub struct VideoBuffer {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp: u64,
}

pub struct VideoCaptureApi {}

impl VideoCaptureApi {
    pub fn new(
        instant: Arc<Instant>,
        callback: channel::Sender<VideoBuffer>,
    ) -> channel::Sender<()> {
        let (stop_tx, stop_rx) = channel::unbounded::<()>();

        std::thread::spawn(move || {
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
            let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &directx_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                3,
                capture_item.Size().expect("failed to get monitor size"),
            )
            .expect("failed to create frame pool");

            let cb = Arc::new(callback);

            // bind to frame
            let handler = TypedEventHandler::new(
                move |frame_pool: Ref<Direct3D11CaptureFramePool>, _: Ref<_>| {
                    if let Some(pool) = frame_pool.as_ref() {
                        if let Ok(frame) = pool.TryGetNextFrame() {
                            let content_size =
                                frame.ContentSize().expect("failed to get frame size");

                            let d3d_surface: IDirect3DSurface =
                                frame.Surface().expect("failed to get frame surface");

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

                            unsafe {
                                device.CreateTexture2D(&desc, None, Some(&mut staging_texture))
                            }
                            .expect("couldn't create staging texture");

                            let texture: ID3D11Texture2D =
                                staging_texture.clone().expect("failed to unwrap texture");

                            unsafe { device_context.CopyResource(&texture, &frame_texture) };

                            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                            unsafe {
                                device_context.Map(
                                    &texture,
                                    0,
                                    D3D11_MAP_READ,
                                    0,
                                    Some(&mut mapped),
                                )?;
                            }

                            let buffer_size =
                                (mapped.RowPitch * content_size.Height as u32) as usize;
                            let buffer = VideoBuffer {
                                bgra: unsafe {
                                    std::slice::from_raw_parts(
                                        mapped.pData as *const u8,
                                        buffer_size,
                                    )
                                }
                                .to_vec(),
                                height: content_size.Height as u32,
                                width: content_size.Width as u32,
                                timestamp: instant.elapsed().as_nanos() as u64,
                            };

                            unsafe {
                                device_context.Unmap(
                                    &staging_texture.expect("failed to unwrap staging texture"),
                                    0,
                                )
                            };

                            cb.send(buffer).expect("failed to send videobuffer");
                        }
                    }
                    Ok(())
                },
            );
            frame_pool
                .FrameArrived(&handler)
                .expect("failed to bind to frame");

            // create and start capture session
            let capture_session = frame_pool
                .CreateCaptureSession(&capture_item)
                .expect("failed to create capture session");

            capture_session
                .StartCapture()
                .expect("failed to start capturing");

            loop {
                std::thread::sleep(std::time::Duration::from_millis(10));
                if stop_rx.try_recv().is_ok() {
                    break;
                }
            }
        });

        stop_tx
    }
}
