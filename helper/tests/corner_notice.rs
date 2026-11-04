//! Ticket 144: the corner notice beside the border, "AI is operating ‹App› · stop key", in a corner of
//! the work area of the monitor the target is on. It names the app (the name the server's approval
//! showed, `app_name`) and the stop key of the delivery (`Ctrl+Alt+Esc` in the background,
//! `Esc` in the foreground). It takes no focus, lets clicks through, stays out of screen
//! capture like the border, and shows and hides with the border. The user's window is an ordinary,
//! activatable fixture of another process holding the foreground (ticket 151).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::w;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, MonitorFromWindow, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetAncestor, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MoveWindow, SetCursorPos,
    WindowFromPoint, GA_ROOT,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// Where the target is put, (left, top, width, height) in screen pixels.
const TARGET: (i32, i32, i32, i32) = (200, 200, 400, 300);
const APP: &str = "示範程式";

/// The notice names the app and the stop key of the delivery, sits in a corner of the work area
/// of the target's monitor, and the user's window keeps the foreground throughout.
#[test]
fn the_notice_names_the_app_and_the_stop_key_of_the_delivery() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    let user = hwnd(&scene.user_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window is not in front");

    scene.show("background");
    let notice = scene.notice();
    assert!(wait_until(|| window_text(notice) == format!("AI 正在操作 {APP} · Ctrl+Alt+Esc 停止")), "background text: {:?}", window_text(notice));
    assert!(unsafe { IsWindowVisible(notice) }.as_bool(), "the notice is not shown");
    assert_foreground_stays(user, "after the background notice appeared");

    let work = work_area(hwnd(&scene.target_id));
    let placed = window_rect(notice);
    assert!(
        placed.left >= work.left && placed.top >= work.top && placed.right <= work.right && placed.bottom <= work.bottom,
        "the notice {placed:?} is not inside the work area {work:?}"
    );
    assert!(work.right - placed.right <= 64 && work.bottom - placed.bottom <= 64, "the notice {placed:?} is not in the bottom-right corner of {work:?}");

    scene.show("foreground");
    assert!(wait_until(|| window_text(notice) == format!("AI 正在操作 {APP} · Esc 停止")), "foreground text: {:?}", window_text(notice));
    assert_eq!(scene.notice(), notice, "the notice is one window, updated in place");
    assert_foreground_stays(user, "after the foreground notice appeared");
}

/// A click at the notice lands on the window below it, and the foreground does not change.
#[test]
fn a_click_at_the_notice_lands_on_the_window_below() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    scene.show("background");
    let notice = scene.notice();
    assert!(wait_until(|| unsafe { IsWindowVisible(notice) }.as_bool()), "the notice is not shown");
    let placed = window_rect(notice);
    let center = POINT { x: (placed.left + placed.right) / 2, y: (placed.top + placed.bottom) / 2 };

    // The user's window is moved so that its button lies under the notice's center.
    let user = hwnd(&scene.user_id);
    let button = unsafe { FindWindowExW(Some(user), None, w!("Button"), w!("Click me")) }.expect("the fixture's button");
    let (frame, pressed) = (window_rect(user), window_rect(button));
    let offset = ((pressed.left + pressed.right) / 2 - frame.left, (pressed.top + pressed.bottom) / 2 - frame.top);
    place_window(&scene.user_id, (center.x - offset.0, center.y - offset.1, frame.right - frame.left, frame.bottom - frame.top));
    let activated = scene.helper.request("activate_window", json!({ "window_id": scene.user_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(center), GA_ROOT) }, user, "the user's window is not under the notice, so the test proves nothing");
    assert!(unsafe { IsWindowVisible(notice) }.as_bool() && window_rect(notice) == placed, "the notice moved");

    unsafe {
        SetCursorPos(center.x, center.y).expect("SetCursorPos");
        let inputs = [mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)];
        assert_eq!(SendInput(&inputs, size_of::<INPUT>() as i32), 2, "SendInput");
    }
    let id = scene.user_id.clone();
    assert!(wait_until(|| scene.title(&id).contains("click:left")), "the click at the notice did not reach the window below: {}", scene.title(&id));
    assert_foreground_stays(user, "after clicking at the notice");
}

/// A screen capture of the notice's part of the desktop, the way a GDI screenshot copies the
/// screen, has none of its pixels. Then the helper (test builds only) is asked to leave the
/// overlay in the capture: the same check sees the notice, so it would have seen a leak.
#[test]
fn the_notice_is_left_out_of_a_screen_capture() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    scene.show("background");
    let notice = scene.notice();
    assert!(wait_until(|| unsafe { IsWindowVisible(notice) }.as_bool()), "the notice is not shown");
    let placed = window_rect(notice);
    let around = RECT { left: placed.left - 20, top: placed.top - 20, right: placed.right + 20, bottom: placed.bottom + 20 };
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(800) {
        let marked = capture_screen(around).iter().filter(|pixel| is_notice_mark(pixel)).count();
        assert_eq!(marked, 0, "{marked} notice pixels in a screen capture of {around:?}");
        std::thread::sleep(Duration::from_millis(40));
    }

    let capturable = scene.helper.request("debug_overlay_capturable", json!({}));
    assert_eq!(capturable["ok"], true, "{capturable}");
    scene.show("background");
    assert!(
        wait_until(|| capture_screen(placed).iter().filter(|pixel| is_notice_mark(pixel)).count() > 0),
        "the notice cannot be seen even when it is left in the capture, so the check above proves nothing"
    );
}

/// The notice hides with the border (the server's idle timer hides the overlay with
/// `overlay_hide`), and shows again with it.
#[test]
fn the_notice_hides_and_shows_with_the_border() {
    let Some(_lock) = interactive() else { return };
    let mut scene = Scene::start();
    scene.show("background");
    let notice = scene.notice();
    let border = own_window(scene.helper.child.id(), w!("DESKPASS_OVERLAY")).expect("the border window");
    assert!(wait_until(|| unsafe { IsWindowVisible(notice) }.as_bool()), "the notice is not shown");
    assert!(unsafe { IsWindowVisible(border) }.as_bool(), "the border is not shown");

    let hidden = scene.helper.request("overlay_hide", json!({}));
    assert_eq!(hidden["ok"], true, "{hidden}");
    assert!(!unsafe { IsWindowVisible(border) }.as_bool(), "the border did not hide");
    assert!(!unsafe { IsWindowVisible(notice) }.as_bool(), "the notice did not hide with the border");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!unsafe { IsWindowVisible(notice) }.as_bool(), "the notice came back after hiding");

    scene.show("foreground");
    assert!(wait_until(|| unsafe { IsWindowVisible(notice) }.as_bool()), "the notice did not show again with the border");
}

/// The target fixture at `TARGET`, the user's window (another fixture process) in front, and a
/// helper. The overlay is hidden when dropped.
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
        place_window(&user_id, (TARGET.0 + 450, TARGET.1, 400, 300));
        for id in [&target_id, &user_id] {
            let activated = helper.request("activate_window", json!({ "window_id": id }));
            assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
        }
        Self { helper, target_id, user_id, _target: target_process, _user: user_process }
    }

    fn show(&mut self, delivery: &str) {
        let shown = self.helper.request("overlay_show", json!({ "window_id": self.target_id, "app_name": APP, "delivery_mode": delivery }));
        assert_eq!(shown["ok"], true, "{shown}");
    }

    /// The helper's notice window.
    fn notice(&self) -> HWND {
        let pid = self.helper.child.id();
        let mut found = None;
        assert!(wait_until(|| {
            found = own_window(pid, w!("DESKPASS_NOTICE"));
            found.is_some()
        }), "the helper has no notice window");
        found.expect("notice")
    }

    fn title(&mut self, window_id: &str) -> String {
        self.helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = self.helper.request("overlay_hide", json!({}));
    }
}

/// The window of class `class` of the helper process `pid` (another helper may have its own).
fn own_window(pid: u32, class: windows::core::PCWSTR) -> Option<HWND> {
    let mut after: Option<HWND> = None;
    loop {
        let window = unsafe { FindWindowExW(None, after, class, None) }.ok()?;
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };
        if owner == pid {
            return Some(window);
        }
        after = Some(window);
    }
}

/// Watches the foreground for 400 ms; it must stay `user`.
fn assert_foreground_stays(user: HWND, when: &str) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(400) {
        assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window lost the foreground {when}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn mouse(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn work_area(hwnd: HWND) -> RECT {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    assert!(unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool(), "GetMonitorInfoW");
    info.rcWork
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

/// The notice's accent, the border's green (`helper/src/overlay.rs`).
fn is_notice_mark(pixel: &[u8; 3]) -> bool {
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
            .args(["--lang", "zh-TW", "--server-pid", &std::process::id().to_string()])
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
