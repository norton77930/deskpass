//! A window can stick out past every screen (dragged above the top of the topmost monitor or
//! beyond an outer edge). Windows clamps absolute pointer input aimed there to the nearest
//! screen edge, so it would land on another part of the window, or on another window. Pointer
//! input at such a point is refused with `offscreen`, and the screenshot reports which part of
//! the window is on a screen.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONULL};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The framed fixture is moved so its top 10 rows are above the topmost monitor, where no
/// screen is in any layout. A click, drag, scroll, or pointer move at a point there is refused
/// and nothing reaches the fixture (it records every click on its caption); the same caption
/// 10 rows lower, on screen, still takes the click. The screenshot names the on-screen part.
#[test]
fn pointer_input_at_a_point_outside_every_screen_is_refused() {
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
    let original = visible_frame(hwnd);
    let (width, height) = (original.right - original.left, original.bottom - original.top);
    let on_screen = helper.request("capture_window", json!({ "window_id": window_id, "target_width": width, "target_height": height }));
    assert_eq!(on_screen["ok"], true, "{on_screen}");
    assert!(on_screen["result"].get("visible_parts").is_none(), "a window wholly on screen reports no visible_parts: {on_screen}");

    // The topmost monitor has no screen above it, whatever the layout.
    let monitor = monitors().into_iter().min_by_key(|rect| (rect.top, rect.left)).expect("a monitor");
    assert!(
        monitor.right - monitor.left > width && monitor.bottom - monitor.top > height,
        "the {width}x{height} fixture does not fit on the topmost monitor {monitor:?}"
    );
    let left = monitor.left + (monitor.right - monitor.left - width) / 2;
    let top = monitor.top - 10;
    let _restore = Restore { hwnd, frame: original };
    move_frame(hwnd, left, top).expect("move the fixture");
    // Moving onto a monitor of another scale makes Windows rescale the window, which can shift
    // its other edges by a pixel or two (measured: left -1, right +1, bottom +1); the top edge,
    // which puts its top rows off screen, is what the test needs exact.
    let moved = visible_frame(hwnd);
    let near = |a: i32, b: i32| (a - b).abs() <= 2;
    assert!(
        moved.top == top && near(moved.left, left) && near(moved.right, left + width) && near(moved.bottom, top + height),
        "the fixture did not move: {moved:?}, expected about {:?}",
        RECT { left, top, right: left + width, bottom: top + height }
    );
    let (x, off_y, on_y) = (width / 2, 4, 20);
    assert!(!on_a_monitor(left + x, top + off_y), "the point chosen to be off screen is on a monitor");
    assert!(on_a_monitor(left + x, top + on_y), "the point chosen to be on screen is on no monitor");

    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture, so the test must fail rather than skip: {activated}");
    let before = events(&mut helper, &window_id);
    let cursor_before = cursor();

    expect_foreground(hwnd, "before clicking off screen");
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": x, "y": off_y }));
    assert_offscreen(&clicked, "click");
    expect_foreground(hwnd, "before dragging to a point off screen");
    let drag_out = helper.request("drag", json!({ "window_id": window_id, "from_x": x, "from_y": on_y, "to_x": x + 40, "to_y": off_y }));
    assert_offscreen(&drag_out, "drag ending off screen");
    expect_foreground(hwnd, "before dragging from a point off screen");
    let drag_in = helper.request("drag", json!({ "window_id": window_id, "from_x": x, "from_y": off_y, "to_x": x + 40, "to_y": on_y }));
    assert_offscreen(&drag_in, "drag starting off screen");
    assert!(unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 == 0, "the left button is held after a refused drag");
    expect_foreground(hwnd, "before scrolling off screen");
    let scrolled = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": off_y, "direction": "down", "amount": 1 }));
    assert_offscreen(&scrolled, "scroll");
    expect_foreground(hwnd, "before moving the pointer off screen");
    let moved = helper.request("move_mouse", json!({ "window_id": window_id, "x": x, "y": off_y }));
    assert_offscreen(&moved, "move_mouse");
    assert_eq!(cursor(), cursor_before, "refused pointer input moved the pointer");
    assert_eq!(events(&mut helper, &window_id), before, "refused pointer input reached the fixture");

    expect_foreground(hwnd, "before clicking on screen");
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": x, "y": on_y }));
    assert_eq!(clicked["ok"], true, "a click on the on-screen part failed: {clicked}");
    let after = events(&mut helper, &window_id);
    assert_eq!(after.rsplit(',').next().unwrap_or(""), format!("miss@{x}x{on_y}"), "the on-screen click did not land where aimed (events {after})");

    let shot = helper.request("capture_window", json!({ "window_id": window_id, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    assert_eq!(
        shot["result"]["visible_parts"],
        json!([{ "x": 0, "y": 10, "width": width, "height": height - 10 }]),
        "the screenshot must name the on-screen part of the window: {shot}"
    );

    drop(_restore);
    drop(helper);
    drop(fixture);
}

fn assert_offscreen(response: &Value, action: &str) {
    assert_eq!(response["error"]["code"], "offscreen", "{action} at a point outside every screen must be refused: {response}");
    let message = response["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("outside every screen") && message.contains("no input was sent"), "{action}: {response}");
}

/// Puts the fixture's frame back where it was, also when an assertion failed. Dropping never
/// panics: a failed move back while an assertion unwinds is logged, so the assertion's message
/// is not lost to a double panic.
struct Restore {
    hwnd: HWND,
    frame: RECT,
}

impl Drop for Restore {
    fn drop(&mut self) {
        if let Err(error) = move_frame(self.hwnd, self.frame.left, self.frame.top) {
            eprintln!("could not move the fixture back to {:?}: {error}", self.frame);
        }
    }
}

/// Moves the window so its visible frame (not its invisible resize borders) starts at `left`, `top`.
fn move_frame(hwnd: HWND, left: i32, top: i32) -> Result<(), String> {
    let frame = try_visible_frame(hwnd)?;
    let mut outer = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut outer).map_err(|error| format!("window rect: {error}"))?;
        SetWindowPos(hwnd, None, outer.left + left - frame.left, outer.top + top - frame.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
            .map_err(|error| format!("SetWindowPos: {error}"))
    }
}

fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<RECT>);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            list.push(info.rcMonitor);
        }
        true.into()
    }
    let mut list: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut Vec<RECT> as isize));
    }
    list
}

fn on_a_monitor(x: i32, y: i32) -> bool {
    !unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) }.is_invalid()
}

/// What the fixture recorded, from its title (`CUMCP_FIXTURE | ev=a,b,...`).
fn events(helper: &mut Helper, window_id: &str) -> String {
    let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
    title.split(" | ev=").nth(1).unwrap_or("").to_string()
}

fn cursor() -> (i32, i32) {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.expect("cursor position");
    (point.x, point.y)
}

/// Fails, without sending anything, unless the foreground window is `expected`.
fn expect_foreground(expected: HWND, when: &str) {
    let foreground = unsafe { GetForegroundWindow() };
    assert!(expected == foreground, "{when}: the foreground window {:?} is not the fixture, so no input is sent", foreground.0);
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
    try_visible_frame(hwnd).expect("extended frame bounds")
}

fn try_visible_frame(hwnd: HWND) -> Result<RECT, String> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut RECT as *mut _, size_of::<RECT>() as u32)
            .map_err(|error| format!("extended frame bounds: {error}"))?;
    }
    Ok(rect)
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
