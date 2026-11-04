//! Ticket 145 (spec-background "Stop"): a physical Ctrl+Alt+Esc stops Deskpass at any time; a
//! physical single Esc stops it only while a foreground action is in progress or the border of the
//! most recent foreground action still shows, and otherwise reaches the user's app like any key.
//! Keys the AI sends (injected) never count.
//!
//! A test's SendInput is injected, so it can never be a physical key. Test builds of the helper
//! take an injected key event whose `dwExtraInfo` is [`PHYSICAL_MARK`] as physical (debug-only,
//! like `debug_user_input` of ticket 149): the marked keys go through the real low-level hook and
//! its decision, and on to the user's window like a real key. Unmarked SendInput keys are what an
//! injected key looks like. The user's window is an ordinary, activatable fixture of another
//! process, in front, whose edit keeps the focus and records an Esc (`key:escape`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_ESCAPE, VK_LCONTROL, VK_LMENU,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindow, GetWindowTextW, MoveWindow, GW_HWNDNEXT};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The `dwExtraInfo` that makes test builds of the helper take an injected key as physical
/// (`esc::DEBUG_PHYSICAL_MARK`).
const PHYSICAL_MARK: usize = 0x4445_534B_5048_5953;

/// The helper's `interrupted` text for a stop before the action (`esc::INTERRUPTED_MESSAGE`).
const INTERRUPTED_MESSAGE: &str = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";

/// How long a key that is not to stop is given to (wrongly) stop.
const NO_STOP_WITHIN: Duration = Duration::from_millis(600);

/// The user presses Esc in their own app while a background action is under way (waiting for the
/// user to stop typing): the action is not stopped and goes ahead, no stop is reported, and the
/// user's app receives the Esc.
#[test]
fn a_physical_esc_during_a_background_action_reaches_the_users_app_and_does_not_stop() {
    let Some(mut scene) = Scene::start() else { return };
    scene.show("background");
    let check = scene.check();
    scene.mark_typing(1500);
    let acting = scene.helper.send("element_action", json!({ "window_id": scene.target_id, "tree_id": scene.tree["tree_id"], "index": check, "action": "toggle", "delivery_mode": "background" }));
    std::thread::sleep(Duration::from_millis(400));
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    let acted = scene.helper.response(acting);
    assert_eq!(acted["ok"], true, "a single Esc stopped a background action: {acted}");
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "a single Esc during a background action was reported as a stop");
    assert!(scene.user_title().contains("key:escape"), "the user's app did not receive the Esc: {}", scene.user_title());
}

/// The user presses Ctrl+Alt+Esc while a background action is under way: the action is answered
/// with the plain stop (`interrupted`, the server's `stopped_by_user`), a stop is reported, and input
/// stays refused until the server resumes, as for today's Esc.
#[test]
fn a_physical_ctrl_alt_esc_during_a_background_action_stops_it() {
    let Some(mut scene) = Scene::start() else { return };
    scene.show("background");
    let check = scene.check();
    scene.mark_typing(3000);
    let acting = scene.helper.send("element_action", json!({ "window_id": scene.target_id, "tree_id": scene.tree["tree_id"], "index": check, "action": "toggle", "delivery_mode": "background" }));
    std::thread::sleep(Duration::from_millis(400));
    send_ctrl_alt_esc(true);
    let acted = scene.helper.response(acting);
    assert_eq!(acted["error"]["code"], "interrupted", "Ctrl+Alt+Esc did not stop the background action: {acted}");
    assert_eq!(acted["error"]["message"], INTERRUPTED_MESSAGE, "{acted}");
    assert!(scene.helper.stop_within(Duration::from_secs(2)), "Ctrl+Alt+Esc was not reported as a stop");
    let refused = scene.helper.request("element_action", json!({ "window_id": scene.target_id, "tree_id": scene.tree["tree_id"], "index": check, "action": "toggle", "delivery_mode": "background" }));
    assert_eq!(refused["error"]["code"], "interrupted", "input ran while stopped: {refused}");
    let resumed = scene.helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
}

/// Windows takes Ctrl+Alt+Esc as Alt+Esc and sends the front window to the back. When the helper
/// counts a physical Ctrl+Alt+Esc as a stop it swallows the Esc, so neither Windows nor the user's
/// app sees it: the user's window stays in front, on top of the z-order, and records no Esc.
#[test]
fn a_physical_ctrl_alt_esc_that_stops_is_swallowed() {
    let Some(mut scene) = Scene::start() else { return };
    send_ctrl_alt_esc(true);
    assert!(scene.helper.stop_within(Duration::from_secs(2)), "Ctrl+Alt+Esc was not reported as a stop");
    std::thread::sleep(Duration::from_millis(300));
    let user = hwnd(&scene.user_id);
    let target = hwnd(&scene.target_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "Windows sent the user's front window to the back (Alt+Esc)");
    assert!(is_above(user, target), "the user's window is no longer above the target in the z-order");
    assert!(!scene.user_title().contains("key:escape"), "the user's app received the swallowed Esc: {}", scene.user_title());
}

/// Ticket 153: the user stops with Ctrl+Alt+Esc, lets go of Ctrl and Alt first and keeps Esc held
/// (its auto-repeat goes on) before letting go of it last. The press was swallowed, so its repeats
/// and its release are too: the user's app never sees an Esc press without its release.
#[test]
fn a_ctrl_alt_esc_stop_released_esc_last_reaches_the_users_app_as_no_unpaired_esc() {
    let Some(mut scene) = Scene::start() else { return };
    send_keys(&[(VK_LCONTROL, false), (VK_LMENU, false), (VK_ESCAPE, false), (VK_ESCAPE, false)], true);
    assert!(scene.helper.stop_within(Duration::from_secs(2)), "Ctrl+Alt+Esc was not reported as a stop");
    send_keys(&[(VK_LCONTROL, true), (VK_LMENU, true), (VK_ESCAPE, false), (VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    std::thread::sleep(Duration::from_millis(300));
    let title = scene.user_title();
    let presses = title.matches("key:escape").count();
    let releases = title.matches("keyup:escape").count();
    assert_eq!(presses > 0, releases > 0, "the user's app got an Esc press without its release, or a release without its press: {title}");
}

/// Whether `upper` is above `lower` in the z-order (`lower` is found walking down from `upper`).
fn is_above(upper: HWND, lower: HWND) -> bool {
    let mut current = upper;
    while let Ok(next) = unsafe { GetWindow(current, GW_HWNDNEXT) } {
        if next == lower {
            return true;
        }
        current = next;
    }
    false
}

/// While the border of a foreground action shows, a single Esc stops; once the border is hidden,
/// a single Esc is the user's own again.
#[test]
fn a_physical_esc_stops_while_a_foreground_border_shows_and_not_after_it_hides() {
    let Some(mut scene) = Scene::start() else { return };
    scene.show("foreground");
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    assert!(scene.helper.stop_within(Duration::from_secs(2)), "a single Esc while the foreground border shows did not stop");
    let resumed = scene.helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");

    let hidden = scene.helper.request("overlay_hide", json!({}));
    assert_eq!(hidden["ok"], true, "{hidden}");
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "a single Esc stopped after the foreground border was hidden");

    // A background border after a foreground one is not the foreground action's border.
    scene.show("foreground");
    scene.show("background");
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "a single Esc stopped while a background border replaced the foreground one");
}

/// A single Esc during a foreground action (typing) stops it, border or not.
#[test]
fn a_physical_esc_stops_a_foreground_action_in_progress() {
    let Some(mut scene) = Scene::start() else { return };
    let text: String = (0..300).map(|index| (b'a' + (index % 26) as u8) as char).collect();
    let typing = scene.helper.send("type_text", json!({ "window_id": scene.target_id, "text": text }));
    std::thread::sleep(Duration::from_millis(400));
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    let typed = scene.helper.response(typing);
    assert_eq!(typed["error"]["code"], "interrupted", "a single Esc did not stop a foreground action: {typed}");
    assert!(scene.helper.stop_within(Duration::from_secs(2)), "the stop was not reported");
}

/// Injected Esc and Ctrl+Alt+Esc, the test's own and the helper's (`press_key`), never stop, even
/// while a foreground border shows; nor does a physical Esc with an injected Ctrl and Alt held
/// while a background border shows.
#[test]
fn injected_esc_and_ctrl_alt_esc_never_stop() {
    let Some(mut scene) = Scene::start() else { return };
    scene.show("foreground");
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], false);
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "an injected Esc stopped");
    send_ctrl_alt_esc(false);
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "an injected Ctrl+Alt+Esc stopped");
    let user_id = scene.user_id.clone();
    // Windows takes Ctrl+Alt+Esc as Alt+Esc (the window in front goes to the back), so press_key
    // may answer that its target lost the foreground; it must not answer a stop.
    let pressed = scene.helper.request("press_key", json!({ "window_id": user_id, "keys": ["ctrl", "alt", "escape"] }));
    assert_ne!(pressed["error"]["code"], "interrupted", "{pressed}");
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "the helper's own Ctrl+Alt+Esc stopped");

    scene.show("background");
    send_keys(&[(VK_LCONTROL, false), (VK_LMENU, false)], false);
    send_keys(&[(VK_ESCAPE, false), (VK_ESCAPE, true)], true);
    send_keys(&[(VK_LMENU, true), (VK_LCONTROL, true)], false);
    assert!(scene.helper.no_stop_within(NO_STOP_WITHIN), "a physical Esc with injected Ctrl and Alt held stopped in the background");
}

/// Ctrl and Alt pressed, Esc pressed and released, Alt and Ctrl released, like a user does it.
fn send_ctrl_alt_esc(physical: bool) {
    send_keys(&[(VK_LCONTROL, false), (VK_LMENU, false), (VK_ESCAPE, false), (VK_ESCAPE, true), (VK_LMENU, true), (VK_LCONTROL, true)], physical);
}

/// Sends `(key, up)` events with SendInput; `physical` marks them with [`PHYSICAL_MARK`].
fn send_keys(keys: &[(VIRTUAL_KEY, bool)], physical: bool) {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|&(key, up)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: if physical { PHYSICAL_MARK } else { 0 },
                },
            },
        })
        .collect();
    assert_eq!(unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize, inputs.len(), "SendInput");
    std::thread::sleep(Duration::from_millis(50));
}

/// The target fixture (with UI controls) and the user's window (another fixture process, its edit
/// keeping the focus) in front, with the session started. The overlay hides when the helper ends.
struct Scene {
    helper: Helper,
    target_id: String,
    user_id: String,
    tree: Value,
    // Dropped in this order: the helper and fixtures end before the lock is released.
    _target: KillOnDrop,
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
        let target = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
        let mut helper = Helper::spawn();
        let started = helper.request("session_start", json!({}));
        assert_eq!(started["ok"], true, "{started}");
        let target_id = wait_for_fixture_besides(&mut helper, &[]);
        place_window(&target_id, (200, 200, 400, 300));
        let user = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
        let user_id = wait_for_fixture_besides(&mut helper, &[&target_id]);
        place_window(&user_id, (650, 200, 400, 300));
        let activated = helper.request("activate_window", json!({ "window_id": user_id }));
        assert_eq!(activated["ok"], true, "could not foreground the user's window: {activated}");
        assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the user's window is not in front");
        let mut scene = Scene { helper, target_id, user_id, tree: Value::Null, _target: target, _user: user, _interactive: interactive };
        let read = scene.helper.request("get_ui_tree", json!({ "window_id": scene.target_id }));
        assert_eq!(read["ok"], true, "{read}");
        scene.tree = read["result"].clone();
        Some(scene)
    }

    fn show(&mut self, delivery: &str) {
        let shown = self.helper.request("overlay_show", json!({ "window_id": self.target_id, "app_name": "示範程式", "delivery_mode": delivery }));
        assert_eq!(shown["ok"], true, "{shown}");
    }

    fn check(&self) -> u64 {
        let elements = self.tree["elements"].as_array().expect("elements");
        let element = elements.iter().find(|element| element["name"] == "Remember me").unwrap_or_else(|| panic!("no check box in {}", self.tree));
        element["index"].as_u64().expect("index")
    }

    /// The user types (physically, `debug_user_input`) for `ms`, so a background action waits.
    fn mark_typing(&mut self, ms: u64) {
        let marked = self.helper.request("debug_user_input", json!({ "typing_ms": ms }));
        assert_eq!(marked["ok"], true, "{marked}");
    }

    fn user_title(&self) -> String {
        let mut buffer = [0u16; 512];
        let length = unsafe { GetWindowTextW(hwnd(&self.user_id), &mut buffer) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }
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
    /// `stopped` events seen and not yet taken by `stop_within`.
    stops: usize,
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
        Self { child, incoming, next_id: 1, stops: 0 }
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

    /// The answer to request `id`; `stopped` events are counted, other lines skipped.
    fn response(&mut self, id: u64) -> Value {
        loop {
            let value = self.incoming.recv_timeout(Duration::from_secs(15)).expect("response");
            self.note(&value);
            if value.get("event").is_some() || value["id"].as_u64() != Some(id) {
                continue;
            }
            return value;
        }
    }

    fn note(&mut self, value: &Value) {
        if value["event"] == "stopped" {
            self.stops += 1;
        }
    }

    /// Whether a `stopped` event comes (or came) within `limit`; takes all seen so far.
    fn stop_within(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.stops == 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.incoming.recv_timeout(left) {
                Ok(value) => self.note(&value),
                Err(_) => break,
            }
        }
        std::mem::take(&mut self.stops) > 0
    }

    /// No `stopped` event came so far nor comes within `limit`.
    fn no_stop_within(&mut self, limit: Duration) -> bool {
        !self.stop_within(limit)
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
