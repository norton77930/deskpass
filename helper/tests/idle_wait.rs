//! Ticket 149: every background action first waits for the user to stop (keyboard idle 300 ms, no
//! mouse button held), at most 3 s. A test's SendInput is injected and never counts, so physical
//! input is marked with the debug-only `debug_user_input` (spec-background testing decisions, hook
//! 1); the test's own SendInput typing shows that injected input does not delay the action. Every
//! test starts the session (`session_start`), which sets the low-level hooks the wait reads.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The helper's `interrupted` text for a stop before the action (`esc::INTERRUPTED_MESSAGE`).
const INTERRUPTED_MESSAGE: &str = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";

/// The user types for 600 ms: the toggle waits until 300 ms after the last key (about 900 ms), then
/// changes the check box. Then the user holds a mouse button for 500 ms: the next toggle waits
/// about that long. Neither wait reaches the cap.
#[test]
fn a_background_action_waits_until_the_user_stops_typing_or_releases_the_mouse() {
    let Some(mut scene) = Scene::start() else { return };
    let check = scene.check();

    scene.mark(json!({ "typing_ms": 600 }));
    let started = Instant::now();
    let toggled = scene.act(check, "toggle");
    let took = started.elapsed();
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    let waited = waited_ms(&toggled);
    assert!((750..=1400).contains(&waited), "waited {waited} ms for typing that ended 600 ms in: {toggled}");
    assert!(took >= Duration::from_millis(waited), "the call took {took:?}, less than the {waited} ms it says it waited");
    assert!(toggled["result"].get("idle_wait_capped").is_none(), "{toggled}");

    scene.mark(json!({ "mouse_held_ms": 500 }));
    let toggled = scene.act(check, "toggle");
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "off", "{toggled}");
    let waited = waited_ms(&toggled);
    assert!((400..=1100).contains(&waited), "waited {waited} ms for a button held 500 ms: {toggled}");
    assert!(toggled["result"].get("idle_wait_capped").is_none(), "{toggled}");
}

/// The user never stops within 3 s: the action waits 3 s, goes ahead anyway and says it was capped.
#[test]
fn a_user_who_does_not_stop_holds_the_action_up_3_s_and_then_it_goes_ahead() {
    let Some(mut scene) = Scene::start() else { return };
    let check = scene.check();
    scene.mark(json!({ "typing_ms": 8000 }));
    let toggled = scene.act(check, "toggle");
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "the capped action was not done: {toggled}");
    let waited = waited_ms(&toggled);
    assert!((3000..=3500).contains(&waited), "waited {waited} ms, not the 3 s cap: {toggled}");
    assert_eq!(toggled["result"]["idle_wait_capped"], true, "{toggled}");
}

/// The test types into the user's window with SendInput every 15 ms throughout the action: that is
/// injected input, so the action does not wait (`waited_for_idle_ms: 0`).
#[test]
fn injected_typing_does_not_hold_a_background_action_up() {
    let Some(mut scene) = Scene::start() else { return };
    let check = scene.check();
    let typist = Typist::start();
    std::thread::sleep(Duration::from_millis(100));
    let toggled = scene.act(check, "toggle");
    let typed = typist.stop();
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert_eq!(toggled["result"]["waited_for_idle_ms"], 0, "injected typing held the action up: {toggled}");
    assert!(toggled["result"].get("idle_wait_capped").is_none(), "{toggled}");
    assert!(typed >= 8, "only {typed} keys were typed, so the typing did not span the action");
}

/// A background set_value goes through the same wait (`act_on_element`).
#[test]
fn a_background_set_value_waits_for_the_user_too() {
    let Some(mut scene) = Scene::start() else { return };
    let edit = index_of(&scene.tree, |element| element["role"] == "edit" && element["bounds"]["y"] == 10);
    scene.mark(json!({ "typing_ms": 400 }));
    let set = scene.helper.request("set_value", json!({ "window_id": scene.window_id, "tree_id": scene.tree["tree_id"], "index": edit, "value": "idle 149", "delivery_mode": "background" }));
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["read_back_matches"], true, "{set}");
    let waited = waited_ms(&set);
    assert!((550..=1200).contains(&waited), "waited {waited} ms for typing that ended 400 ms in: {set}");
    assert!(set["result"].get("idle_wait_capped").is_none(), "{set}");
}

/// The user is typing and presses the stop key while the action waits: the answer is the plain
/// stop (`interrupted`, the server's `stopped_by_user` path), at once, and the check box is never
/// toggled, not even once the wait would have ended.
#[test]
fn a_stop_during_the_wait_is_interrupted_and_nothing_is_done() {
    let Some(mut scene) = Scene::start() else { return };
    let check = scene.check();
    scene.mark(json!({ "typing_ms": 8000 }));
    let started = Instant::now();
    let acting = scene.helper.send("element_action", json!({ "window_id": scene.window_id, "tree_id": scene.tree["tree_id"], "index": check, "action": "toggle", "delivery_mode": "background" }));
    std::thread::sleep(Duration::from_millis(500));
    let stop = scene.helper.send("debug_stop", json!({}));
    let stopped_at = Instant::now();
    let stopped = scene.helper.response(acting);
    let took = stopped_at.elapsed();
    assert!(took < Duration::from_millis(1000), "answered {took:?} after the stop, not at once: {stopped}");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    assert_eq!(stopped["error"]["message"], INTERRUPTED_MESSAGE, "{stopped}");
    assert_eq!(scene.helper.response(stop)["ok"], true);
    // Past the 3 s cap: a wait that went on after the stop would have toggled by now.
    std::thread::sleep(Duration::from_millis(3500).saturating_sub(started.elapsed()));
    let resumed = scene.helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    assert!(has(find(&scene.read_tree(), |element| element["name"] == "Remember me"), "states", "off"), "the stopped toggle was done");
}

/// The target fixture (with UI controls) and a second fixture as the user's window, in front, with
/// the session started.
struct Scene {
    helper: Helper,
    window_id: String,
    tree: Value,
    // Dropped in this order: the helper and fixtures end before the lock is released.
    _fixture: KillOnDrop,
    _user: KillOnDrop,
    _interactive: interactive_lock::InteractiveLock,
}

impl Scene {
    fn start() -> Option<Scene> {
        if !interactive_desktop() {
            eprintln!("SKIP interactive: no desktop");
            return None;
        }
        let interactive = interactive_lock::acquire();
        let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
        let mut helper = Helper::spawn();
        let started = helper.request("session_start", json!({}));
        assert_eq!(started["ok"], true, "{started}");
        let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE", None);
        let user = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
        let user_id = wait_for_window(&mut helper, "CUMCP_FIXTURE", Some(&window_id));
        let activated = helper.request("activate_window", json!({ "window_id": user_id }));
        assert_eq!(activated["ok"], true, "could not foreground the user's window: {activated}");
        assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the user's window is not in front");
        let mut scene = Scene { helper, window_id, tree: Value::Null, _fixture: fixture, _user: user, _interactive: interactive };
        scene.tree = scene.read_tree();
        Some(scene)
    }

    fn check(&self) -> u64 {
        index_of(&self.tree, |element| element["name"] == "Remember me")
    }

    fn mark(&mut self, params: Value) {
        let marked = self.helper.request("debug_user_input", params);
        assert_eq!(marked["ok"], true, "{marked}");
    }

    fn act(&mut self, index: u64, action: &str) -> Value {
        self.helper.request("element_action", json!({ "window_id": self.window_id, "tree_id": self.tree["tree_id"], "index": index, "action": action, "delivery_mode": "background" }))
    }

    fn read_tree(&mut self) -> Value {
        let read = self.helper.request("get_ui_tree", json!({ "window_id": self.window_id }));
        assert_eq!(read["ok"], true, "{read}");
        read["result"].clone()
    }
}

fn waited_ms(answer: &Value) -> u64 {
    answer["result"]["waited_for_idle_ms"].as_u64().unwrap_or_else(|| panic!("no waited_for_idle_ms: {answer}"))
}

/// Types `k` with SendInput every 15 ms on a thread of its own, into whatever window holds the
/// foreground; `stop` answers how many were typed.
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

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
}

/// The id of the listed window whose title starts with `prefix`, other than `other`.
fn wait_for_window(helper: &mut Helper, prefix: &str, other: Option<&str>) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_windows", json!({}));
        let found = last["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                let id = window["window_id"].as_str().filter(|id| !id.is_empty() && Some(*id) != other)?;
                window["title"].as_str().filter(|title| title.starts_with(prefix)).map(|_| id.to_string())
            })
        });
        if let Some(found) = found {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no window titled {prefix}* was listed: {last}");
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
    incoming: Receiver<Value>,
    next_id: u64,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("helper");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let ready = incoming.recv_timeout(Duration::from_secs(10)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        Self { child, incoming, next_id: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.response(id)
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
        id
    }

    /// The answer to request `id`; events and answers to other requests are skipped.
    fn response(&mut self, id: u64) -> Value {
        loop {
            let value = self.incoming.recv_timeout(Duration::from_secs(15)).expect("response");
            if value.get("event").is_some() || value["id"].as_u64() != Some(id) {
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
