use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetForegroundWindow, GetMessageW, SendMessageTimeoutW, HWND_MESSAGE, MSG, SMTO_ABORTIFHUNG, WM_APP, WS_POPUP,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn helper_foregrounds_the_other_fixture_at_least_nineteen_of_twenty_times() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let first = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture A"));
    let mut helper = Helper::spawn();
    let a = wait_for_new_fixture(&mut helper, &[]);
    let second = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture B"));
    let b = wait_for_new_fixture(&mut helper, &[a.clone()]);
    let mut successes = 0;
    for _ in 0..20 {
        let _ = helper.request("activate_window", json!({ "window_id": b }));
        let activated = helper.request("activate_window", json!({ "window_id": a }));
        if activated["ok"] == true && unsafe { GetForegroundWindow() } == hwnd(&a) {
            successes += 1;
        }
    }
    drop(helper);
    drop(first);
    drop(second);
    assert!(successes >= 19, "foreground succeeded {successes}/20 times");
}

#[test]
fn failed_activation_sends_no_input_and_restores_stacking() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let first = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture A"));
    let mut helper = Helper::spawn();
    let a = wait_for_new_fixture(&mut helper, &[]);
    let second = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture B"));
    let b = wait_for_new_fixture(&mut helper, &[a.clone()]);
    let b_hwnd = hwnd(&b);
    let hidden = message_only_window();
    let placed = helper.request("activate_window", json!({ "window_id": b }));
    assert_eq!(placed["ok"], true, "could not place B in front before the failure case: {placed}");
    assert_eq!(unsafe { GetForegroundWindow() }, b_hwnd, "B was not foreground before the failing activation");
    let before = helper.request("get_window", json!({ "window_id": a }));
    let fg_before = unsafe { GetForegroundWindow() };
    let activated = helper.request("activate_window", json!({ "window_id": (hidden.0 as isize).to_string() }));
    let fg_after = unsafe { GetForegroundWindow() };
    let after = helper.request("get_window", json!({ "window_id": a }));
    drop(helper);
    drop(first);
    drop(second);
    assert_eq!(activated["ok"], false, "{activated}");
    assert_eq!(activated["error"]["code"], "input_failed", "{activated}");
    assert_eq!(fg_after, fg_before, "failed activation changed the foreground window: {activated}");
    assert_eq!(before["result"]["title"], after["result"]["title"], "failed activation changed the fixture");
}

/// A target that already holds the foreground gets no keyboard input from foregrounding: a lone
/// ALT would put an app with a menu bar into menu mode and turn the typed text into mnemonics.
/// The fixture records every system key and ALT key it receives.
#[test]
fn a_target_already_in_front_receives_no_alt_from_activation_or_typing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_new_fixture(&mut helper, &[]);
    let target = hwnd(&window_id);
    let placed = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(placed["ok"], true, "could not foreground the fixture: {placed}");
    assert_eq!(unsafe { GetForegroundWindow() }, target, "refusing to send input: the fixture is not in the foreground");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": width * 240 / 480, "y": height * 24 / 360 }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let before = events_of(&title_of(&mut helper, &window_id));

    for _ in 0..2 {
        assert_eq!(unsafe { GetForegroundWindow() }, target, "refusing to send input: the fixture is not in the foreground");
        let activated = helper.request("activate_window", json!({ "window_id": window_id }));
        assert_eq!(activated["ok"], true, "{activated}");
    }
    assert_eq!(unsafe { GetForegroundWindow() }, target, "refusing to send input: the fixture is not in the foreground");
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "hello" }));
    assert_eq!(typed["ok"], true, "{typed}");
    let title = title_of(&mut helper, &window_id);
    drop(helper);
    drop(fixture);
    let after = events_of(&title);
    // Typing records no event, so anything new is a system key, an ALT, or a menu loop.
    assert_eq!(after, before, "the fixture already in front received ALT or entered menu mode: {title}");
    assert!(title.contains("edit=hello | len=5 |"), "{title}");
}

/// F15: raising the target waits for it to answer WM_WINDOWPOSCHANGING. A target that does not
/// answer it (here the fixture, armed with `--slow-raise`, sleeps 5 s in it) makes activation
/// fail with `input_failed` within about a second instead of holding the request until it does.
#[test]
fn activation_of_a_target_that_does_not_answer_the_raise_fails_in_time() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-raise", "5000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_new_fixture(&mut helper, &[]);
    let target = hwnd(&window_id);
    let placed = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(placed["ok"], true, "could not foreground the fixture before arming it: {placed}");
    // `ARM_SLOW_RAISE` (WM_APP + 1) in `fixture-window.rs`; sent, so it is armed once this returns.
    let mut answer = 0usize;
    let armed = unsafe { SendMessageTimeoutW(target, WM_APP + 1, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 1000, Some(&mut answer)) };
    assert!(armed.0 != 0 && answer == 1, "could not arm the fixture's slow raise");
    let started = Instant::now();
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    let elapsed = started.elapsed();
    drop(helper);
    drop(fixture);
    assert_eq!(activated["error"]["code"], "input_failed", "{activated} after {elapsed:?}");
    assert!(elapsed < Duration::from_secs(3), "activation took {elapsed:?}: {activated}");
}

fn title_of(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

fn events_of(title: &str) -> String {
    title.rsplit(" | ev=").next().unwrap_or("").to_string()
}

fn message_only_window() -> HWND {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let hwnd = CreateWindowExW(
            Default::default(),
            w!("Static"),
            w!("CUMCP_MESSAGE_ONLY"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
        .expect("message-only window");
        tx.send(hwnd.0 as isize).expect("hwnd");
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            DispatchMessageW(&message);
        }
    });
    HWND(rx.recv_timeout(Duration::from_secs(2)).expect("message-only window") as *mut core::ffi::c_void)
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

fn wait_for_new_fixture(helper: &mut Helper, seen: &[String]) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(windows) = listed["result"]["windows"].as_array() {
            if let Some(id) = windows.iter().find_map(|window| {
                let title = window["title"].as_str().unwrap_or("");
                let id = window["window_id"].as_str().unwrap_or("").to_string();
                if title.starts_with("CUMCP_FIXTURE") && !seen.iter().any(|existing| existing == &id) && !id.is_empty() {
                    Some(id)
                } else {
                    None
                }
            }) {
                return id;
            }
        }
        std::thread::sleep(Duration::from_millis(40));
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
            .stderr(Stdio::piped())
            .spawn()
            .expect("helper");
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        Self { child, reader, next_id: 1 }
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        let _ = self.child.stdin.as_ref().unwrap().flush();
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
