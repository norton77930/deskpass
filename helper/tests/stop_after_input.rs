//! Ticket 73: a stop after a click's or a drag's button went down is answered with the text saying
//! the action was taken (as ticket 70 does for the element actions); a stop before it is the plain
//! stop, and nothing is clicked.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, PostMessageW, WM_APP};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The fixture's `HANG_UI`: its UI thread stops for `wparam` milliseconds.
const HANG_UI: u32 = WM_APP + 2;
/// The helper's text for a stop that came before anything was sent.
const INTERRUPTED_MESSAGE: &str = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";
/// How long the scroll area stays busy after the button goes down (`--slow-pointer`): far longer
/// than the helper waits for it to read the release (0.3 s), so the stop can only be answered by
/// the stop notifier, while the helper still waits.
const BUSY_MS: u64 = 3000;

/// The scroll area stays busy after the press (`slow:down`), so the click's wait for its release
/// is under way when Esc comes (`debug_stop`): the answer says the click was taken and is not to be
/// repeated, and the click lands once the fixture is done.
#[test]
fn a_stop_after_a_click_was_sent_says_the_click_was_taken() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-pointer", &BUSY_MS.to_string()]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let (width, height) = activate(&mut helper, &window_id);

    let stopped = stop_once_pressed(&mut helper, &window_id, "click", json!({ "window_id": window_id, "x": width * 200 / 480, "y": height * 160 / 360 }));
    assert_after_action(&stopped);
    wait_for_release(&mut helper, &window_id);
}

/// As for a click: once the drag's button is down (`slow:down`), a stop says the drag was taken;
/// the button is released either way.
#[test]
fn a_stop_after_a_drag_was_pressed_says_the_drag_was_taken() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-pointer", &BUSY_MS.to_string()]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let (width, height) = activate(&mut helper, &window_id);

    let params = json!({ "window_id": window_id, "from_x": width * 30 / 480, "from_y": height * 120 / 360, "to_x": width * 80 / 480, "to_y": height * 180 / 360 });
    let stopped = stop_once_pressed(&mut helper, &window_id, "drag", params);
    assert_after_action(&stopped);
    wait_for_release(&mut helper, &window_id);
}

/// The fixture's UI thread hangs, so a click by element waits for UI Automation to say where the
/// element is, and Esc comes then: nothing was sent, so the answer is the plain stop text, and once
/// the app answers the click is not sent.
#[test]
fn a_stop_before_a_click_was_sent_is_the_plain_stop_and_nothing_is_clicked() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    activate(&mut helper, &window_id);
    let tree = read_tree(&mut helper, &window_id);
    let button = index_of(&tree, |element| element["name"] == "Click me");

    let hang = Duration::from_secs(2);
    unsafe { PostMessageW(Some(hwnd_of(&window_id)), HANG_UI, WPARAM(hang.as_millis() as usize), LPARAM(0)) }.expect("post the hang");
    let hang_started = Instant::now();
    std::thread::sleep(Duration::from_millis(200));
    let clicking = helper.send("click", json!({ "window_id": window_id, "element": { "tree_id": tree["tree_id"], "index": button } }));
    std::thread::sleep(Duration::from_millis(500));
    let stop = helper.send("debug_stop", json!({}));
    let stopped_at = Instant::now();
    let stopped = helper.response(clicking);
    let took = stopped_at.elapsed();
    assert!(took < Duration::from_millis(1000), "answered {took:?} after the stop, not at once: {stopped}");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    assert_eq!(stopped["error"]["message"], INTERRUPTED_MESSAGE, "{stopped}");
    assert_eq!(helper.response(stop)["ok"], true);
    assert!(hang_started.elapsed() >= hang, "the click ended before the app answered again");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    assert!(!title(&mut helper, &window_id).contains("click:"), "the stopped click reached the fixture");
}

/// Sends `method`, and once the fixture shows the button went down (`slow:down` in its title, read
/// directly: the helper is busy) sends `debug_stop`. The answer must come at once, while the
/// fixture is still busy. Then waits for the stopped request to end (the `debug_stop` answer
/// follows it) and resumes; returns the answer.
fn stop_once_pressed(helper: &mut Helper, window_id: &str, method: &str, params: Value) -> Value {
    let hwnd = hwnd_of(window_id);
    assert!(!window_title(hwnd).contains("slow:down"), "pressed before the call: {}", window_title(hwnd));
    let acting = helper.send(method, params);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !window_title(hwnd).contains("slow:down") {
        assert!(Instant::now() < deadline, "the button never went down: {}", window_title(hwnd));
        std::thread::sleep(Duration::from_millis(2));
    }
    let stop = helper.send("debug_stop", json!({}));
    let stopped_at = Instant::now();
    let stopped = helper.response(acting);
    let took = stopped_at.elapsed();
    assert!(took < Duration::from_millis(1000), "answered {took:?} after the stop, not at once: {stopped}");
    assert_eq!(helper.response(stop)["ok"], true);
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    stopped
}

fn assert_after_action(stopped: &Value) {
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    let message = stopped["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("after the action was taken") && message.contains("Do not repeat the action"), "{message}");
    assert!(message.contains("end this turn"), "{message}");
}

/// The click or drag landed: once the busy fixture is done it reads the release (`drag:`), and the
/// button is not left down.
fn wait_for_release(helper: &mut Helper, window_id: &str) {
    let deadline = Instant::now() + Duration::from_millis(BUSY_MS + 3000);
    while !title(helper, window_id).contains("drag:") {
        assert!(Instant::now() < deadline, "the fixture never read the release: {}", title(helper, window_id));
    }
    assert!(unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) } as u16 & 0x8000 == 0, "the left button is stuck down");
}

/// Brings the fixture forward; returns its width and height.
fn activate(helper: &mut Helper, window_id: &str) -> (i64, i64) {
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground.0 as isize, window_id.parse::<isize>().expect("hwnd"), "refusing to send input: the fixture is not in the foreground");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    (info["result"]["width"].as_i64().unwrap_or(480), info["result"]["height"].as_i64().unwrap_or(360))
}

/// The fixture's title, read directly so it works while the helper is busy.
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn read_tree(helper: &mut Helper, window_id: &str) -> Value {
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], true, "{read}");
    read["result"].clone()
}

fn index_of(tree: &Value, matches: impl Fn(&Value) -> bool) -> u64 {
    let elements = tree["elements"].as_array().expect("elements");
    let element = elements.iter().find(|element| matches(element)).unwrap_or_else(|| panic!("no such element in {tree}"));
    element["index"].as_u64().unwrap_or_else(|| panic!("not numbered: {element}"))
}

/// The fixture's events, in its title; read after the fixture had time to handle what was sent.
fn title(helper: &mut Helper, window_id: &str) -> String {
    std::thread::sleep(Duration::from_millis(150));
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

fn hwnd_of(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        let found = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).map(|_| window["window_id"].as_str().unwrap_or("").to_string())
            })
        });
        if let Some(found) = found {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the fixture was not listed");
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else {
            return false;
        };
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
