use std::time::{Duration, Instant};

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::{imageops, ExtendedColorType, RgbaImage};
use serde_json::{json, Value};
use windows::core::{factory, Interface};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GdiFlush, GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GetWindowRect, GW_OWNER};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;

use crate::popups::{self, Popup};
use crate::windows_list::{self, parse_hwnd};

pub fn capture_window(params: &Value) -> Result<Value, (String, String)> {
    let window_id = params.get("window_id").and_then(|value| value.as_str()).unwrap_or("");
    let target_width = positive_i32(params.get("target_width"))?;
    let target_height = positive_i32(params.get("target_height"))?;
    let hwnd = parse_hwnd(window_id).ok_or_else(|| ("invalid_params".to_string(), "window_id is not a window handle.".to_string()))?;
    let info = windows_list::get_window(window_id)?;
    windows_list::check_expected_app(hwnd, params)?;
    if info.get("minimized").and_then(|value| value.as_bool()) == Some(true) {
        return Err((
            "window_minimized".to_string(),
            "The window is minimized. Call computer_activate_window before taking a screenshot.".to_string(),
        ));
    }
    let window_width = info.get("width").and_then(|value| value.as_i64()).unwrap_or(0);
    let window_height = info.get("height").and_then(|value| value.as_i64()).unwrap_or(0);
    if window_width <= 0 || window_height <= 0 {
        return Err(("capture_failed".to_string(), "The window has no visible area.".to_string()));
    }
    let crop = parse_crop(params.get("crop"))?;
    let window_x = info.get("x").and_then(|value| value.as_i64()).unwrap_or(0);
    let window_y = info.get("y").and_then(|value| value.as_i64()).unwrap_or(0);
    let frame = RECT { left: window_x as i32, top: window_y as i32, right: (window_x + window_width) as i32, bottom: (window_y + window_height) as i32 };

    let bgra = unsafe { capture_bgra(hwnd, WINDOW_FRAME_TIMEOUT) }.map_err(|error| ("capture_failed".to_string(), error))?;
    let target = RgbaImage::from_raw(bgra.width, bgra.height, bgra.pixels).ok_or_else(|| {
        ("capture_failed".to_string(), "Captured pixels did not match the frame size.".to_string())
    })?;
    // The captured frame is the truth for pixel positions. It normally equals the visible
    // frame that get_window reports; when it is off by a pixel or two, keep the frame's own
    // size so image-to-window conversion stays exact.
    if target.width() as i64 != window_width || target.height() as i64 != window_height {
        eprintln!(
            "captured frame is {}x{} but the visible frame is {window_width}x{window_height}; using the captured frame",
            target.width(),
            target.height()
        );
    }
    // The target's own popups (menus, dropdowns, tooltips) that overlap or touch it join it on
    // one canvas. Each is captured on its own, so no other app's pixels enter the image. When
    // one cannot be captured, the image is the target alone and the result says so.
    let popups = popups::target_popups(hwnd, frame);
    let (mut image, origin, shown, not_captured) = match compose(&target, frame, &popups) {
        Ok((canvas, origin)) => (canvas, origin, popups.iter().collect::<Vec<_>>(), None),
        Err(message) => (target, (0, 0), Vec::new(), Some(message)),
    };
    let (original_width, original_height) = (image.width(), image.height());
    let expected = match crop {
        Some(crop) => {
            // The crop is in window pixels, which start left of and above the canvas when it
            // reaches past the target's left or top edge.
            let (x, y) = (i64::from(crop.x) - origin.0, i64::from(crop.y) - origin.1);
            if x < 0 || y < 0 || x + i64::from(crop.width) > i64::from(original_width) || y + i64::from(crop.height) > i64::from(original_height) {
                return Err((
                    "out_of_bounds".to_string(),
                    format!(
                        "The zoom region {}x{} at ({}, {}) is outside the {original_width}x{original_height} window capture. Take a new screenshot.",
                        crop.width, crop.height, crop.x, crop.y
                    ),
                ));
            }
            image = imageops::crop_imm(&image, x as u32, y as u32, crop.width, crop.height).to_image();
            (crop.width, crop.height)
        }
        None => (window_width as u32, window_height as u32),
    };
    let (image_width, image_height) = output_size((image.width(), image.height()), expected, (target_width as u32, target_height as u32));
    if image.width() != image_width || image.height() != image_height {
        image = imageops::resize(&image, image_width, image_height, imageops::FilterType::Triangle);
    }
    let encoded = encode_jpeg(&image).map_err(|error| ("capture_failed".to_string(), error))?;
    let mut result = json!({
        "image_base64": base64::engine::general_purpose::STANDARD.encode(encoded),
        "image_width": image_width,
        "image_height": image_height,
        "original_width": original_width,
        "original_height": original_height,
        "window_x": window_x,
        "window_y": window_y,
        "window_width": window_width,
        "window_height": window_height,
    });
    let relative = |rect: &RECT| json!({ "x": rect.left - frame.left, "y": rect.top - frame.top, "width": rect.right - rect.left, "height": rect.bottom - rect.top });
    if !shown.is_empty() {
        result["origin_x"] = json!(origin.0);
        result["origin_y"] = json!(origin.1);
        result["popups"] = json!(shown
            .iter()
            .map(|popup| {
                let mut described = relative(&popup.frame);
                described["kind"] = json!(popup.kind);
                described
            })
            .collect::<Vec<_>>());
    }
    if let Some(message) = not_captured {
        result["popups_not_captured"] = json!({ "kinds": popups.iter().map(|popup| popup.kind).collect::<Vec<_>>(), "message": message });
    }
    // Over the whole canvas, in window pixels like everything else.
    let canvas = RECT {
        left: frame.left + origin.0 as i32,
        top: frame.top + origin.1 as i32,
        right: frame.left + origin.0 as i32 + original_width as i32,
        bottom: frame.top + origin.1 as i32 + original_height as i32,
    };
    if let Some(parts) = visible_parts(canvas) {
        result["visible_parts"] = json!(parts.iter().map(relative).collect::<Vec<_>>());
    }
    Ok(result)
}

/// The target's image and its popups' on one canvas covering all of them, and the canvas origin
/// in window pixels (zero or negative). Without popups the canvas is the target image itself.
/// Popups are drawn back to front over the target; canvas pixels none of them covers are
/// `FILL`. Fails, naming the popup, when one cannot be captured.
fn compose(target: &RgbaImage, frame: RECT, popups: &[Popup]) -> Result<(RgbaImage, (i64, i64)), String> {
    if popups.is_empty() {
        return Ok((target.clone(), (0, 0)));
    }
    let target_rect = RECT { left: frame.left, top: frame.top, right: frame.left + target.width() as i32, bottom: frame.top + target.height() as i32 };
    let mut captured = Vec::new();
    for popup in popups {
        let bgra = unsafe { capture_popup(popup.hwnd) }.map_err(|error| format!("the {} could not be captured ({error})", popup.kind))?;
        let image = RgbaImage::from_raw(bgra.width, bgra.height, bgra.pixels).ok_or_else(|| format!("the {} capture did not match its size", popup.kind))?;
        captured.push((popup.frame, image));
    }
    let union = captured.iter().fold(target_rect, |union, (rect, _)| RECT {
        left: union.left.min(rect.left),
        top: union.top.min(rect.top),
        right: union.right.max(rect.right),
        bottom: union.bottom.max(rect.bottom),
    });
    let mut canvas = RgbaImage::from_pixel((union.right - union.left) as u32, (union.bottom - union.top) as u32, image::Rgba(FILL));
    imageops::replace(&mut canvas, target, i64::from(target_rect.left - union.left), i64::from(target_rect.top - union.top));
    // `popups` are front to back, so the frontmost is drawn last.
    for (rect, image) in captured.iter().rev() {
        imageops::replace(&mut canvas, image, i64::from(rect.left - union.left), i64::from(rect.top - union.top));
    }
    Ok((canvas, (i64::from(union.left - frame.left), i64::from(union.top - frame.top))))
}

/// Color of the canvas where neither the target nor one of its popups is.
const FILL: [u8; 4] = [0, 0, 0, 255];

/// Windows Graphics Capture refuses unowned menus (`#32768`); PrintWindow renders them.
unsafe fn capture_popup(hwnd: HWND) -> Result<BgraFrame, String> {
    match capture_bgra(hwnd, POPUP_FRAME_TIMEOUT) {
        Ok(frame) => Ok(frame),
        Err(wgc) => print_window(hwnd).map_err(|printed| format!("Windows Graphics Capture: {wgc}; PrintWindow: {printed}")),
    }
}

/// Every monitor's rectangle, in physical screen pixels: what the screenshot's `visible_parts` and
/// an element click (`uia::Placement`) count as on a screen.
pub(crate) fn monitor_rects() -> Vec<RECT> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let monitors = &mut *(data.0 as *mut Vec<RECT>);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            monitors.push(info.rcMonitor);
        }
        true.into()
    }
    let mut monitors: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut monitors as *mut Vec<RECT> as isize));
    }
    monitors
}

/// The parts of `frame` (screen pixels) that are on a monitor, one per monitor it overlaps, or
/// `None` when all of it is on screen. Pointer input at the rest is refused (`offscreen`), so the
/// screenshot names what can be used. Monitors do not overlap, so the parts cover the whole frame
/// exactly when their areas add up to its area.
fn visible_parts(frame: RECT) -> Option<Vec<RECT>> {
    let area = |rect: &RECT| (rect.right - rect.left) as i64 * (rect.bottom - rect.top) as i64;
    let parts: Vec<RECT> = monitor_rects()
        .iter()
        .map(|monitor| RECT {
            left: frame.left.max(monitor.left),
            top: frame.top.max(monitor.top),
            right: frame.right.min(monitor.right),
            bottom: frame.bottom.min(monitor.bottom),
        })
        .filter(|part| part.right > part.left && part.bottom > part.top)
        .collect();
    if parts.iter().map(area).sum::<i64>() >= area(&frame) {
        return None;
    }
    Some(parts)
}

/// Size of the returned image. The server picks the target from the size it expects the
/// source to be; when the source really has that size the target is used as is. When the
/// captured frame differs, scale it uniformly to fit inside the target instead of
/// stretching one axis more than the other.
fn output_size(source: (u32, u32), expected: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    if source == expected {
        return target;
    }
    let scale = (target.0 as f64 / source.0 as f64).min(target.1 as f64 / source.1 as f64).min(1.0);
    let width = ((source.0 as f64 * scale).round() as u32).clamp(1, target.0);
    let height = ((source.1 as f64 * scale).round() as u32).clamp(1, target.1);
    (width, height)
}

/// A zoom region in window pixels: relative to the target's visible frame, so negative where it
/// lies on a popup left of or above the target.
#[derive(Clone, Copy)]
struct Crop {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn parse_crop(value: Option<&Value>) -> Result<Option<Crop>, (String, String)> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let signed = |name: &str| value.get(name).and_then(|item| item.as_i64()).and_then(|item| i32::try_from(item).ok());
    let size = |name: &str| value.get(name).and_then(|item| item.as_u64()).and_then(|item| u32::try_from(item).ok());
    let (Some(x), Some(y), Some(width), Some(height)) = (signed("x"), signed("y"), size("width"), size("height")) else {
        return Err(("invalid_params".to_string(), "crop needs integer x and y and non-negative integer width and height.".to_string()));
    };
    if width == 0 || height == 0 {
        return Err(("invalid_params".to_string(), "crop width and height must be positive.".to_string()));
    }
    Ok(Some(Crop { x, y, width, height }))
}

fn positive_i32(value: Option<&Value>) -> Result<i32, (String, String)> {
    let Some(number) = value.and_then(|item| item.as_i64()) else {
        return Err(("invalid_params".to_string(), "target width and height are required.".to_string()));
    };
    if number <= 0 || number > i32::MAX as i64 {
        return Err(("invalid_params".to_string(), "target width and height must be positive.".to_string()));
    }
    Ok(number as i32)
}

fn encode_jpeg(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut rgb = Vec::with_capacity(image.width() as usize * image.height() as usize * 3);
    for pixel in image.pixels() {
        rgb.extend_from_slice(&pixel.0[..3]);
    }
    let mut output = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut output, 75);
    encoder
        .encode(&rgb, image.width(), image.height(), ExtendedColorType::Rgb8)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

struct BgraFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// How long Windows Graphics Capture may take to deliver the target's first frame.
const WINDOW_FRAME_TIMEOUT: Duration = Duration::from_secs(3);
/// How long it may take for a popup's: one it accepts but never draws falls back to PrintWindow
/// soon instead of holding up the screenshot.
const POPUP_FRAME_TIMEOUT: Duration = Duration::from_millis(500);

/// Captures `hwnd` with Windows Graphics Capture, waiting up to `frame_timeout` for a frame.
unsafe fn capture_bgra(hwnd: HWND, frame_timeout: Duration) -> Result<BgraFrame, String> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|error| error.to_string())?;
    let item: GraphicsCaptureItem = match interop.CreateForWindow(hwnd) {
        Ok(item) => item,
        // Windows Graphics Capture refuses every owned window (E_INVALIDARG), which is what
        // an app's dialogs are. Render the dialog itself instead.
        Err(error) if GetWindow(hwnd, GW_OWNER).is_ok() => {
            eprintln!("Windows Graphics Capture refused owned window {:?} ({error}); using PrintWindow", hwnd.0);
            return print_window(hwnd);
        }
        Err(error) => return Err(error.to_string()),
    };
    let (device, context) = create_device()?;
    let dxgi: IDXGIDevice = device.cast().map_err(|error| error.to_string())?;
    let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(|error| error.to_string())?;
    let winrt_device: IDirect3DDevice = inspectable.cast().map_err(|error| error.to_string())?;
    let size: SizeInt32 = item.Size().map_err(|error| error.to_string())?;
    if size.Width <= 0 || size.Height <= 0 {
        return Err("Capture item has no size.".to_string());
    }
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(|error| error.to_string())?;
    // Guards close the pool, session and frame on every way out, failures included, in the
    // reverse order of their creation.
    let _pool_closed = OnDrop(|| {
        let _ = pool.Close();
    });
    let session = pool.CreateCaptureSession(&item).map_err(|error| error.to_string())?;
    let _session_closed = OnDrop(|| {
        let _ = session.Close();
    });
    if let Err(error) = session.SetIsBorderRequired(false) {
        eprintln!("capture border could not be disabled: {error}");
    }
    // Windows 10 2004+; an older system fails the call and keeps capturing the cursor.
    if let Err(error) = session.SetIsCursorCaptureEnabled(false) {
        eprintln!("capture cursor could not be disabled: {error}");
    }
    session.StartCapture().map_err(|error| error.to_string())?;

    let started = Instant::now();
    let frame = loop {
        match pool.TryGetNextFrame() {
            Ok(frame) => break frame,
            Err(error) if started.elapsed() < frame_timeout => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(error) => return Err(format!("No capture frame: {error}")),
        }
    };
    let _frame_closed = OnDrop(|| {
        let _ = frame.Close();
    });
    let content: SizeInt32 = frame.ContentSize().map_err(|error| error.to_string())?;
    let surface = frame.Surface().map_err(|error| error.to_string())?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|error| error.to_string())?;
    let texture: ID3D11Texture2D = access.GetInterface().map_err(|error| error.to_string())?;
    // The texture keeps the pool size; the window's pixels fill only the content area.
    copy_texture(&device, &context, &texture, content)
}

/// Runs its closure when dropped.
struct OnDrop<F: FnMut()>(F);

impl<F: FnMut()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}

/// Renders `hwnd` itself with PrintWindow (full content, so other windows and the overlay
/// never appear) and keeps the part inside its visible frame: the same area and size that
/// Windows Graphics Capture returns for an unowned window.
unsafe fn print_window(hwnd: HWND) -> Result<BgraFrame, String> {
    const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);
    let mut outer = RECT::default();
    GetWindowRect(hwnd, &mut outer).map_err(|error| error.to_string())?;
    let frame = windows_list::visible_frame(hwnd)?;
    let (outer_width, outer_height) = (outer.right - outer.left, outer.bottom - outer.top);
    if outer_width <= 0 || outer_height <= 0 {
        return Err("The window has no size.".to_string());
    }
    let left = (frame.left - outer.left).clamp(0, outer_width);
    let top = (frame.top - outer.top).clamp(0, outer_height);
    let width = (frame.right - frame.left).min(outer_width - left);
    let height = (frame.bottom - frame.top).min(outer_height - top);
    if width <= 0 || height <= 0 {
        return Err("The window has no visible area.".to_string());
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: outer_width,
            biHeight: -outer_height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let screen = GetDC(None);
    let memory = CreateCompatibleDC(Some(screen));
    let mut bits = std::ptr::null_mut();
    let result = match CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
        Err(error) => Err(error.to_string()),
        Ok(bitmap) => {
            let previous = SelectObject(memory, bitmap.into());
            let printed = PrintWindow(hwnd, memory, PW_RENDERFULLCONTENT).as_bool();
            let _ = GdiFlush();
            let pixels = if printed && !bits.is_null() {
                let source = std::slice::from_raw_parts(bits as *const u8, outer_width as usize * outer_height as usize * 4);
                let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
                for row in top..top + height {
                    let start = (row * outer_width + left) as usize * 4;
                    for bgra in source[start..start + width as usize * 4].chunks_exact(4) {
                        pixels.extend_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
                    }
                }
                Ok(BgraFrame { width: width as u32, height: height as u32, pixels })
            } else {
                Err("PrintWindow could not render the window.".to_string())
            };
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap.into());
            pixels
        }
    };
    let _ = DeleteDC(memory);
    ReleaseDC(None, screen);
    result
}

unsafe fn create_device() -> Result<(ID3D11Device, windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext), String> {
    let mut device = None;
    let mut context = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        windows::Win32::Foundation::HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        Some(&mut context),
    )
    .map_err(|error| error.to_string())?;
    Ok((device.ok_or_else(|| "D3D11 device was not created.".to_string())?, context.ok_or_else(|| "D3D11 context was not created.".to_string())?))
}

unsafe fn copy_texture(
    device: &ID3D11Device,
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
    content: SizeInt32,
) -> Result<BgraFrame, String> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    texture.GetDesc(&mut desc);
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    desc.SampleDesc.Count = 1;
    desc.SampleDesc.Quality = 0;
    let mut staging = None;
    device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(|error| error.to_string())?;
    let staging = staging.ok_or_else(|| "Staging texture was not created.".to_string())?;
    context.CopyResource(&staging, texture);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|error| error.to_string())?;
    let width = desc.Width.min(content.Width.max(1) as u32);
    let height = desc.Height.min(content.Height.max(1) as u32);
    let pitch = mapped.RowPitch as usize;
    let source = std::slice::from_raw_parts(mapped.pData as *const u8, pitch.saturating_mul(desc.Height as usize));
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let start = row * pitch;
        for col in 0..width as usize {
            let index = start + col * 4;
            let blue = source[index];
            let green = source[index + 1];
            let red = source[index + 2];
            let alpha = source[index + 3];
            pixels.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    context.Unmap(&staging, 0);
    Ok(BgraFrame { width, height, pixels })
}
