//! Ticket 141: `click` at a point (`x`, `y`, window pixels) with `delivery_mode: "background"` on
//! the fixture window. The helper looks for the deepest element of the target's own UI tree that
//! contains the point and has a click pattern (Invoke, Toggle, SelectionItem.Select,
//! ExpandCollapse) and acts on it as an element click does (ticket 140): no hit-test of the
//! screen, so a window that covers the point gets nothing, and the foreground, the windows above
//! the target and the cursor end as they were (ADR 0002). A point with no such element is
//! `background_unavailable` (`no_element_at_point`), a click that needs the real pointer
//! `needs_pointer`; neither sends input. `delivery_mode: "foreground"` is the real mouse, as
//! before. The user's window and the cover are ordinary (activatable) fixture windows.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow, MoveWindow, IsWindowVisible, SetCursorPos, WindowFromPoint, GA_ROOT, GW_HWNDPREV,
};

#[path = "physical_mouse.rs"]
mod physical_mouse;
#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The user's window, a fixture of another process with an edit box, holds the foreground; another
/// process's ordinary window sits between it and the target in the z-order. While the test types
/// into the user's window with SendInput every 15 ms, background clicks at the points of the
/// "Click me" button and the check box (a Win32 check box has Invoke as well as Toggle, and Invoke
/// comes first) act on those controls: each answers the pattern it used, the foreground, the
/// nearest visible window above the target and the cursor are as before, the button and the check
/// box took effect, and every typed key lands in the user's edit.
#[test]
fn a_background_click_at_a_button_point_acts_on_it_and_leaves_the_user_window_in_front() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let left = info["result"]["x"].as_i64().expect("x");
    let width = info["result"]["width"].as_i64().expect("width");

    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
    let user_id = wait_for_new_fixture(&mut helper, &window_id);
    let cover_rect = (left + width - 320, 300, 320, 200);
    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id, &user_id]);
    place_window(&cover, cover_rect);
    for id in [&window_id, &cover, &user_id] {
        let activated = helper.request("activate_window", json!({ "window_id": id }));
        assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
    }

    let target = hwnd(&window_id);
    let user = hwnd(&user_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window is not in front, so the test does not show a target out of front");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the cover is not just above the fixture, so the z-order check proves nothing");
    let user_len_before = edit_length(&title(&mut helper, &user_id));
    let target_len_before = edit_length(&title(&mut helper, &window_id));
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();

    let button = centre_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let check = centre_of(&tree, |element| element["name"] == "Remember me");
    let typist = Typist::start();
    std::thread::sleep(Duration::from_millis(60));
    let pressed = click_at(&mut helper, &window_id, button, json!({}));
    let ticked = click_at(&mut helper, &window_id, check, json!({}));
    std::thread::sleep(Duration::from_millis(60));
    let typed = typist.stop();

    assert_eq!(pressed["ok"], true, "{pressed}");
    assert_eq!(pressed["result"]["action"], "invoke", "{pressed}");
    assert_eq!(ticked["ok"], true, "{ticked}");
    assert_eq!(ticked["result"]["action"], "invoke", "{ticked}");
    assert_eq!(ticked["result"]["foreground_restored"], true, "{ticked}");
    assert!(ticked["result"].get("foreground_changed").is_none(), "{ticked}");
    for answer in [&pressed, &ticked] {
        assert!(answer["result"].get("restore_failed").is_none(), "{answer}");
        assert!(answer["result"]["waited_for_idle_ms"].is_u64(), "{answer}");
    }
    assert!(typed >= 20, "only {typed} keys were typed, so the typing did not span the clicks");

    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground did not end with the user's window");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the target was not put back below the window that was above it");
    assert_eq!(cursor(), cursor_before, "a background click moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut user_title = title(&mut helper, &user_id);
    while edit_length(&user_title) < user_len_before + typed && Instant::now() < deadline {
        user_title = title(&mut helper, &user_id);
    }
    assert_eq!(edit_length(&user_title), user_len_before + typed, "not every typed key landed in the user's window: {user_title}");
    let shown = title(&mut helper, &window_id);
    assert_eq!(edit_length(&shown), target_len_before, "typed keys landed in the target");
    assert!(shown.contains("click:left"), "the button was not pressed: {shown}");
    assert!(!shown.contains("key:") && !shown.contains("char:"), "keys reached the fixture: {shown}");
    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "on"), "the check box did not change");
}

/// Another process's ordinary window sits exactly over the fixture and holds the foreground; its
/// own "Click me" button is where the fixture's is. A background click at that button's point
/// presses the fixture's button (found in the fixture's own tree, not by what is on the screen
/// there): the fixture records the click, the cover's title is exactly what it was, the cover is
/// still in front and still covers the point, and the cursor has not moved.
#[test]
fn a_background_click_at_a_point_another_window_covers_acts_on_the_target_and_the_cover_gets_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));

    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id]);
    place_window(&cover, (left, top, width, height));
    let activated = helper.request("activate_window", json!({ "window_id": cover }));
    assert_eq!(activated["ok"], true, "could not foreground the cover: {activated}");

    let target = hwnd(&window_id);
    let button = centre_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let over = POINT { x: (left + button.0) as i32, y: (top + button.1) as i32 };
    unsafe { SetCursorPos(over.x, over.y) }.expect("SetCursorPos over the button");
    let physical = physical_mouse::PhysicalMouse::start();
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(over), GA_ROOT) }, hwnd(&cover), "the cover does not cover the button, so the test proves nothing");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&cover), "the cover is not in front, so the test does not show a target out of front");
    let above = windows_above(target);
    assert!(above.contains(&hwnd(&cover)), "the cover is not above the fixture in the z-order");
    let cover_before = title(&mut helper, &cover);
    let cursor_before = cursor();

    let pressed = click_at(&mut helper, &window_id, button, json!({}));
    assert_eq!(pressed["ok"], true, "{pressed}");
    assert_eq!(pressed["result"]["action"], "invoke", "{pressed}");
    assert!(pressed["result"].get("restore_failed").is_none(), "{pressed}");
    assert!(pressed["result"].get("foreground_changed").is_none(), "{pressed}");

    let shown = title(&mut helper, &window_id);
    assert!(shown.contains("click:left"), "the fixture's button was not pressed: {shown}");
    assert_eq!(title(&mut helper, &cover), cover_before, "the covering window received something");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&cover), "a background click changed the foreground window");
    assert_eq!(windows_above(target), above, "a background click changed the z-order above the fixture");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(over), GA_ROOT) }, hwnd(&cover), "the fixture was raised over the cover");
    assert_eq!(cursor(), cursor_before, "a background click moved the cursor (physical mouse input meanwhile: {})", physical.seen());
}

/// A point on the edit box (nothing there has a click pattern) is `no_element_at_point`, a point
/// outside the window `input_failed` as in the foreground; a right or middle click, a double or
/// triple click and held modifiers are `needs_pointer`, whatever is at the point. None sends
/// input: the check box stays off, the fixture records nothing, and the foreground, the focus and
/// the cursor are as before.
#[test]
fn a_point_with_nothing_to_act_on_or_a_click_that_needs_the_pointer_is_background_unavailable_and_sends_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("user window"));
    let user_id = wait_for_new_fixture(&mut helper, &window_id);
    let activated = helper.request("activate_window", json!({ "window_id": user_id }));
    assert_eq!(activated["ok"], true, "could not foreground the user's window: {activated}");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, hwnd(&user_id), "the user's window is not in front");
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();
    let focused_before = read_tree(&mut helper, &window_id)["focused_index"].clone();

    let button = centre_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let check = centre_of(&tree, |element| element["name"] == "Remember me");
    let edit = centre_of(&tree, |element| element["role"] == "edit" && element["bounds"]["y"] == 10);
    for (point, extra) in [
        (button, json!({ "button": "right" })),
        (button, json!({ "button": "middle" })),
        (button, json!({ "click_count": 2 })),
        (check, json!({ "click_count": 3 })),
        (check, json!({ "modifiers": ["shift"] })),
        (edit, json!({ "button": "right" })),
        (check, json!({ "button": "right", "click_count": 2, "modifiers": ["ctrl"] })),
    ] {
        let refused = click_at(&mut helper, &window_id, point, extra.clone());
        assert_eq!(refused["error"]["code"], "background_unavailable", "{extra}: {refused}");
        assert_eq!(refused["error"]["data"]["reason"], "needs_pointer", "{extra}: {refused}");
        assert_eq!(refused["error"]["data"]["recommended"], "foreground", "{extra}: {refused}");
        assert_eq!(refused["error"]["data"]["input_sent"], false, "{extra}: {refused}");
        assert!(refused["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{refused}");
    }
    let none = click_at(&mut helper, &window_id, edit, json!({}));
    assert_eq!(none["error"]["code"], "background_unavailable", "{none}");
    assert_eq!(none["error"]["data"]["reason"], "no_element_at_point", "{none}");
    assert_eq!(none["error"]["data"]["recommended"], "foreground", "{none}");
    assert_eq!(none["error"]["data"]["input_sent"], false, "{none}");
    assert!(none["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{none}");
    let outside = click_at(&mut helper, &window_id, (-50, -50), json!({}));
    assert_eq!(outside["error"]["code"], "input_failed", "{outside}");
    assert!(outside["error"]["message"].as_str().unwrap_or("").contains("outside the target window"), "{outside}");

    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "a refused click changed the foreground window");
    assert_eq!(cursor(), cursor_before, "a refused click moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let after = read_tree(&mut helper, &window_id);
    assert_eq!(after["focused_index"], focused_before, "a refused click moved the focus");
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "off"), "a refused click changed the check box");
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");

    let unknown = click_at(&mut helper, &window_id, button, json!({ "delivery_mode": "auto" }));
    assert_eq!(unknown["error"]["code"], "invalid_params", "{unknown}");
}

/// `delivery_mode: "foreground"` is the click with the real mouse, as before: the target is brought
/// forward from behind the user's window, the cursor goes to the point, the button is pressed, and
/// the answer has none of the background fields.
#[test]
fn a_foreground_click_at_a_point_still_uses_the_real_mouse() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");

    let button = centre_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    unsafe { SetCursorPos(0, 0) }.expect("SetCursorPos");
    let clicked = click_at(&mut helper, &window_id, button, json!({ "delivery_mode": "foreground" }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    assert!(clicked["result"].get("action").is_none() && clicked["result"].get("waited_for_idle_ms").is_none(), "{clicked}");
    let now = cursor();
    assert_ne!((now.x, now.y), (0, 0), "a foreground click did not move the cursor to the point");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(now), GA_ROOT) }, hwnd(&window_id), "the cursor is not over the target after a foreground click");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&window_id), "the foreground click left the target out of front");
    assert!(title(&mut helper, &window_id).contains("click:left"), "the foreground click did not press the button");
}

/// A click at window pixel `point`; `extra` adds or overrides params.
fn click_at(helper: &mut Helper, window_id: &str, point: (i64, i64), extra: Value) -> Value {
    let mut params = json!({ "window_id": window_id, "x": point.0, "y": point.1, "delivery_mode": "background" });
    for (key, value) in extra.as_object().expect("extra params") {
        params[key] = value.clone();
    }
    helper.request("click", params)
}

/// The centre of the first element `matches`, in window pixels (the tree's bounds).
fn centre_of(tree: &Value, matches: impl Fn(&Value) -> bool) -> (i64, i64) {
    let element = find(tree, matches);
    let bounds = &element["bounds"];
    let number = |name: &str| bounds[name].as_i64().unwrap_or_else(|| panic!("no {name} in {element}"));
    (number("x") + number("width") / 2, number("y") + number("height") / 2)
}

/// The visible top-level windows above `hwnd`, from the one just above it up to the top of the
/// z-order. Hidden windows are left out: their place relative to the target is nothing the user sees.
fn windows_above(hwnd: HWND) -> Vec<HWND> {
    let mut above = Vec::new();
    let mut current = hwnd;
    for _ in 0..4096 {
        let Ok(previous) = (unsafe { GetWindow(current, GW_HWNDPREV) }) else { break };
        if previous.0.is_null() {
            break;
        }
        if unsafe { IsWindowVisible(previous) }.as_bool() {
            above.push(previous);
        }
        current = previous;
    }
    above
}

fn nearest_visible_above(hwnd: HWND) -> Option<HWND> {
    windows_above(hwnd).first().copied()
}

/// The length of the fixture's edit text, from its title (`| len=<n> |`).
fn edit_length(title: &str) -> usize {
    title.split(" | ").find_map(|part| part.strip_prefix("len=")).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no len= in {title}"))
}

/// Types `k` with SendInput every 15 ms on a thread of its own, into whatever window holds the
/// foreground, as a user typing during the action; `stop` answers how many were typed.
struct Typist {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<usize>,
}

impl Typist {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut typed = 0;
            while !stopped.load(Ordering::SeqCst) {
                let key = |flags| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: 'k' as u16, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
                };
                let inputs = [key(KEYEVENTF_UNICODE), key(KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)];
                assert_eq!(unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) }, 2, "SendInput");
                typed += 1;
                std::thread::sleep(Duration::from_millis(15));
            }
            typed
        });
        Self { stop, handle }
    }

    fn stop(self) -> usize {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.join().expect("typist")
    }
}

fn cursor() -> POINT {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.expect("cursor position");
    point
}

fn read_tree(helper: &mut Helper, window_id: &str) -> Value {
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], true, "{read}");
    read["result"].clone()
}

fn find(tree: &Value, matches: impl Fn(&Value) -> bool) -> &Value {
    let elements = tree["elements"].as_array().expect("elements");
    elements.iter().find(|element| matches(element)).unwrap_or_else(|| panic!("no such element in {tree}"))
}

fn has(element: &Value, list: &str, item: &str) -> bool {
    element[list].as_array().is_some_and(|items| items.iter().any(|value| value == item))
}

/// The fixture's events, in its title; read after the fixture had time to handle what was sent.
fn title(helper: &mut Helper, window_id: &str) -> String {
    std::thread::sleep(Duration::from_millis(150));
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
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
    panic!("the extra fixture was not listed");
}

/// Puts the ordinary (activatable) fixture window at `(left, top, width, height)`.
fn place_window(window_id: &str, (left, top, width, height): (i64, i64, i64, i64)) {
    unsafe { MoveWindow(hwnd(window_id), left as i32, top as i32, width as i32, height as i32, true) }.expect("MoveWindow");
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
}

/// The id of the listed window whose title starts with `prefix`.
fn wait_for_window(helper: &mut Helper, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_windows", json!({}));
        let found = last["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| window["title"].as_str().filter(|title| title.starts_with(prefix)).and_then(|_| Some(window["window_id"].as_str()?.to_string())))
        });
        if let Some(found) = found {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no window titled {prefix}* was listed: {last}");
}

fn wait_for_new_fixture(helper: &mut Helper, seen: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                let title = window["title"].as_str().unwrap_or("");
                let id = window["window_id"].as_str().unwrap_or("").to_string();
                (title.starts_with("CUMCP_FIXTURE") && id != seen && !id.is_empty()).then_some(id)
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("the second fixture was not listed");
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
