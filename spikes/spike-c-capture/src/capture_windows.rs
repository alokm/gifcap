/// Windows capture backend for Spike C.
///
/// Primary path: Windows Graphics Capture API (WGC).
///   - Requires Windows 10 build 1903+ (19H1).
///   - Uses GraphicsCaptureItem::CreateFromMonitorHANDLE → captures entire
///     display, then we crop to the target Rect after frame delivery.
///   - Note: WGC shows a yellow highlight border around the captured source —
///     this is OS behaviour on the SOURCE being captured (other windows / the
///     display chrome). It does NOT affect GifCap's own window pixels.
///     Spike A will verify self-exclusion separately.
///
/// KEY QUESTION THIS SPIKE ANSWERS:
///   Does WGC deliver frames at 10 FPS with < 4% drop rate and < 20 ms jitter
///   when capturing the primary display via CreateFromMonitorHANDLE?
///
/// Fallback: DXGI Desktop Duplication API (no yellow border, higher privilege).
///   If WGC frame delivery fails this spike, document and switch to DXGI.

use crate::{CaptureConfig, CapturedFrame};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Foundation::TypedEventHandler,
    Graphics::Capture::{
        Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
    },
    Graphics::DirectX::DirectXPixelFormat,
    Graphics::DirectX::Direct3D11::IDirect3DDevice,
    Win32::Foundation::HMONITOR,
    Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_SDK_VERSION,
        D3D_DRIVER_TYPE_HARDWARE,
    },
    Win32::Graphics::Dxgi::IDXGIDevice,
    Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop,
    Win32::UI::WindowsAndMessaging::{EnumDisplayMonitors, MONITORENUMPROC},
};

pub fn capture(config: CaptureConfig) -> Result<Vec<CapturedFrame>, String> {
    match capture_via_wgc(&config) {
        Ok(frames) => Ok(frames),
        Err(e) => {
            log::error!("WGC capture failed: {}", e);
            Err(format!(
                "Windows Graphics Capture failed: {}\n\
                 Fallback: consider DXGI Desktop Duplication API (no yellow border,\
                 but requires higher privilege). See capture_windows.rs fallback notes.",
                e
            ))
        }
    }
}

/// Capture via Windows Graphics Capture API.
///
/// Flow:
///   1. Create a D3D11 device (hardware accelerated).
///   2. Get the primary monitor HMONITOR.
///   3. Create a GraphicsCaptureItem from the monitor via IGraphicsCaptureItemInterop.
///   4. Create a Direct3D11CaptureFramePool.
///   5. Subscribe to the FrameArrived event.
///   6. Start a GraphicsCaptureSession.
///   7. Collect frames for `config.duration`, then stop.
///   8. For each frame: copy GPU texture to CPU, extract RGBA bytes, crop to Rect.
#[cfg(target_os = "windows")]
fn capture_via_wgc(config: &CaptureConfig) -> Result<Vec<CapturedFrame>, String> {
    // ── 1. D3D11 device ───────────────────────────────────────────────────────
    let (d3d_device, _context) = create_d3d11_device()?;

    // Wrap the D3D11 device in the WinRT IDirect3DDevice interface.
    let dxgi_device: IDXGIDevice = d3d_device
        .cast()
        .map_err(|e| format!("cast to IDXGIDevice failed: {e}"))?;
    let direct3d_device = create_direct3d_device(&dxgi_device)?;

    // ── 2. Primary monitor handle ─────────────────────────────────────────────
    let hmonitor = get_primary_monitor()?;

    // ── 3. GraphicsCaptureItem from monitor ───────────────────────────────────
    let interop: IGraphicsCaptureItemInterop = windows::core::factory::<
        GraphicsCaptureItem,
        IGraphicsCaptureItemInterop,
    >()
    .map_err(|e| format!("IGraphicsCaptureItemInterop factory failed: {e}"))?;

    let item: GraphicsCaptureItem = unsafe {
        interop
            .CreateForMonitor(hmonitor)
            .map_err(|e| format!("CreateForMonitor failed: {e}"))?
    };

    let item_size = item
        .Size()
        .map_err(|e| format!("GraphicsCaptureItem::Size failed: {e}"))?;

    log::info!(
        "Capture item size: {}×{}",
        item_size.Width,
        item_size.Height
    );

    // ── 4. Frame pool ─────────────────────────────────────────────────────────
    // We request BGRA8 (B8G8R8A8UIntNormalized) as that's what WGC natively delivers.
    // We convert to RGBA after CPU readback.
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2, // buffer count — 2 frames in flight
        item_size,
    )
    .map_err(|e| format!("Direct3D11CaptureFramePool::CreateFreeThreaded failed: {e}"))?;

    // ── 5. Shared frame buffer ────────────────────────────────────────────────
    let frames: Arc<Mutex<Vec<CapturedFrame>>> = Arc::new(Mutex::new(Vec::new()));
    let frames_writer = Arc::clone(&frames);
    let capture_width = config.width;
    let capture_height = config.height;
    let capture_x = config.x;
    let capture_y = config.y;
    let d3d_device_clone = d3d_device.clone();

    // ── 6. FrameArrived handler ───────────────────────────────────────────────
    frame_pool
        .FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(
                move |pool, _| {
                    let received_at = Instant::now();
                    let pool = pool.as_ref().ok_or_else(|| {
                        windows::core::Error::new(
                            windows::core::HRESULT(0),
                            "null frame pool",
                        )
                    })?;

                    let frame = pool.TryGetNextFrame()?;
                    let surface = frame.Surface()?;

                    // Readback the texture to CPU memory and extract BGRA bytes.
                    match readback_surface_to_rgba(
                        &surface,
                        &d3d_device_clone,
                        capture_x,
                        capture_y,
                        capture_width,
                        capture_height,
                    ) {
                        Ok(rgba) => {
                            frames_writer.lock().unwrap().push(CapturedFrame {
                                rgba,
                                width: capture_width,
                                height: capture_height,
                                received_at,
                            });
                        }
                        Err(e) => {
                            log::warn!("Frame readback failed: {}", e);
                        }
                    }

                    Ok(())
                },
            ),
        )
        .map_err(|e| format!("FrameArrived subscription failed: {e}"))?;

    // ── 7. Start capture session ──────────────────────────────────────────────
    let session = frame_pool
        .CreateCaptureSession(&item)
        .map_err(|e| format!("CreateCaptureSession failed: {e}"))?;

    session
        .StartCapture()
        .map_err(|e| format!("StartCapture failed: {e}"))?;

    log::info!(
        "WGC session started — capturing for {}s",
        config.duration.as_secs()
    );

    thread::sleep(config.duration);

    // ── 8. Stop ───────────────────────────────────────────────────────────────
    session
        .Close()
        .map_err(|e| format!("session.Close failed: {e}"))?;
    frame_pool
        .Close()
        .map_err(|e| format!("frame_pool.Close failed: {e}"))?;

    log::info!("WGC session stopped");

    let collected = frames.lock().unwrap().drain(..).collect();
    Ok(collected)
}

// ── D3D11 helpers ─────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut device = None;
    let mut context = None;

    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            Default::default(),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;
    }

    Ok((
        device.ok_or("D3D11CreateDevice returned null device")?,
        context.ok_or("D3D11CreateDevice returned null context")?,
    ))
}

#[cfg(target_os = "windows")]
fn create_direct3d_device(dxgi_device: &IDXGIDevice) -> Result<IDirect3DDevice, String> {
    // Use the WinRT interop to wrap the DXGI device as an IDirect3DDevice.
    use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
    let inspectable = unsafe {
        CreateDirect3D11DeviceFromDXGIDevice(dxgi_device)
            .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice failed: {e}"))?
    };
    inspectable
        .cast::<IDirect3DDevice>()
        .map_err(|e| format!("cast to IDirect3DDevice failed: {e}"))
}

#[cfg(target_os = "windows")]
fn get_primary_monitor() -> Result<HMONITOR, String> {
    use windows::Win32::Graphics::Gdi::HMONITOR as GdiHMONITOR;
    // EnumDisplayMonitors with a null HDC enumerates all monitors.
    // The first one returned is typically the primary display.
    let monitors: Arc<Mutex<Vec<HMONITOR>>> = Arc::new(Mutex::new(Vec::new()));
    let monitors_cb = Arc::clone(&monitors);

    unsafe {
        extern "system" fn enum_monitor_proc(
            hmon: windows::Win32::Graphics::Gdi::HMONITOR,
            _hdc: windows::Win32::Graphics::Gdi::HDC,
            _lprect: *mut windows::Win32::Foundation::RECT,
            lparam: windows::Win32::Foundation::LPARAM,
        ) -> windows::Win32::Foundation::BOOL {
            let list = lparam.0 as *mut Vec<HMONITOR>;
            unsafe {
                (*list).push(HMONITOR(hmon.0));
            }
            true.into()
        }

        let mut list: Vec<HMONITOR> = Vec::new();
        windows::Win32::Graphics::Gdi::EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor_proc),
            windows::Win32::Foundation::LPARAM(&mut list as *mut _ as isize),
        );

        list.into_iter()
            .next()
            .ok_or_else(|| "No monitors found".to_string())
    }
}

/// Copy a WGC surface to CPU memory and return RGBA bytes cropped to the
/// capture rectangle.
///
/// WGC delivers BGRA textures; we swap B↔R channels to produce RGBA.
#[cfg(target_os = "windows")]
fn readback_surface_to_rgba(
    surface: &windows::Graphics::DirectX::Direct3D11::IDirect3DSurface,
    d3d_device: &ID3D11Device,
    crop_x: i32,
    crop_y: i32,
    crop_w: u32,
    crop_h: u32,
) -> Result<Vec<u8>, String> {
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Resource, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };

    // Get the underlying D3D11 texture from the WinRT surface.
    let texture: ID3D11Texture2D = surface
        .cast::<ID3D11Resource>()
        .and_then(|r| r.cast::<ID3D11Texture2D>())
        .map_err(|e| format!("surface cast to ID3D11Texture2D failed: {e}"))?;

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };

    // Create a staging texture for CPU readback.
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: crop_w,
        Height: crop_h,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        BindFlags: 0,
        MiscFlags: 0,
    };

    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe {
        d3d_device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| format!("CreateTexture2D (staging) failed: {e}"))?;
    }
    let staging = staging.ok_or("CreateTexture2D returned null")?;

    // CopySubresourceRegion to crop and transfer to the staging texture.
    let src_box = D3D11_BOX {
        left: crop_x.max(0) as u32,
        top: crop_y.max(0) as u32,
        front: 0,
        right: (crop_x.max(0) as u32 + crop_w).min(desc.Width),
        bottom: (crop_y.max(0) as u32 + crop_h).min(desc.Height),
        back: 1,
    };

    let mut context: Option<windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext> = None;
    unsafe {
        d3d_device.GetImmediateContext(&mut context);
    }
    let context = context.ok_or("GetImmediateContext returned null")?;

    unsafe {
        context.CopySubresourceRegion(
            &staging,
            0,
            0,
            0,
            0,
            &texture,
            0,
            Some(&src_box),
        );
    }

    // Map the staging texture and read pixel bytes.
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("Map failed: {e}"))?;
    }

    let row_pitch = mapped.RowPitch as usize;
    let mut rgba = Vec::with_capacity((crop_w * crop_h * 4) as usize);

    for row in 0..crop_h as usize {
        let row_start = row * row_pitch;
        let row_bytes =
            unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, row_pitch * crop_h as usize) };
        let row_slice = &row_bytes[row_start..row_start + (crop_w as usize * 4)];

        // WGC delivers BGRA — swap B↔R to produce RGBA.
        for bgra in row_slice.chunks_exact(4) {
            rgba.push(bgra[2]); // R
            rgba.push(bgra[1]); // G
            rgba.push(bgra[0]); // B
            rgba.push(bgra[3]); // A
        }
    }

    unsafe { context.Unmap(&staging, 0) };

    Ok(rgba)
}

// ── stub for non-Windows builds (keeps the module from breaking on macOS CI) ──

#[cfg(not(target_os = "windows"))]
fn capture_via_wgc(_config: &CaptureConfig) -> Result<Vec<CapturedFrame>, String> {
    Err("capture_via_wgc: not on Windows".into())
}
