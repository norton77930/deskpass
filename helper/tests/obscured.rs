use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, SetWindowPos, WindowFromPoint, GA_ROOT, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// Another process's always-on-top window covers the left part of the fixture's button and
/// scroll area. Pointer input aimed at a covered point is refused with `target_obscured` naming
/// that window, and nothing reaches either window; the uncovered part of the same controls,
/// with our own click-through overlay shown, still works.
#[test]
fn input_at_a_point_covered_by_another_apps_window_is_refused() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, _) = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let at = |x: i64, y: i64| (width * x / 480, height * y / 360);

    // Covers fixture pixels x 0..70, y 40..200: the left of the button (10..130 x 48..76)
    // and of the scroll area (10..470 x 88..288).
    let cover_rect = format!("{},{},{},{}", left, top + height * 40 / 360, width * 70 / 480, height * 160 / 360);
    let cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", &cover_rect]).spawn().expect("cover"));
    let (cover, cover_app) = wait_for_window(&mut helper, "CUMCP_COVER");

    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    // An always-on-top window stays above the raised fixture, and activation says so.
    let covered_by = activated["result"]["covered_by"].as_array().cloned().unwrap_or_default();
    assert_eq!(covered_by.len(), 1, "{activated}");
    assert_eq!(covered_by[0]["window_id"], json!(cover), "{activated}");
    assert_eq!(covered_by[0]["app"], json!(cover_app), "{activated}");
    assert!(covered_by[0]["title"].as_str().is_some_and(|title| title.starts_with("CUMCP_COVER")), "{activated}");
    assert_eq!(covered_by[0]["topmost"], json!(true), "the always-on-top cover must say so: {activated}");

    let (x, y) = at(40, 62);
    expect_foreground(&main, "before clicking the covered part of the button");
    let refused = helper.request("click", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(refused["error"]["code"], "target_obscured", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(cover), "{refused}");
    assert_eq!(refused["error"]["data"]["app"], json!(cover_app), "{refused}");
    assert!(refused["error"]["data"]["title"].as_str().is_some_and(|title| title.starts_with("CUMCP_COVER")), "{refused}");
    assert_eq!(refused["error"]["data"]["topmost"], json!(true), "the always-on-top cover must say so: {refused}");
    let message = refused["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("CUMCP_COVER") && message.contains("no input was sent"), "{refused}");

    let (covered_x, covered_y) = at(40, 150);
    let (open_x, open_y) = at(200, 150);
    expect_foreground(&main, "before dragging onto the covered part");
    let drag_to_cover = helper.request("drag", json!({ "window_id": main, "from_x": open_x, "from_y": open_y, "to_x": covered_x, "to_y": covered_y, "modifiers": ["shift"] }));
    assert_eq!(drag_to_cover["error"]["code"], "target_obscured", "a covered end point must be refused: {drag_to_cover}");
    expect_foreground(&main, "before dragging from the covered part");
    let drag_from_cover = helper.request("drag", json!({ "window_id": main, "from_x": covered_x, "from_y": covered_y, "to_x": open_x, "to_y": open_y, "modifiers": ["shift"] }));
    assert_eq!(drag_from_cover["error"]["code"], "target_obscured", "a covered start point must be refused: {drag_from_cover}");
    assert!(unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 == 0, "the left button is held after a refused drag");
    assert!(unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } as u16 & 0x8000 == 0, "Shift is held after a refused Shift drag");
    expect_foreground(&main, "before scrolling the covered part");
    let scroll = helper.request("scroll", json!({ "window_id": main, "x": covered_x, "y": covered_y, "direction": "down", "amount": 1 }));
    assert_eq!(scroll["error"]["code"], "target_obscured", "{scroll}");
    let cursor_before = cursor();
    expect_foreground(&main, "before moving onto the covered part");
    let moved = helper.request("move_mouse", json!({ "window_id": main, "x": covered_x, "y": covered_y }));
    assert_eq!(moved["error"]["code"], "target_obscured", "{moved}");
    assert_eq!(cursor(), cursor_before, "a refused move_mouse moved the pointer");

    let fixture_title = title(&mut helper, &main);
    for recorded in ["click:", "drag:", "scroll:"] {
        assert!(!fixture_title.contains(recorded), "refused input reached the fixture: {fixture_title}");
    }
    assert_eq!(title(&mut helper, &cover), "CUMCP_COVER | ev=", "refused input reached the covering window");

    // Uncovered points of the same controls work, with our own overlay on top of the fixture.
    let shown = helper.request("overlay_show", json!({ "window_id": main }));
    assert_eq!(shown["ok"], true, "{shown}");
    let (x, y) = at(100, 62);
    expect_foreground(&main, "before clicking the uncovered part of the button");
    let clicked = helper.request("click", json!({ "window_id": main, "x": x, "y": y }));
    expect_foreground(&main, "before scrolling the uncovered part");
    let scrolled = helper.request("scroll", json!({ "window_id": main, "x": open_x, "y": open_y, "direction": "down", "amount": 1 }));
    let _ = helper.request("overlay_hide", json!({}));
    assert_eq!(clicked["ok"], true, "the uncovered click failed (our overlay must not count as covering): {clicked}");
    assert_eq!(scrolled["ok"], true, "{scrolled}");
    let fixture_title = title(&mut helper, &main);
    assert!(fixture_title.contains("click:left") && fixture_title.contains("scroll:"), "{fixture_title}");
    assert_eq!(title(&mut helper, &cover), "CUMCP_COVER | ev=", "input aimed at the fixture reached the covering window");
    drop(helper);
    drop(cover_process);
    drop(fixture);
}

/// The fixture holds the foreground, but another process's ordinary window was placed over the
/// left part of its button, and the fixture was moved below it with `SWP_NOACTIVATE`, so it is
/// buried under it (as Paint was under VS Code). Activating the fixture raises it without any keyboard input, and a click on
/// the formerly covered part of the button then lands on the fixture.
#[test]
fn a_foreground_target_buried_under_another_apps_window_is_raised_without_alt() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, _) = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let placed = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(placed["ok"], true, "could not foreground the fixture: {placed}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let at = |x: i64, y: i64| (width * x / 480, height * y / 360);

    // Covers fixture pixels x 0..70, y 40..200: the left of the button (10..130 x 48..76).
    let cover_rect = format!("{},{},{},{}", left, top + height * 40 / 360, width * 70 / 480, height * 160 / 360);
    let cover_process =
        KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", &cover_rect, "--not-topmost"]).spawn().expect("cover"));
    let (cover, _) = wait_for_window(&mut helper, "CUMCP_COVER");
    // Windows keeps a background process's window from rising above the foreground window, so
    // the fixture is moved below the covering window instead; it keeps the foreground.
    unsafe { SetWindowPos(hwnd(&main), Some(hwnd(&cover)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }.expect("bury the fixture");
    let (x, y) = at(40, 62);
    let covered = (left as i32 + x as i32, top as i32 + y as i32);
    expect_foreground(&main, "after the fixture was buried under the covering window");
    assert_eq!(root_at(covered), hwnd(&cover), "the covering window is not over the fixture's button (fixture {main}, cover {cover})");
    let before = events_of(&title(&mut helper, &main));

    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "{activated}");
    assert!(activated["result"].get("covered_by").is_none(), "nothing covers the raised fixture: {activated}");
    // Checked before the click, which raises the target again, so the raise is activation's.
    assert_eq!(root_at(covered), hwnd(&main), "activation did not raise the fixture above the covering window");
    let after = events_of(&title(&mut helper, &main));
    assert_eq!(after, before, "activating the fixture already in front sent it ALT or a system key");
    expect_foreground(&main, "before clicking the formerly covered part of the button");
    let clicked = helper.request("click", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let fixture_title = title(&mut helper, &main);
    assert!(fixture_title.contains("click:left"), "{fixture_title}");
    assert_eq!(title(&mut helper, &cover), "CUMCP_COVER | ev=", "the click reached the covering window");
    drop(helper);
    drop(cover_process);
    drop(fixture);
}

/// Ticket 52 (F18, ticket 33's gap): a raise Windows refuses. The helper here runs at Low
/// integrity (`low_integrity_helper`), below the medium-integrity fixture, so UIPI refuses its
/// raise of the fixture, and another app's ordinary window over the buried fixture stays above
/// it. A click at a point that window covers is refused with `target_obscured` saying the window
/// is not always on top, activation lists it in `covered_by` the same way, and nothing reaches
/// either window.
#[test]
fn a_window_over_the_target_that_the_raise_cannot_pass_is_reported_as_not_topmost() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let low_helper = low_integrity_helper();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, _) = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let placed = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(placed["ok"], true, "could not foreground the fixture: {placed}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let at = |x: i64, y: i64| (width * x / 480, height * y / 360);

    // Covers fixture pixels x 0..70, y 40..200: the left of the button (10..130 x 48..76).
    let cover_rect = format!("{},{},{},{}", left, top + height * 40 / 360, width * 70 / 480, height * 160 / 360);
    let cover_process =
        KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", &cover_rect, "--not-topmost"]).spawn().expect("cover"));
    let (cover, cover_app) = wait_for_window(&mut helper, "CUMCP_COVER");
    // As in the buried-target test: the fixture keeps the foreground under the covering window.
    unsafe { SetWindowPos(hwnd(&main), Some(hwnd(&cover)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }.expect("bury the fixture");
    let (x, y) = at(40, 62);
    let covered = (left as i32 + x as i32, top as i32 + y as i32);
    expect_foreground(&main, "after the fixture was buried under the covering window");
    assert_eq!(root_at(covered), hwnd(&cover), "the covering window is not over the fixture's button (fixture {main}, cover {cover})");

    let mut low = Helper::spawn_from(&low_helper);
    let refused = low.request("click", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(refused["error"]["code"], "target_obscured", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(cover), "{refused}");
    assert_eq!(refused["error"]["data"]["app"], json!(cover_app), "{refused}");
    assert_eq!(refused["error"]["data"]["topmost"], json!(false), "the cover is not always on top: {refused}");
    assert_eq!(root_at(covered), hwnd(&cover), "the fixture was raised, so the raise was not refused");
    expect_foreground(&main, "before activating the fixture from the Low-integrity helper");
    let activated = low.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "{activated}");
    let covered_by = activated["result"]["covered_by"].as_array().cloned().unwrap_or_default();
    assert_eq!(covered_by.len(), 1, "{activated}");
    assert_eq!(covered_by[0]["window_id"], json!(cover), "{activated}");
    assert_eq!(covered_by[0]["topmost"], json!(false), "the cover is not always on top: {activated}");
    drop(low);
    let fixture_title = title(&mut helper, &main);
    assert!(!fixture_title.contains("click:"), "the refused click reached the fixture: {fixture_title}");
    assert_eq!(title(&mut helper, &cover), "CUMCP_COVER | ev=", "the refused click reached the covering window");
    drop(helper);
    drop(cover_process);
    drop(fixture);
}

/// A copy of the helper exe whose file carries the Low mandatory label, so the process this
/// (medium-integrity) test starts from it runs at Low integrity: Windows starts a process at the
/// lower of its parent's level and its file's label. No elevation is needed, but the label takes
/// WRITE_OWNER on the file, which a user who only has Modify on the build directory lacks, so the
/// copy's owner (this test) first grants the owner full control of it.
fn low_integrity_helper() -> String {
    let path = std::path::Path::new(env!("CARGO_BIN_EXE_deskpass-helper")).with_file_name("deskpass-helper-low-integrity.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_deskpass-helper"), &path).expect("copy the helper exe");
    for args in [["/grant", "*S-1-3-4:F"], ["/setintegritylevel", "low"]] {
        let output = Command::new("icacls").arg(&path).args(args).output().expect("icacls");
        assert!(output.status.success(), "icacls {args:?} failed: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
    path.to_str().expect("path").to_string()
}

/// Ticket 51 (F17, ticket 35's gap): the fixture's modal dialog holds the foreground while the
/// fixture and the dialog lie under another app's window that is not always on top. Activating
/// the fixture raises both above it, the dialog on top (it rises with its owner, and `raise`
/// raises it last as well), and the dialog keeps the foreground; the call is `dialog_open`
/// naming the dialog.
#[test]
fn activating_a_buried_target_whose_dialog_holds_the_foreground_raises_both_dialog_on_top() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, _) = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let placed = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(placed["ok"], true, "could not foreground the fixture: {placed}");
    let rect = |helper: &mut Helper, window_id: &str| {
        let info = helper.request("get_window", json!({ "window_id": window_id }));
        ["x", "y", "width", "height"].map(|key| info["result"][key].as_i64().unwrap_or_else(|| panic!("{key}: {info}")))
    };
    let [left, top, width, height] = rect(&mut helper, &main);
    expect_foreground(&main, "before clicking the Dialog button");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "{opened}");
    let (dialog, _) = wait_for_window(&mut helper, "CUMCP_DIALOG");
    let [dialog_left, dialog_top, dialog_width, dialog_height] = rect(&mut helper, &dialog);
    let dialog_middle = ((dialog_left + dialog_width / 2) as i32, (dialog_top + dialog_height / 2) as i32);
    // A point of the fixture the dialog does not cover: its top-left corner.
    let owner_point = ((left + 5) as i32, (top + 5) as i32);
    assert!(i64::from(owner_point.0) < dialog_left || i64::from(owner_point.1) < dialog_top, "the dialog covers the fixture's top-left corner");

    // Covers the fixture and the dialog.
    let (cover_left, cover_top) = (left.min(dialog_left), top.min(dialog_top));
    let (cover_right, cover_bottom) = ((left + width).max(dialog_left + dialog_width), (top + height).max(dialog_top + dialog_height));
    let cover_rect = format!("{cover_left},{cover_top},{},{}", cover_right - cover_left, cover_bottom - cover_top);
    let cover_process =
        KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", &cover_rect, "--not-topmost"]).spawn().expect("cover"));
    let (cover, _) = wait_for_window(&mut helper, "CUMCP_COVER");
    // As in the test above, the windows are moved below the covering one; the dialog keeps the
    // foreground.
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    unsafe { SetWindowPos(hwnd(&dialog), Some(hwnd(&cover)), 0, 0, 0, 0, flags) }.expect("bury the dialog");
    unsafe { SetWindowPos(hwnd(&main), Some(hwnd(&dialog)), 0, 0, 0, 0, flags) }.expect("bury the fixture");
    expect_foreground(&dialog, "after the fixture and its dialog were buried");
    assert_eq!(root_at(dialog_middle), hwnd(&cover), "the covering window is not over the dialog (dialog {dialog}, cover {cover})");
    assert_eq!(root_at(owner_point), hwnd(&cover), "the covering window is not over the fixture (fixture {main}, cover {cover})");

    let activated = helper.request("activate_window", json!({ "window_id": main }));
    let (over_dialog, over_owner, foreground) = (root_at(dialog_middle), root_at(owner_point), unsafe { GetForegroundWindow() });
    drop(helper);
    drop(cover_process);
    drop(fixture);
    assert_eq!(activated["error"]["code"], "dialog_open", "{activated}");
    assert_eq!(activated["error"]["data"]["window_id"], dialog.as_str(), "{activated}");
    assert_eq!(over_owner, hwnd(&main), "activation did not raise the fixture above the covering window");
    assert_eq!(over_dialog, hwnd(&dialog), "activation did not raise the dialog above the fixture and the covering window");
    assert_eq!(foreground, hwnd(&dialog), "the dialog lost the foreground");
}

/// The root window at a screen point in physical pixels, as the helper sees it.
fn root_at((x, y): (i32, i32)) -> HWND {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT)
    }
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

fn events_of(title: &str) -> String {
    title.rsplit(" | ev=").next().unwrap_or("").to_string()
}

fn cursor() -> (i32, i32) {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.expect("cursor position");
    (point.x, point.y)
}

/// Fails, without sending anything, unless the foreground window is `expected`.
fn expect_foreground(expected: &str, when: &str) {
    let foreground = unsafe { GetForegroundWindow() };
    let expected_hwnd = HWND(expected.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void);
    assert!(expected_hwnd == foreground, "{when}: the foreground window {:?} is not {expected}, so no input is sent", foreground.0);
}

fn title(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

/// The window id and app of the first listed window whose title starts with `prefix`.
fn wait_for_window(helper: &mut Helper, prefix: &str) -> (String, String) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_windows", json!({}));
        if let Some(found) = last["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"]
                    .as_str()
                    .filter(|title| title.starts_with(prefix))
                    .and_then(|_| Some((window["window_id"].as_str()?.to_string(), window["app"].as_str()?.to_string())))
            })
        }) {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{prefix} was not listed: {last}");
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

    fn spawn_from(path: &str) -> Self {
        let mut child = Command::new(path)
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
