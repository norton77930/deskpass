use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// Ticket 137: the checks a background action makes before acting (`prepare_background`, reached
/// through the debug-build request `debug_prepare_background`) do not depend on the foreground. With
/// the fixture's modal dialog open while another window holds the foreground, they report
/// `dialog_open` naming that dialog, as the foreground path does for input aimed at the fixture; a
/// window with no dialog passes; and none of it changes the foreground window.
#[test]
fn a_background_check_names_the_open_dialog_of_a_target_that_is_not_in_front_and_leaves_the_foreground_alone() {
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

    // A opens its modal dialog (the Dialog button), then B takes the foreground from both.
    let activated = helper.request("activate_window", json!({ "window_id": a }));
    assert_eq!(activated["ok"], true, "could not foreground fixture A: {activated}");
    let info = helper.request("get_window", json!({ "window_id": a }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let opened = helper.request("click", json!({ "window_id": a, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "clicking the button that opens the dialog failed: {opened}");
    let dialog = wait_for_dialog(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": b }));
    assert_eq!(activated["ok"], true, "could not foreground fixture B: {activated}");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&b), "B is not in front, so the test does not show a target out of front");

    let refused = helper.request("debug_prepare_background", json!({ "window_id": a }));
    assert_eq!(refused["error"]["code"], "dialog_open", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(dialog), "{refused}");
    assert!(refused["error"]["data"]["title"].as_str().is_some_and(|title| title.starts_with("CUMCP_DIALOG")), "{refused}");
    let message = refused["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.starts_with(&format!("The target window has an open dialog (window_id {dialog}, title \"CUMCP_DIALOG"))
            && message.ends_with("so no input was sent. Target the dialog instead."),
        "{refused}"
    );
    assert_eq!(refused["error"]["data"]["input_sent"], json!(false), "{refused}");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&b), "the background check changed the foreground window");

    // The window in front has no dialog, the dialog has no dialog of its own: both pass.
    for (name, window_id) in [("the window in front", &b), ("the dialog", &dialog)] {
        let passed = helper.request("debug_prepare_background", json!({ "window_id": window_id }));
        assert_eq!(passed["ok"], true, "{name}: {passed}");
        assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&b), "the background check on {name} changed the foreground window");
    }
    drop(helper);
    drop(first);
    drop(second);
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

/// The id of the listed window whose title starts with `CUMCP_DIALOG`.
fn wait_for_dialog(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_windows", json!({}));
        if let Some(id) = last["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_DIALOG")).and_then(|_| Some(window["window_id"].as_str()?.to_string()))
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the dialog was not listed: {last}");
}

fn wait_for_new_fixture(helper: &mut Helper, seen: &[String]) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(windows) = listed["result"]["windows"].as_array() {
            if let Some(id) = windows.iter().find_map(|window| {
                let title = window["title"].as_str().unwrap_or("");
                let id = window["window_id"].as_str().unwrap_or("").to_string();
                (title.starts_with("CUMCP_FIXTURE") && !seen.contains(&id) && !id.is_empty()).then_some(id)
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
