//! Ticket 139: `set_value` with `delivery_mode: "background"` on the fixture window. The value goes
//! in through UI Automation's Value pattern while another window holds the foreground; the Win32
//! proxy may bring the fixture forward, so the helper restores the foreground (ADR 0002): the
//! foreground window, the visible windows above the target and the cursor end as they were, also
//! when another window covers the target, the field still reads back what was sent, and keys the
//! user types meanwhile land in the user's window. A password field is refused without its value
//! in any answer; a minimized target is `background_unavailable`.

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
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow, IsIconic, MoveWindow, IsWindowVisible, SetCursorPos, ShowWindow, WindowFromPoint, GA_ROOT, GW_HWNDPREV, SW_MINIMIZE,
    SW_RESTORE,
};

#[path = "physical_mouse.rs"]
mod physical_mouse;
#[path = "interactive_lock.rs"]
mod interactive_lock;

const VALUE: &str = "背景填值 background 0123";

/// The user's window, a fixture of another process with an edit box, holds the foreground; another
/// process's ordinary window sits between it and the target in the z-order, covering part of the
/// target. While the test types into the user's window with SendInput every 15 ms, a background
/// set_value fills the target's edit: the foreground, the nearest visible window above the target
/// and the cursor are as before, every typed key lands in the user's edit, none in the target's,
/// and the field reads back the value (`read_back_matches`).
#[test]
fn a_background_set_value_while_the_user_types_leaves_the_user_window_in_front_and_every_key_lands_there() {
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
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();

    let edit = index_of(&tree, main_edit);
    let typist = Typist::start();
    std::thread::sleep(Duration::from_millis(60));
    let set = set_value(&mut helper, &window_id, &tree, edit, VALUE);
    std::thread::sleep(Duration::from_millis(60));
    let typed = typist.stop();

    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["read_back"], VALUE, "{set}");
    assert_eq!(set["result"]["read_back_matches"], true, "{set}");
    assert_eq!(set["result"]["foreground_restored"], true, "{set}");
    assert!(set["result"].get("foreground_changed").is_none(), "{set}");
    assert!(set["result"].get("restore_failed").is_none(), "{set}");
    assert!(typed >= 20, "only {typed} keys were typed, so the typing did not span the action");

    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground did not end with the user's window");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the target was not put back below the window that was above it");
    assert_eq!(cursor(), cursor_before, "a background set_value moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut user_title = title(&mut helper, &user_id);
    while edit_length(&user_title) < user_len_before + typed && Instant::now() < deadline {
        user_title = title(&mut helper, &user_id);
    }
    assert_eq!(edit_length(&user_title), user_len_before + typed, "not every typed key landed in the user's window: {user_title}");
    let shown = title(&mut helper, &window_id);
    assert!(shown.contains(&format!("edit={VALUE} | len={}", VALUE.chars().count())), "the target's field does not hold the value, or typed keys landed there: {shown}");
    assert_eq!(find(&read_tree(&mut helper, &window_id), main_edit)["value"], VALUE);
}

/// A background set_value the target fronts itself for, with the helper's debug hook making the next
/// restore fail: the value is still reported set, with `foreground_changed` and `restore_failed`.
#[test]
fn a_failed_restore_still_reports_the_value_set_with_foreground_changed_and_restore_failed() {
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
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the user's window is not in front");

    let armed = helper.request("debug_fail_next_restore", json!({}));
    assert_eq!(armed["ok"], true, "{armed}");
    let edit = index_of(&tree, main_edit);
    let set = set_value(&mut helper, &window_id, &tree, edit, VALUE);
    assert_eq!(set["ok"], true, "the value was not reported set: {set}");
    assert_eq!(set["result"]["read_back_matches"], true, "{set}");
    assert_eq!(set["result"]["restore_failed"], true, "{set}");
    assert!(set["result"]["foreground_changed"].is_object(), "{set}");
    assert!(set["result"].get("foreground_restored").is_none(), "{set}");
    assert_ne!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the restore was not made to fail");
}

/// Another process's ordinary window covers the whole fixture and holds the foreground, and the
/// cursor rests over the covered fixture. A background set_value fills the covered edit and reads
/// it back; afterwards the foreground window, the visible windows above the fixture and the cursor
/// are as before, the fixture is still covered, and no keyboard or pointer input reached it.
#[test]
fn a_background_set_value_on_a_covered_window_fills_the_field_and_leaves_it_covered() {
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

    let cover_rect = (left, top, width, height);
    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id]);
    place_window(&cover, cover_rect);
    let activated = helper.request("activate_window", json!({ "window_id": cover }));
    assert_eq!(activated["ok"], true, "could not foreground the cover: {activated}");

    let target = hwnd(&window_id);
    let middle = POINT { x: (left + width / 2) as i32, y: (top + height / 2) as i32 };
    unsafe { SetCursorPos(middle.x, middle.y) }.expect("SetCursorPos over the fixture");
    let physical = physical_mouse::PhysicalMouse::start();
    let root_at_middle = unsafe { GetAncestor(WindowFromPoint(middle), GA_ROOT) };
    assert_eq!(root_at_middle, hwnd(&cover), "the cover does not cover the fixture, so the test proves nothing");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, hwnd(&cover), "the cover is not in front, so the test does not show a target out of front");
    let above = windows_above(target);
    assert!(above.contains(&hwnd(&cover)), "the cover is not above the fixture in the z-order");
    let cursor_before = cursor();

    let edit = index_of(&tree, main_edit);
    let set = set_value(&mut helper, &window_id, &tree, edit, VALUE);
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["read_back"], VALUE, "{set}");
    assert_eq!(set["result"]["read_back_matches"], true, "{set}");
    assert_eq!(set["result"]["foreground_restored"], true, "{set}");
    assert!(set["result"].get("foreground_changed").is_none(), "{set}");
    assert!(set["result"].get("restore_failed").is_none(), "{set}");

    assert_eq!(find(&read_tree(&mut helper, &window_id), main_edit)["value"], VALUE);
    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "a background set_value changed the foreground window");
    assert_eq!(windows_above(target), above, "a background set_value changed the z-order above the fixture");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(middle), GA_ROOT) }, hwnd(&cover), "the fixture was raised over the cover");
    assert_eq!(cursor(), cursor_before, "a background set_value moved the cursor (physical mouse input meanwhile: {})", physical.seen());

    let shown = title(&mut helper, &window_id);
    assert!(shown.contains(&format!("edit={VALUE} | len={}", VALUE.chars().count())), "{shown}");
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");
}

/// In the background, as in the foreground, a password field is refused (`password_field`) and
/// takes nothing, and no answer carries the value. A minimized target is `background_unavailable`
/// (`minimized`) and stays minimized with the foreground unchanged, and sets nothing; an unknown
/// `delivery_mode` is `invalid_params`.
#[test]
fn a_background_set_value_refuses_a_password_field_and_a_minimized_window_and_an_unknown_mode() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let edit = index_of(&tree, main_edit);
    let password = index_of(&tree, |element| element["role"] == "edit" && has(element, "states", "password"));

    let secret = "Pw-新值-0042";
    let refused = set_value(&mut helper, &window_id, &tree, password, secret);
    assert_eq!(refused["error"]["code"], "password_field", "{refused}");
    assert_eq!(refused["error"]["data"]["input_sent"], false, "{refused}");
    assert!(!refused.to_string().contains(secret) && !refused.to_string().contains("0042"), "the answer carries the value: {refused}");
    assert!(!title(&mut helper, &window_id).contains("pw:len="), "the password field took the value");

    let unknown = helper.request("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": edit, "value": VALUE, "delivery_mode": "auto" }));
    assert_eq!(unknown["error"]["code"], "invalid_params", "{unknown}");
    assert!(!unknown.to_string().contains(VALUE), "{unknown}");

    let target = hwnd(&window_id);
    let _ = unsafe { ShowWindow(target, SW_MINIMIZE) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !unsafe { IsIconic(target) }.as_bool() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(unsafe { IsIconic(target) }.as_bool(), "the fixture did not minimize");
    std::thread::sleep(Duration::from_millis(200));
    let foreground = unsafe { GetForegroundWindow() };

    let refused = set_value(&mut helper, &window_id, &tree, edit, VALUE);
    assert_eq!(refused["error"]["code"], "background_unavailable", "{refused}");
    assert_eq!(refused["error"]["data"]["reason"], "minimized", "{refused}");
    assert_eq!(refused["error"]["data"]["recommended"], "foreground", "{refused}");
    assert_eq!(refused["error"]["data"]["input_sent"], false, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{refused}");
    assert!(!refused.to_string().contains(VALUE), "{refused}");
    assert!(unsafe { IsIconic(target) }.as_bool(), "the refused set_value restored the fixture");
    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "the refused set_value changed the foreground window");
    // A minimized window's tree leaves its controls out: it is restored to read the field.
    let _ = unsafe { ShowWindow(target, SW_RESTORE) };
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(find(&read_tree(&mut helper, &window_id), main_edit)["value"], "", "the refused set_value was done");
    let stderr = helper.stderr();
    assert!(!stderr.contains(secret) && !stderr.contains("0042") && !stderr.contains(VALUE), "the helper logged a value: {stderr}");
}

/// The fixture's main edit: the first one, at the top.
fn main_edit(element: &Value) -> bool {
    element["role"] == "edit" && element["bounds"]["y"] == 10
}

fn set_value(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, value: &str) -> Value {
    helper.request("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": index, "value": value, "delivery_mode": "background" }))
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

fn index_of(tree: &Value, matches: impl Fn(&Value) -> bool) -> u64 {
    let element = find(tree, matches);
    element["index"].as_u64().unwrap_or_else(|| panic!("not numbered: {element}"))
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
    stderr: Option<std::process::ChildStderr>,
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
        let stderr = child.stderr.take();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        Self { child, reader, stderr, next_id: 1 }
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

    /// What the helper wrote to its error output so far: ends the helper, so it is the last call.
    fn stderr(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut text = String::new();
        if let Some(mut stderr) = self.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        }
        text
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
