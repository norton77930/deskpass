use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetWindowRect, GetWindowThreadProcessId};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn overlay_is_absent_from_the_fixture_capture_and_clicks_still_land() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let shown = helper.request("overlay_show", json!({ "window_id": window_id }));
    assert_eq!(shown["ok"], true, "{shown}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(0);
    let height = info["result"]["height"].as_i64().unwrap_or(0);
    let shot = helper.request("capture_window", json!({ "window_id": window_id, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot["result"]["image_base64"].as_str().unwrap_or(""))
        .expect("jpeg");
    let image = image::load_from_memory(&bytes).expect("jpeg").to_rgb8();
    let green = image.pixels().filter(|pixel| pixel[1] > 180 && pixel[0] < 40 && pixel[2] < 40).count();
    assert_eq!(green, 0, "overlay border leaked into the capture");
    let listed = helper.request("list_windows", json!({}));
    let titled = listed["result"]["windows"].as_array().is_some_and(|windows| {
        windows.iter().any(|window| window["title"].as_str().unwrap_or("").contains("AI 正在操作") || window["title"].as_str().unwrap_or("").contains("AI is operating"))
    });
    assert!(!titled, "overlay must not be an operable window: {listed}");

    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    if activated["ok"] != true {
        let _ = helper.request("overlay_hide", json!({}));
        panic!("foreground failed, so the overlay click-through test must fail rather than skip: {activated}");
    }
    let clicked = helper.request("click", json!({
        "window_id": window_id,
        "x": width * 70 / 480,
        "y": height * 62 / 360,
        "button": "left",
        "click_count": 1,
    }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
    let _ = helper.request("overlay_hide", json!({}));
    assert!(title.contains("click:left"), "{title}");
    drop(helper);
    drop(fixture);
}

/// The overlay is a visible tool window of the helper's own process. Targeting it by id is
/// `window_not_found` (it belongs to Deskpass itself), the same as `hwnd_from` answers for
/// input, and not `window_not_operable` because it is a tool window.
#[test]
fn get_window_on_the_overlay_is_window_not_found() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let shown = helper.request("overlay_show", json!({ "window_id": window_id }));
    assert_eq!(shown["ok"], true, "{shown}");
    let mut overlay = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && overlay.is_none() {
        overlay = unsafe { FindWindowW(w!("DESKPASS_OVERLAY"), None) }.ok();
        if overlay.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let Some(overlay) = overlay else {
        let _ = helper.request("overlay_hide", json!({}));
        panic!("the overlay window was not created");
    };
    let refused = helper.request("get_window", json!({ "window_id": (overlay.0 as isize).to_string() }));
    let _ = helper.request("overlay_hide", json!({}));
    assert_eq!(refused["error"]["code"], "window_not_found", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("belongs to Deskpass itself"), "{refused}");
    drop(helper);
    drop(fixture);
}

/// Ticket 110: a window of another Deskpass helper process is refused by its executable name,
/// `deskpass-helper.exe` or a copy renamed aside while it ran (`deskpass-helper.old-<time>.exe`,
/// scripts/copy-helper.mjs). Here a renamed-aside copy shows its overlay; the server's helper,
/// whose own file name differs, neither lists it nor operates it.
#[test]
fn the_overlay_of_a_renamed_aside_deskpass_helper_is_not_listed_or_targeted() {
    assert_overlay_of_a_copy_is_refused("deskpass-helper");
}

/// Ticket 110: so is a window of an older install's helper still running on the same desktop,
/// `computer-use-helper.exe` or its renamed-aside copy (`computer-use-helper.old-<time>.exe`).
#[test]
fn the_overlay_of_an_old_install_computer_use_helper_is_not_listed_or_targeted() {
    assert_overlay_of_a_copy_is_refused("computer-use-helper");
}

/// A copy of the helper named `<stem>.old-…exe` shows its overlay; the server's helper neither
/// lists it nor operates it (`window_not_found`, as the helper's own window).
fn assert_overlay_of_a_copy_is_refused(stem: &str) {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let copy = RenamedHelper::new(stem);
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let mut other = Helper::spawn_from(copy.path());
    let window_id = wait_for_fixture(&mut helper);
    let shown = other.request("overlay_show", json!({ "window_id": window_id }));
    assert_eq!(shown["ok"], true, "{shown}");
    let deadline = Instant::now() + Duration::from_secs(5);
    let overlay = loop {
        if let Some(overlay) = overlay_of(other.child.id()) {
            break overlay;
        }
        if Instant::now() > deadline {
            let _ = other.request("overlay_hide", json!({}));
            panic!("the renamed helper's overlay window was not created");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let id = (overlay.0 as isize).to_string();
    let listed = helper.request("list_windows", json!({}));
    let refused = helper.request("get_window", json!({ "window_id": id }));
    let clicked = helper.request("click", json!({ "window_id": id, "x": 10, "y": 10, "button": "left", "click_count": 1 }));
    let _ = other.request("overlay_hide", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    assert!(!listed["result"]["windows"].as_array().expect("windows").iter().any(|window| window["window_id"] == id.as_str()), "the overlay is listed: {listed}");
    assert_eq!(refused["error"]["code"], "window_not_found", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("belongs to Deskpass itself"), "{refused}");
    assert_eq!(clicked["error"]["code"], "window_not_found", "{clicked}");
    drop(other);
    drop(helper);
    drop(fixture);
}

/// A copy of the helper exe named `<stem>.old-…exe`, the way scripts/copy-helper.mjs renames a
/// running one aside, beside the built exe; deleted when dropped.
struct RenamedHelper(std::path::PathBuf);
impl RenamedHelper {
    fn new(stem: &str) -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let built = std::path::Path::new(env!("CARGO_BIN_EXE_deskpass-helper"));
        let path = built.with_file_name(format!("{stem}.old-{}-{nanos}.exe", std::process::id()));
        std::fs::copy(built, &path).expect("copy the helper exe");
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for RenamedHelper {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The overlay's border and label sit around the target's visible frame (DWM extended frame
/// bounds, the frame screenshots and clicks use), not around GetWindowRect, which on a framed
/// window also covers the invisible resize borders (F22). `BORDER` and `LABEL_HEIGHT` are
/// `helper/src/overlay.rs`'s.
#[test]
fn the_overlay_border_surrounds_the_visible_frame() {
    const BORDER: i32 = 6;
    const LABEL_HEIGHT: i32 = 28;
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--frame").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let target = HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void);
    let mut frame = RECT::default();
    let mut outer = RECT::default();
    unsafe {
        DwmGetWindowAttribute(target, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut RECT as *mut _, size_of::<RECT>() as u32).expect("extended frame bounds");
        GetWindowRect(target, &mut outer).expect("window rect");
    }
    assert_ne!((outer.left, outer.right, outer.bottom), (frame.left, frame.right, frame.bottom), "the framed fixture must have invisible resize borders, or this test proves nothing");

    let shown = helper.request("overlay_show", json!({ "window_id": window_id }));
    assert_eq!(shown["ok"], true, "{shown}");
    let overlay = overlay_of(helper.child.id());
    let mut placed = RECT::default();
    let found = overlay.is_some_and(|overlay| unsafe { GetWindowRect(overlay, &mut placed) }.is_ok());
    let _ = helper.request("overlay_hide", json!({}));
    assert!(found, "the helper's overlay window was not found");
    let expected = RECT { left: frame.left - BORDER, top: frame.top - BORDER - LABEL_HEIGHT, right: frame.right + BORDER, bottom: frame.bottom + BORDER };
    assert_eq!(
        (placed.left, placed.top, placed.right, placed.bottom),
        (expected.left, expected.top, expected.right, expected.bottom),
        "the overlay is not placed around the visible frame {frame:?} (GetWindowRect {outer:?})"
    );
    drop(helper);
    drop(fixture);
}

/// The overlay window of the helper process `pid` (another helper, a real one, may have its own).
fn overlay_of(pid: u32) -> Option<HWND> {
    let mut after: Option<HWND> = None;
    loop {
        let window = unsafe { FindWindowExW(None, after, w!("DESKPASS_OVERLAY"), None) }.ok()?;
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };
        if owner == pid {
            return Some(window);
        }
        after = Some(window);
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
        Self::spawn_from(env!("CARGO_BIN_EXE_deskpass-helper"))
    }
    fn spawn_from(exe: impl AsRef<std::ffi::OsStr>) -> Self {
        let mut child = Command::new(exe)
            .args(["--server-pid", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("helper");
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        Self { child, reader, next_id: 1 }
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
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
