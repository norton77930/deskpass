//! Ticket 143: while the target is out of front (a background action), the border is drawn only on
//! the part of the target the user can see: the overlay is clipped to the target's frame minus the
//! visible top-level windows above it, follows changes of that part while it shows, and draws
//! nothing when the target is fully covered. A target in front gets the whole border, as before.
//! The user's window is an ordinary, activatable fixture of another process holding the
//! foreground (ticket 151). The overlay is excluded from screen capture, so the tests first ask
//! the helper (test builds only) to leave it in, then compare screen captures.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GetWindowRect, MoveWindow, WindowFromPoint, GA_ROOT};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// `helper/src/overlay.rs`'s border width.
const BORDER: i32 = 6;
/// Where the target is put, (left, top, width, height) in screen pixels.
const TARGET: (i32, i32, i32, i32) = (200, 200, 400, 300);

/// The user's window covers the right half of the target (and the border around it): the border
/// shows on the left side and never over the user's window.
#[test]
fn a_half_covered_target_gets_the_border_only_on_the_uncovered_side() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    let target = scene.target_rect();
    let cover = scene.cover((target.left + (target.right - target.left) / 2, target.top - 100, 300, (target.bottom - target.top) + 200));
    scene.show_border();

    let left = left_strip(target);
    assert!(wait_until(|| green_share(left) > 0.95), "no border on the uncovered left side: {:.2} green", green_share(left));
    let top_left = RECT { left: target.left, top: target.top - BORDER, right: cover.left, bottom: target.top };
    assert!(green_share(top_left) > 0.95, "no border on the uncovered part of the top: {:.2} green", green_share(top_left));
    assert_never_green(cover, "the border is drawn over the user's window covering the right half");
}

/// When the user's window moves off the target, the border fills in the part it covered.
#[test]
fn the_border_fills_in_after_the_cover_moves_away() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    let target = scene.target_rect();
    let cover = scene.cover((target.left + (target.right - target.left) / 2, target.top - 100, 300, (target.bottom - target.top) + 200));
    scene.show_border();
    assert!(wait_until(|| green_share(left_strip(target)) > 0.95), "no border on the uncovered left side");
    assert_never_green(cover, "the border is drawn over the user's window covering the right half");

    scene.move_cover((target.right + 150, target.top - 100, 300, (target.bottom - target.top) + 200));
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&scene.user_id), "the user's window lost the foreground");
    for (side, strip) in strips(target) {
        assert!(wait_until(|| green_share(strip) > 0.95), "the {side} border did not fill in after the cover moved away: {:.2} green", green_share(strip));
    }
}

/// A target the user's window covers entirely gets no border pixels at all over that window; once
/// the window moves off, the border appears (so the capture would have shown it).
#[test]
fn a_fully_covered_target_draws_no_border_over_the_cover() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    let target = scene.target_rect();
    let cover = scene.cover((target.left - 100, target.top - 100, (target.right - target.left) + 200, (target.bottom - target.top) + 200));
    scene.show_border();
    assert_never_green(cover, "the border is drawn over the user's window covering the whole target");

    scene.move_cover((target.right + 150, target.top - 100, 300, (target.bottom - target.top) + 200));
    for (side, strip) in strips(target) {
        assert!(wait_until(|| green_share(strip) > 0.95), "the {side} border did not appear after the cover moved away: {:.2} green", green_share(strip));
    }
}

/// A target in front (a foreground action) gets the whole border, as before ticket 143.
#[test]
fn a_target_in_front_gets_the_whole_border() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    let target = scene.target_rect();
    scene.cover((target.right + 150, target.top - 100, 300, (target.bottom - target.top) + 200));
    let activated = scene.helper.request("activate_window", json!({ "window_id": scene.target_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    scene.show_border();
    for (side, strip) in strips(target) {
        assert!(wait_until(|| green_share(strip) > 0.95), "no {side} border around the target in front: {:.2} green", green_share(strip));
    }
}

/// The target fixture at `TARGET`, the user's window (another fixture process), and a helper
/// whose overlay a screen capture shows. The border is hidden when dropped.
struct Scene {
    helper: Helper,
    target_id: String,
    user_id: String,
    _target: KillOnDrop,
    _user: KillOnDrop,
}

impl Scene {
    fn start() -> Self {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let target_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
        let mut helper = Helper::spawn();
        let target_id = wait_for_fixture_besides(&mut helper, &[]);
        place_window(&target_id, TARGET);
        let user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("user window"));
        let user_id = wait_for_fixture_besides(&mut helper, &[&target_id]);
        let capturable = helper.request("debug_overlay_capturable", json!({}));
        assert_eq!(capturable["ok"], true, "{capturable}");
        Self { helper, target_id, user_id, _target: target_process, _user: user_process }
    }

    fn target_rect(&self) -> RECT {
        window_rect(hwnd(&self.target_id))
    }

    /// Puts the user's window at `rect` above the target, in front, and answers its frame.
    fn cover(&mut self, rect: (i32, i32, i32, i32)) -> RECT {
        place_window(&self.user_id, rect);
        for id in [self.target_id.clone(), self.user_id.clone()] {
            let activated = self.helper.request("activate_window", json!({ "window_id": id }));
            assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
        }
        assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&self.user_id), "the user's window is not in front");
        let cover = window_rect(hwnd(&self.user_id));
        let target = self.target_rect();
        let inside = POINT { x: target.right - 20, y: target.top + 20 };
        if cover.left < target.right && cover.right > target.left {
            assert_eq!(unsafe { GetAncestor(WindowFromPoint(inside), GA_ROOT) }, hwnd(&self.user_id), "the user's window does not cover the target, so the test proves nothing");
        }
        cover
    }

    fn move_cover(&mut self, rect: (i32, i32, i32, i32)) {
        place_window(&self.user_id, rect);
    }

    fn show_border(&mut self) {
        let shown = self.helper.request("overlay_show", json!({ "window_id": self.target_id }));
        assert_eq!(shown["ok"], true, "{shown}");
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = self.helper.request("overlay_hide", json!({}));
    }
}

fn left_strip(target: RECT) -> RECT {
    RECT { left: target.left - BORDER, top: target.top, right: target.left, bottom: target.bottom }
}

/// The four border strips around `target`, outside its frame.
fn strips(target: RECT) -> [(&'static str, RECT); 4] {
    [
        ("left", left_strip(target)),
        ("right", RECT { left: target.right, top: target.top, right: target.right + BORDER, bottom: target.bottom }),
        ("top", RECT { left: target.left - BORDER, top: target.top - BORDER, right: target.right + BORDER, bottom: target.top }),
        ("bottom", RECT { left: target.left - BORDER, top: target.bottom, right: target.right + BORDER, bottom: target.bottom + BORDER }),
    ]
}

/// Captures `rect` every 40 ms for 800 ms; no capture may show a border pixel there.
fn assert_never_green(rect: RECT, message: &str) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(800) {
        let green = capture_screen(rect).iter().filter(|pixel| is_border(pixel)).count();
        assert_eq!(green, 0, "{message}: {green} border pixels inside {rect:?}");
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if condition() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn green_share(rect: RECT) -> f64 {
    let pixels = capture_screen(rect);
    pixels.iter().filter(|pixel| is_border(pixel)).count() as f64 / pixels.len().max(1) as f64
}

fn is_border(pixel: &[u8; 3]) -> bool {
    pixel[1] > 180 && pixel[0] < 40 && pixel[2] < 40
}

/// Copies a rectangle of the screen the way a GDI screenshot does; one RGB triple per pixel.
fn capture_screen(rect: RECT) -> Vec<[u8; 3]> {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
        BitBlt(memory, 0, 0, width, height, Some(screen), rect.left, rect.top, SRCCOPY).expect("BitBlt");
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(memory, bitmap, 0, height as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        assert_eq!(lines, height, "GetDIBits");
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    bgra.chunks_exact(4).map(|pixel| [pixel[2], pixel[1], pixel[0]]).collect()
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.expect("window rect");
    rect
}

/// Puts the ordinary (activatable) fixture window at `(left, top, width, height)`.
fn place_window(window_id: &str, (left, top, width, height): (i32, i32, i32, i32)) {
    unsafe { MoveWindow(hwnd(window_id), left, top, width, height, true) }.expect("MoveWindow");
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
}

/// The id of a fixture window other than the `seen` ones.
fn wait_for_fixture_besides(helper: &mut Helper, seen: &[&String]) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                let title = window["title"].as_str().unwrap_or("");
                let id = window["window_id"].as_str().unwrap_or("").to_string();
                (title.starts_with("CUMCP_FIXTURE") && !id.is_empty() && !seen.iter().any(|seen| **seen == id)).then_some(id)
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("the fixture was not listed");
}

/// The interactive-desktop lock, or `None` (the test skips) without a desktop.
fn interactive() -> Option<interactive_lock::InteractiveLock> {
    let desktop = unsafe { OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) };
    let Ok(desktop) = desktop else {
        eprintln!("SKIP interactive: no desktop");
        return None;
    };
    let _ = unsafe { CloseDesktop(desktop) };
    Some(interactive_lock::acquire())
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
            .stderr(Stdio::piped())
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
