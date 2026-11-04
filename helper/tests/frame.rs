//! Screenshots and clicks share the visible window frame (DWM extended frame bounds),
//! not GetWindowRect with its invisible resize borders.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use image::RgbImage;
use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// Edge targets painted by `fixture-window --frame` inside the client area, with their
/// RGB colors. The three top targets sit on the caption and are not painted. The
/// fixture turns off Windows 11 corner rounding so the corner targets stay visible.
const PAINTED: [(&str, [u8; 3]); 5] = [
    ("ml", [255, 255, 0]),
    ("mr", [255, 0, 255]),
    ("bl", [0, 255, 255]),
    ("bm", [255, 128, 0]),
    ("br", [0, 255, 0]),
];

#[test]
fn framed_window_screenshot_matches_the_visible_frame_and_edge_targets_are_clickable() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    dpi_aware();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--frame").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = hwnd_of(&window_id);
    let frame = visible_frame(hwnd);
    let outer = window_rect(hwnd);
    assert_ne!(
        (outer.left, outer.right, outer.bottom),
        (frame.left, frame.right, frame.bottom),
        "the framed fixture must have invisible resize borders, or this test proves nothing"
    );

    let (width, height) = assert_geometry_is_the_visible_frame(&mut helper, &window_id, frame);
    let full = capture(&mut helper, &window_id, width, height);
    assert_painted_targets_are_in_place(&full);
    let (small_width, small_height) = (width * 2 / 3, height * 2 / 3);
    let small = capture(&mut helper, &window_id, small_width, small_height);
    assert_painted_targets_are_in_place(&small);

    click_every_edge_target(&mut helper, &window_id, hwnd, &full);
    drop(helper);
    drop(fixture);
}

#[test]
fn maximized_window_screenshot_stays_on_screen_and_edge_targets_are_clickable() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    dpi_aware();
    let fixture = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--frame", "--maximized"]).spawn().expect("fixture"),
    );
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = hwnd_of(&window_id);
    let monitor = monitor_rect(hwnd);
    let frame = visible_frame(hwnd);
    let (width, height) = assert_geometry_is_the_visible_frame(&mut helper, &window_id, frame);
    assert!(
        frame.left >= monitor.left && frame.top >= monitor.top && frame.right <= monitor.right && frame.bottom <= monitor.bottom,
        "maximized frame {frame:?} is not inside the monitor {monitor:?}"
    );

    let shot = capture(&mut helper, &window_id, width, height);
    assert!(
        shot.original_width <= monitor.right - monitor.left && shot.original_height <= monitor.bottom - monitor.top,
        "maximized screenshot {}x{} is larger than the monitor {monitor:?}",
        shot.original_width,
        shot.original_height
    );
    assert_painted_targets_are_in_place(&shot);
    click_every_edge_target(&mut helper, &window_id, hwnd, &shot);
    drop(helper);
    drop(fixture);
}

#[test]
fn a_crop_outside_the_captured_frame_is_out_of_bounds() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    dpi_aware();
    let fixture = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--frame", "--no-activate"]).spawn().expect("fixture"),
    );
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().expect("width");
    let height = info["result"]["height"].as_i64().expect("height");

    let inside = helper.request(
        "capture_window",
        json!({ "window_id": window_id, "target_width": 20, "target_height": 10, "crop": { "x": width - 20, "y": height - 10, "width": 20, "height": 10 } }),
    );
    assert_eq!(inside["ok"], true, "a crop touching the far edges is inside the frame: {inside}");
    let beyond = helper.request(
        "capture_window",
        json!({ "window_id": window_id, "target_width": 20, "target_height": 10, "crop": { "x": width - 10, "y": 0, "width": 20, "height": 10 } }),
    );
    drop(helper);
    drop(fixture);
    assert_eq!(beyond["ok"], false, "a crop past the right edge must not fall back to the whole window: {beyond}");
    assert_eq!(beyond["error"]["code"], "out_of_bounds", "{beyond}");
}

struct Shot {
    image: RgbImage,
    original_width: i32,
    original_height: i32,
}

impl Shot {
    /// Image pixel for a frame-relative pixel, using the same ratio the server uses.
    fn to_image(&self, x: i32, y: i32) -> (u32, u32) {
        let ix = (x as f64 * self.image.width() as f64 / self.original_width as f64).floor() as u32;
        let iy = (y as f64 * self.image.height() as f64 / self.original_height as f64).floor() as u32;
        (ix.min(self.image.width() - 1), iy.min(self.image.height() - 1))
    }

    /// Window-relative pixel for an image pixel, as the server's `imageToWindow` does.
    fn to_window(&self, x: u32, y: u32) -> (i64, i64) {
        (
            (x as f64 * self.original_width as f64 / self.image.width() as f64).round() as i64,
            (y as f64 * self.original_height as f64 / self.image.height() as f64).round() as i64,
        )
    }
}

fn assert_geometry_is_the_visible_frame(helper: &mut Helper, window_id: &str, frame: RECT) -> (i32, i32) {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    assert_eq!(info["ok"], true, "{info}");
    let reported = (
        info["result"]["x"].as_i64().unwrap_or(-1) as i32,
        info["result"]["y"].as_i64().unwrap_or(-1) as i32,
        info["result"]["width"].as_i64().unwrap_or(-1) as i32,
        info["result"]["height"].as_i64().unwrap_or(-1) as i32,
    );
    let expected = (frame.left, frame.top, frame.right - frame.left, frame.bottom - frame.top);
    assert_eq!(reported, expected, "get_window must report the visible frame (x, y, width, height)");
    (expected.2, expected.3)
}

fn capture(helper: &mut Helper, window_id: &str, target_width: i32, target_height: i32) -> Shot {
    let shot = helper.request(
        "capture_window",
        json!({ "window_id": window_id, "target_width": target_width, "target_height": target_height }),
    );
    assert_eq!(shot["ok"], true, "{shot}");
    let original_width = shot["result"]["original_width"].as_i64().expect("original_width") as i32;
    let original_height = shot["result"]["original_height"].as_i64().expect("original_height") as i32;
    let encoded = shot["result"]["image_base64"].as_str().expect("image");
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).expect("base64");
    let image = image::load_from_memory(&bytes).expect("jpeg").to_rgb8();    assert_eq!(
        (shot["result"]["image_width"].as_i64(), shot["result"]["image_height"].as_i64()),
        (Some(image.width() as i64), Some(image.height() as i64)),
        "capture_window must report the size of the image it returned"
    );
    assert!(image.width() as i32 <= target_width && image.height() as i32 <= target_height, "image is larger than the target");
    // Uniform scale: width and height shrink by the same ratio, up to one pixel of rounding.
    let skew = (image.width() as i64 * original_height as i64 - image.height() as i64 * original_width as i64).abs();
    assert!(
        skew <= original_width.max(original_height) as i64,
        "image {}x{} is not a uniform scale of the frame {original_width}x{original_height}",
        image.width(),
        image.height()
    );
    Shot { image, original_width, original_height }
}

fn assert_painted_targets_are_in_place(shot: &Shot) {
    let (width, height) = (shot.original_width, shot.original_height);
    for (name, color) in PAINTED {
        let (x, y) = target_center(name, width, height);
        let (ix, iy) = shot.to_image(x, y);
        let pixel = shot.image.get_pixel(ix, iy).0;
        let distance: i32 = pixel.iter().zip(color).map(|(have, want)| (*have as i32 - want as i32).pow(2)).sum();
        assert!(
            distance <= 110 * 110,
            "target {name} should be {color:?} at image ({ix}, {iy}) of {}x{}, found {pixel:?}",
            shot.image.width(),
            shot.image.height()
        );
    }
}

/// Center of a `TARGET` (8 px) edge target, frame-relative.
fn target_center(name: &str, width: i32, height: i32) -> (i32, i32) {
    match name {
        "tl" => (4, 4),
        "tm" => (width / 2, 4),
        "tr" => (width - 4, 4),
        "ml" => (4, height / 2),
        "mr" => (width - 4, height / 2),
        "bl" => (4, height - 4),
        "bm" => (width / 2, height - 4),
        "br" => (width - 4, height - 4),
        _ => unreachable!(),
    }
}

/// Clicks each target at an image pixel no more than 2 px from the image edge.
fn click_every_edge_target(helper: &mut Helper, window_id: &str, hwnd: HWND, shot: &Shot) {
    let (w, h) = (shot.image.width(), shot.image.height());
    let points = [
        ("tl", 2, 2),
        ("tm", w / 2, 2),
        ("tr", w - 3, 2),
        ("ml", 2, h / 2),
        ("mr", w - 3, h / 2),
        ("bl", 2, h - 3),
        ("bm", w / 2, h - 3),
        ("br", w - 3, h - 3),
    ];
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    for (name, ix, iy) in points {
        assert_eq!(unsafe { GetForegroundWindow() }, hwnd, "the fixture is not in the foreground; no input sent for {name}");
        let (x, y) = shot.to_window(ix, iy);
        let clicked = helper.request("click", json!({ "window_id": window_id, "x": x, "y": y }));
        assert_eq!(clicked["ok"], true, "click on {name} failed: {clicked}");
        let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
        let events = title.rsplit(" | ev=").next().unwrap_or("");
        let last = events.rsplit(',').next().unwrap_or("");
        assert!(
            last.starts_with(&format!("hit:{name}@")),
            "clicking image ({ix}, {iy}) -> window ({x}, {y}) should hit {name}; fixture recorded {last:?} (title {title})"
        );
    }
}

fn dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn hwnd_of(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
}

fn visible_frame(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut RECT as *mut _, size_of::<RECT>() as u32)
            .expect("extended frame bounds");
    }
    rect
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).expect("window rect") };
    rect
}

fn monitor_rect(hwnd: HWND) -> RECT {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        assert!(GetMonitorInfoW(monitor, &mut info).as_bool(), "monitor info");
        info.rcMonitor
    }
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).and_then(|_| window["window_id"].as_str().map(str::to_string))
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture was not listed");
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else { return false };
        let _ = CloseDesktop(desktop);
        true
    }
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Helper {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("helper");
        let stdout = child.stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        Self { child, reader, next_id: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("response");
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
            if value.get("event").is_some() {
                continue;
            }
            return value;
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
