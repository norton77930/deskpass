//! Ticket 43: a click whose input all reached the target, after which the target hands the
//! foreground to another window or closes, succeeds and names the new foreground. Losing the
//! foreground before the last click is still `input_failed`, and nothing more is sent.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, IsWindow, SetCursorPos};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn a_click_after_which_the_target_opens_a_window_and_hands_it_the_foreground_succeeds_naming_it() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--click-handoff", "new"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    let clicked = click_background(&mut helper, &window_id, 1);
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    let foreground = unsafe { GetForegroundWindow() };
    drop(helper);
    drop(fixture);
    assert_eq!(clicked["ok"], true, "the click reached the target before it handed off the foreground: {clicked}");
    let changed = &clicked["result"]["foreground_changed"];
    assert_eq!(changed["target_closed"], false, "{clicked}");
    assert_eq!(changed["foreground"]["window_id"], opened.as_str(), "{clicked}");
    assert_eq!(changed["foreground"]["title"], "CUMCP_HANDOFF", "{clicked}");
    assert!(changed["foreground"]["app"].as_str().is_some_and(|app| app.to_ascii_lowercase().ends_with("fixture-window.exe")), "{clicked}");
    assert!(changed["foreground"]["app_name"].as_str().is_some_and(|name| !name.is_empty()), "{clicked}");
    assert_eq!(foreground, hwnd(&opened), "the opened window was not in the foreground after the click");
}

#[test]
fn a_click_after_which_the_target_closes_succeeds_saying_it_closed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--click-handoff", "close"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    let clicked = click_background(&mut helper, &window_id, 1);
    let still_open = unsafe { IsWindow(Some(hwnd(&window_id))) }.as_bool();
    drop(helper);
    drop(fixture);
    assert!(!still_open, "the click did not close the fixture");
    assert_eq!(clicked["ok"], true, "{clicked}");
    let changed = &clicked["result"]["foreground_changed"];
    assert_eq!(changed["target_closed"], true, "{clicked}");
    // Whatever Windows activated next, or nothing.
    assert!(changed["foreground"].is_null() || changed["foreground"]["window_id"].is_string(), "{clicked}");
    assert_ne!(changed["foreground"]["window_id"], window_id.as_str(), "{clicked}");
}

/// The first click of a double click hands the foreground to a new window: the second click is
/// not sent, and the result is `input_failed` saying so.
#[test]
fn a_double_click_whose_first_click_hands_off_the_foreground_sends_no_second_click() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--click-handoff", "new"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    // Ticket 49: the double click only stops if the fixture hands off within the 40 ms before the
    // second click. The first hand-off of a fresh fixture process (the first time its window is
    // shown) took longer once in 21 runs, each time right after a rebuild; one single click first
    // makes that first hand-off, and the fixture is brought back to the front.
    let warm_up = click_background(&mut helper, &window_id, 1);
    assert_eq!(warm_up["ok"], true, "{warm_up}");
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    let clicked = click_background(&mut helper, &window_id, 2);
    std::thread::sleep(Duration::from_millis(300));
    let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
    drop(helper);
    drop(fixture);
    assert!(title.contains("bg:down,bg:up"), "the first click did not reach the fixture: {title}");
    assert_eq!(title.matches("bg:down").count(), 2, "the second click was sent after the target lost the foreground: {title}; {clicked}");
    assert_eq!(clicked["ok"], false, "{clicked}");
    assert_eq!(clicked["error"]["code"], "input_failed", "{clicked}");
    let message = clicked["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("after click 1 of 2") && message.contains("not sent"), "the error does not say the rest was not sent: {message}");
}

/// Ticket 49 (F34): a drag whose release, its last event, the target answers by handing the
/// foreground to a new window succeeds and names it; the release reached the target at the end.
#[test]
fn a_drag_after_which_the_target_hands_off_the_foreground_succeeds_naming_it() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--click-handoff", "new"]) else { return };
    let (from, to) = (scaled(&mut helper, &window_id, 300, 320), scaled(&mut helper, &window_id, 400, 330));
    let dragged = helper.request("drag", json!({ "window_id": window_id, "from_x": from.0, "from_y": from.1, "to_x": to.0, "to_y": to.1 }));
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    let title = window_title(hwnd(&window_id));
    drop(helper);
    drop(fixture);
    assert_eq!(dragged["ok"], true, "the drag reached the target before it handed off the foreground: {dragged}");
    assert_eq!(dragged["result"]["foreground_changed"]["target_closed"], false, "{dragged}");
    assert_eq!(dragged["result"]["foreground_changed"]["foreground"]["window_id"], opened.as_str(), "{dragged}");
    assert!(title.contains(&format!("bg:down,bg:up@{},{}", to.0, to.1)), "the drag did not end at its end point: {title}");
}

/// Ticket 49 (F34): a drag whose press the target answers by handing the foreground to another
/// thread's window moves no further: `input_failed` says how many moves were sent, and the button
/// is released where the pointer was then, short of the end.
#[test]
fn a_drag_that_loses_the_foreground_partway_moves_no_further() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "down"]) else { return };
    let (from, to) = (scaled(&mut helper, &window_id, 300, 320), scaled(&mut helper, &window_id, 400, 330));
    let dragged = helper.request("drag", json!({ "window_id": window_id, "from_x": from.0, "from_y": from.1, "to_x": to.0, "to_y": to.1 }));
    std::thread::sleep(Duration::from_millis(300));
    let title = window_title(hwnd(&window_id));
    drop(helper);
    drop(fixture);
    assert_eq!(dragged["error"]["code"], "input_failed", "{dragged}");
    let message = dragged["error"]["message"].as_str().unwrap_or("");
    let moved: i64 = (0..5)
        .find(|moved| message == format!("The target window lost the foreground during the drag, after {moved} of 5 moves; the remaining moves were not sent, and the button was released where the pointer was."))
        .unwrap_or_else(|| panic!("{message}"));
    let up = title.split("bg:up@").nth(1).and_then(|rest| rest.split(',').take(2).map(|value| value.parse::<i64>().ok()).collect::<Option<Vec<_>>>());
    let up = up.unwrap_or_else(|| panic!("the release did not reach the fixture: {title}"));
    let expected = (from.0 + (to.0 - from.0) * moved / 5, from.1 + (to.1 - from.1) * moved / 5);
    assert!((up[0] - expected.0).abs() <= 1 && (up[1] - expected.1).abs() <= 1, "released at {up:?}, not after {moved} moves at {expected:?}: {title}");
    eprintln!("the drag lost the foreground after {moved} of 5 moves");
}

/// Ticket 49 (F34): a scroll whose last notch the target answers by handing off the foreground
/// succeeds and names the new foreground.
#[test]
fn a_scroll_whose_last_notch_hands_off_the_foreground_succeeds_naming_it() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "wheel:3"]) else { return };
    let at = scaled(&mut helper, &window_id, 240, 180);
    let scrolled = helper.request("scroll", json!({ "window_id": window_id, "x": at.0, "y": at.1, "direction": "down", "amount": 3 }));
    let opened = find_window(&mut helper, "CUMCP_HANDOFF");
    let title = window_title(hwnd(&window_id));
    drop(helper);
    drop(fixture);
    let opened = opened.unwrap_or_else(|listed| panic!("CUMCP_HANDOFF was not listed; scroll: {scrolled}; fixture: {title}; listed: {listed}"));
    assert_eq!(scrolled["ok"], true, "{scrolled}");
    assert_eq!(scrolled["result"]["foreground_changed"]["target_closed"], false, "{scrolled}");
    assert_eq!(scrolled["result"]["foreground_changed"]["foreground"]["window_id"], opened.as_str(), "{scrolled}");
    // The target's wheel events add up to the 3 notches; Windows may have merged two of them into
    // one event the fixture had not read yet (ticket 51, F39), so their count may be lower.
    assert!(title.contains(",sum=-360,"), "the target did not get all 3 notches: {title}");
}

/// Ticket 49 (F34): losing the foreground after a notch that is not the last sends no more
/// notches, to the target (under the pointer) or to the window in front, and says how many were.
#[test]
fn a_scroll_that_loses_the_foreground_partway_sends_no_more_notches() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "wheel:2"]) else { return };
    let at = scaled(&mut helper, &window_id, 240, 180);
    let scrolled = helper.request("scroll", json!({ "window_id": window_id, "x": at.0, "y": at.1, "direction": "down", "amount": 5 }));
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    std::thread::sleep(Duration::from_millis(300));
    let (title, other) = (window_title(hwnd(&window_id)), window_title(hwnd(&opened)));
    drop(helper);
    drop(fixture);
    assert_eq!(scrolled["error"]["code"], "input_failed", "{scrolled}");
    let message = scrolled["error"]["message"].as_str().unwrap_or("");
    let sent = (2..5)
        .find(|sent| message == format!("The target window lost the foreground after wheel notch {sent} of 5; the remaining {} were not sent.", 5 - sent))
        .unwrap_or_else(|| panic!("{message}"));
    // Counted in notches: Windows may merge wheel events the fixture has not read yet (F39).
    assert!(title.contains(&format!(",sum={},", -120 * sent)), "the target got a different number of notches than the {sent} reported: {title}");
    assert!(!other.contains("wheel"), "a notch went to the window in front: {other}");
}

/// Ticket 49 (F34): a key press whose key the target answers by handing the foreground to another
/// thread's window succeeds and names it, whether the release still reached the target or went to
/// that window (then the target never reads it, and the call ends `FOREGROUND_RETURN_LIMIT`, about
/// a second, later).
#[test]
fn a_key_press_after_which_the_target_hands_off_the_foreground_succeeds_naming_it() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "key:66"]) else { return };
    click_edit(&mut helper, &window_id);
    let started = Instant::now();
    let pressed = helper.request("press_key", json!({ "window_id": window_id, "keys": ["b"] }));
    let elapsed = started.elapsed();
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    std::thread::sleep(Duration::from_millis(300));
    let (title, other) = (window_title(hwnd(&window_id)), window_title(hwnd(&opened)));
    drop(helper);
    drop(fixture);
    eprintln!("press_key answered after {elapsed:?}; {title}; {other}");
    assert_eq!(pressed["ok"], true, "the press reached the target before it handed off the foreground: {pressed}");
    assert_eq!(pressed["result"]["foreground_changed"]["target_closed"], false, "{pressed}");
    assert_eq!(pressed["result"]["foreground_changed"]["foreground"]["window_id"], opened.as_str(), "{pressed}");
    assert!(title.contains("edit=b |"), "the target did not read the key: {title}");
    assert!(!other.contains("key:66"), "the press went to the window in front: {other}");
    assert_released();
}

/// Ticket 49 (F34): losing the foreground on a key that is not the last presses no more keys and
/// releases the ones pressed.
#[test]
fn a_key_press_that_loses_the_foreground_partway_sends_no_more_keys() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "key:16"]) else { return };
    click_edit(&mut helper, &window_id);
    let pressed = helper.request("press_key", json!({ "window_id": window_id, "keys": ["shift", "f5", "b"] }));
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    std::thread::sleep(Duration::from_millis(300));
    let (title, other) = (window_title(hwnd(&window_id)), window_title(hwnd(&opened)));
    drop(helper);
    drop(fixture);
    assert_eq!(pressed["error"]["code"], "input_failed", "{pressed}");
    assert_eq!(
        pressed["error"]["message"],
        "The target window lost the foreground during the key press; the remaining keys were not sent, and the keys pressed so far were released.",
        "{pressed}"
    );
    // F5 may still be sent in the instant between the check before it and the hand-off (it then
    // reaches the window in front, ticket 43's limit); the check before `b` sees the hand-off.
    assert!(!other.contains("key:66"), "a key after the hand-off was pressed: {other}");
    assert!(title.contains("edit= |"), "{title}");
    assert_released();
}

/// Ticket 49 (F34): text whose last character the target answers by handing off the foreground
/// (not by closing) succeeds and names the new foreground.
#[test]
fn text_whose_last_character_hands_off_the_foreground_succeeds_naming_it() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "char:9"]) else { return };
    click_edit(&mut helper, &window_id);
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "129" }));
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    let title = window_title(hwnd(&window_id));
    drop(helper);
    drop(fixture);
    assert_eq!(typed["ok"], true, "{typed}");
    assert_eq!(typed["result"]["foreground_changed"]["target_closed"], false, "{typed}");
    assert_eq!(typed["result"]["foreground_changed"]["foreground"]["window_id"], opened.as_str(), "{typed}");
    assert!(title.contains("edit=129 |"), "{title}");
    assert_released();
}

/// Ticket 49 (F34): losing the foreground after a character that is not the last types no more,
/// into the target or the window in front, and says how many were typed.
#[test]
fn text_that_loses_the_foreground_partway_types_no_more() {
    let Some((_interactive, fixture, mut helper, window_id)) = open(&["--handoff-on", "char:9"]) else { return };
    click_edit(&mut helper, &window_id);
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "12945" }));
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    std::thread::sleep(Duration::from_millis(300));
    let (title, other) = (window_title(hwnd(&window_id)), window_title(hwnd(&opened)));
    drop(helper);
    drop(fixture);
    assert_eq!(typed["error"]["code"], "input_failed", "{typed}");
    assert_eq!(typed["error"]["message"], "The target window lost the foreground after character 3 of 5; the remaining 2 were not sent.", "{typed}");
    assert!(title.contains("edit=129 |"), "{title}");
    assert!(!other.contains("char:"), "characters went to the window in front: {other}");
    assert_released();
}

/// Ticket 49 (F34): a pointer move the target answers by handing off the foreground succeeds and
/// names the new foreground, like every other input whose events all reached the target.
#[test]
fn a_pointer_move_after_which_the_target_hands_off_the_foreground_succeeds_naming_it() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let interactive = interactive_lock::acquire();
    // Off the fixture, so it gets no move onto its right end before the test sends one.
    unsafe { SetCursorPos(0, 0) }.expect("SetCursorPos");
    let Some((_interactive, fixture, mut helper, window_id)) = open_locked(interactive, &["--handoff-on", "move"]) else { return };
    let at = scaled(&mut helper, &window_id, 460, 330);
    let moved = helper.request("move_mouse", json!({ "window_id": window_id, "x": at.0, "y": at.1 }));
    let opened = wait_for_window(&mut helper, "CUMCP_HANDOFF");
    drop(helper);
    drop(fixture);
    assert_eq!(moved["ok"], true, "the move reached the target before it handed off the foreground: {moved}");
    assert_eq!(moved["result"]["foreground_changed"]["target_closed"], false, "{moved}");
    assert_eq!(moved["result"]["foreground_changed"]["foreground"]["window_id"], opened.as_str(), "{moved}");
}

/// Takes the interactive lock and opens a fixture with `args`, brought to the front; None (with a
/// SKIP line) without a desktop.
fn open(args: &[&str]) -> Option<(interactive_lock::InteractiveLock, KillOnDrop, Helper, String)> {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return None;
    }
    open_locked(interactive_lock::acquire(), args)
}

fn open_locked(interactive: interactive_lock::InteractiveLock, args: &[&str]) -> Option<(interactive_lock::InteractiveLock, KillOnDrop, Helper, String)> {
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(args).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    Some((interactive, fixture, helper, window_id))
}

/// Window pixel (x, y) of the fixture's 480x360 layout, scaled to its size now.
fn scaled(helper: &mut Helper, window_id: &str, x: i64, y: i64) -> (i64, i64) {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    (width * x / 480, height * y / 360)
}

fn click_edit(helper: &mut Helper, window_id: &str) {
    let at = scaled(helper, window_id, 240, 24);
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": at.0, "y": at.1 }));
    assert_eq!(clicked["ok"], true, "{clicked}");
}

fn window_title(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn assert_released() {
    for (name, key) in [("Shift", VK_SHIFT.0), ("Ctrl", VK_CONTROL.0), ("Alt", VK_MENU.0), ("B", 0x42), ("F5", 0x74)] {
        let down = unsafe { (GetAsyncKeyState(i32::from(key)) as u16) & 0x8000 != 0 };
        assert!(!down, "{name} is stuck down");
    }
}

/// Clicks the fixture's background, below the scroll area and right of the Dialog button, where
/// `--click-handoff` acts on the button's release.
fn click_background(helper: &mut Helper, window_id: &str, count: u64) -> Value {
    let at = scaled(helper, window_id, 300, 320);
    helper.request("click", json!({ "window_id": window_id, "x": at.0, "y": at.1, "click_count": count }))
}

fn wait_for_window(helper: &mut Helper, prefix: &str) -> String {
    find_window(helper, prefix).unwrap_or_else(|listed| panic!("{prefix} was not listed: {listed}"))
}

/// The window id of the first listed window whose title starts with `prefix`, waiting up to 5
/// seconds for it; the last `list_windows` answer when none was listed.
fn find_window(helper: &mut Helper, prefix: &str) -> Result<String, Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut listed = Value::Null;
    while Instant::now() < deadline {
        listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with(prefix)).and_then(|_| window["window_id"].as_str().map(str::to_string))
            })
        }) {
            return Ok(id);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(listed)
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
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
