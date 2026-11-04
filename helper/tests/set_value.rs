//! Ticket 64: `set_value` on an element of a UI tree, on the fixture window. The element's text is
//! replaced through UI Automation's Value pattern, with no keyboard input, and read back.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, SendMessageW, WM_APP};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The fixture's `ARM_SET_TEXT_HANG`: the next WM_SETTEXT another thread sends its main edit
/// stops its UI thread for `wparam` milliseconds.
const ARM_SET_TEXT_HANG: u32 = WM_APP + 3;

/// Chinese text and text far longer than a tree shows go in whole, as they are, and are read
/// back the same: `read_back` is cut to 200 characters with the full length beside it.
#[test]
fn chinese_and_long_text_are_set_and_read_back() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let edit = index_of(&tree, main_edit);

    let chinese = "你好,世界!繁體中文 測試";
    let set = set_value(&mut helper, &window_id, &tree, edit, chinese);
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["read_back"], chinese, "{set}");
    assert_eq!(set["result"]["read_back_matches"], true, "{set}");
    assert!(set["result"].get("read_back_length").is_none(), "{set}");
    assert!(set["result"].get("password").is_none(), "{set}");
    let shown = title(&mut helper, &window_id);
    assert!(shown.contains(&format!("edit={chinese} | len={}", chinese.chars().count())), "{shown}");
    assert!(!shown.contains("key:") && !shown.contains("char:"), "keyboard input reached the fixture: {shown}");

    let long: String = "長文字 long text 0123456789 ".chars().cycle().take(1000).collect();
    let set = set_value(&mut helper, &window_id, &tree, edit, &long);
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["read_back"], long.chars().take(200).collect::<String>(), "{set}");
    assert_eq!(set["result"]["read_back_length"], 1000, "{set}");
    assert_eq!(set["result"]["read_back_matches"], true, "{set}");
    assert!(title(&mut helper, &window_id).contains("| len=1000 |"), "the fixture does not hold all 1000 characters");
    let after = read_tree(&mut helper, &window_id);
    assert_eq!(find(&after, main_edit)["value"], long.chars().take(200).collect::<String>());
}

/// A read-only field and an element without a value are `not_supported`; nothing is typed instead.
#[test]
fn a_read_only_field_and_an_element_without_a_value_are_not_supported() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-read-only").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let read_only = find(&tree, |element| element["role"] == "edit" && element["value"] == "Fixed text");
    assert!(!has(read_only, "actions", "set_value"), "the tree offers set_value on a read-only field: {read_only}");
    let read_only = read_only["index"].as_u64().expect("index");
    let button = index_of(&tree, |element| element["name"] == "Click me");
    let disabled = index_of(&tree, |element| element["role"] == "edit" && has(element, "states", "disabled"));

    // Review minor 2: a disabled field is refused before SetValue, whatever its provider would do.
    // SetValue on a disabled Win32 edit leaves that edit reported as the foreground window, which
    // refuses the next input, so the refusal must come before it.
    let refused = set_value(&mut helper, &window_id, &tree, disabled, "changed");
    assert_eq!(refused["error"]["code"], "not_supported", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("disabled"), "{refused}");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd_of(&window_id), "the refusal moved the foreground");
    let refused = set_value(&mut helper, &window_id, &tree, read_only, "changed");
    assert_eq!(refused["error"]["code"], "not_supported", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("read-only"), "{refused}");
    let refused = set_value(&mut helper, &window_id, &tree, button, "changed");
    assert_eq!(refused["error"]["code"], "not_supported", "{refused}");
    let message = refused["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("does not accept a value directly") && message.contains("computer_type_text"), "{message}");

    // The pointer may rest over the fixture (`hover:`); no key, character or click reached it.
    let shown = title(&mut helper, &window_id);
    assert!(shown.contains("edit= | len=0") && !shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "something reached the fixture: {shown}");
    let after = read_tree(&mut helper, &window_id);
    assert_eq!(find(&after, |element| element["index"] == read_only)["value"], "Fixed text");
    let still = find(&after, |element| element["index"] == disabled);
    assert!(!still.to_string().contains("changed"), "the disabled field took the value: {still}");
}

/// Ticket 92: a password field is refused (`password_field`) and takes nothing; no answer carries
/// the value, and the helper's error output never shows it.
#[test]
fn a_password_field_is_refused_without_the_value_in_the_answer() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let password = index_of(&tree, |element| element["role"] == "edit" && has(element, "states", "password"));

    let secret = "Pw-新值-0042";
    let set = set_value(&mut helper, &window_id, &tree, password, secret);
    assert_eq!(set["error"]["code"], "password_field", "{set}");
    assert_eq!(set["error"]["data"]["input_sent"], false, "{set}");
    assert!(!set.to_string().contains(secret) && !set.to_string().contains("0042"), "the answer carries the value: {set}");
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("pw:len="), "the password field took the value: {shown}");

    // An answer that fails says nothing of the value either.
    let changed = helper.request(
        "set_value",
        json!({ "window_id": window_id, "expected_app": "C:\\Apps\\other.exe", "tree_id": tree["tree_id"], "index": password, "value": secret }),
    );
    assert_eq!(changed["error"]["code"], "app_changed", "{changed}");
    assert!(!changed.to_string().contains(secret), "{changed}");
    let stderr = helper.stderr();
    assert!(!stderr.contains(secret) && !stderr.contains("0042"), "the helper logged the value: {stderr}");
}

/// The window now belongs to another app than the approved one: `app_changed`, nothing set. A
/// stopped helper refuses with `interrupted`; an unknown tree is `no_ui_tree`.
#[test]
fn a_changed_app_a_stop_and_an_unknown_tree_set_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let edit = index_of(&tree, main_edit);

    let changed = helper.request(
        "set_value",
        json!({ "window_id": window_id, "expected_app": "C:\\Apps\\other.exe", "tree_id": tree["tree_id"], "index": edit, "value": "changed" }),
    );
    assert_eq!(changed["error"]["code"], "app_changed", "{changed}");

    let stopped = helper.request("debug_stop", json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let refused = set_value(&mut helper, &window_id, &tree, edit, "stopped");
    assert_eq!(refused["error"]["code"], "interrupted", "{refused}");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");

    let unknown = helper.request("set_value", json!({ "window_id": window_id, "tree_id": "t999-zzzzz", "index": 1, "value": "x" }));
    assert_eq!(unknown["error"]["code"], "no_ui_tree", "{unknown}");
    let untyped = helper.request("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": edit, "value": 5 }));
    assert_eq!(untyped["error"]["code"], "invalid_params", "{untyped}");
    assert!(title(&mut helper, &window_id).contains("edit= | len=0"), "a refused set_value reached the fixture");
}

/// Spec-uia story 54: the app hangs once the value reaches it. The answer is `input_failed` after
/// 3 seconds, saying the app may still act; the stuck worker is left behind and reports nothing
/// when its call returns, and the next request gets a new worker.
#[test]
fn a_set_value_on_a_hung_app_is_input_failed_after_three_seconds_and_nothing_follows() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let edit = index_of(&tree, main_edit);

    let hang = Duration::from_secs(6);
    let armed = unsafe { SendMessageW(hwnd_of(&window_id), ARM_SET_TEXT_HANG, Some(WPARAM(hang.as_millis() as usize)), Some(LPARAM(0))) };
    assert_eq!(armed.0, 1, "the fixture did not arm the hang");
    let started = Instant::now();
    let hung = set_value(&mut helper, &window_id, &tree, edit, "late");
    let waited = started.elapsed();
    assert_eq!(hung["error"]["code"], "input_failed", "{hung}");
    let message = hung["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("within 3 seconds") && message.contains("may still perform the action later"), "{message}");
    assert!(waited >= Duration::from_millis(2900) && waited < Duration::from_millis(4500), "answered after {waited:?}");

    while started.elapsed() < hang + Duration::from_millis(700) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let late = helper.unsolicited();
    assert!(late.iter().all(|line| line.get("id").is_none()), "the left-behind worker answered: {late:?}");
    let shown = title(&mut helper, &window_id);
    assert!(shown.contains("hang:settext"), "the hang was not reached: {shown}");

    let after = read_tree(&mut helper, &window_id);
    let edit = index_of(&after, main_edit);
    let set = set_value(&mut helper, &window_id, &after, edit, "after the hang");
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["result"]["read_back"], "after the hang", "{set}");
}

/// Ticket 70: Esc while the app holds up the value on its way in (SetValue has been called and
/// waits for the edit): the helper answers at once, while set_value still waits on its worker,
/// with the text for a stop after the action, which says it was taken and is not to be repeated.
/// The value goes in once the app answers.
#[test]
fn a_stop_while_the_value_goes_in_says_it_was_set() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = hwnd_of(&window_id);
    let tree = read_tree(&mut helper, &window_id);
    let edit = index_of(&tree, main_edit);

    let hang = Duration::from_secs(2);
    let armed = unsafe { SendMessageW(hwnd, ARM_SET_TEXT_HANG, Some(WPARAM(hang.as_millis() as usize)), Some(LPARAM(0))) };
    assert_eq!(armed.0, 1, "the fixture did not arm the hang");
    let setting = helper.send("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": edit, "value": "set before the stop" }));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !window_title(hwnd).contains("hang:settext") {
        assert!(Instant::now() < deadline, "the hang was not reached: {}", window_title(hwnd));
        std::thread::sleep(Duration::from_millis(20));
    }
    let stop = helper.send("debug_stop", json!({}));
    let stopped_at = Instant::now();
    let stopped = helper.response(setting);
    let took = stopped_at.elapsed();
    assert!(took < Duration::from_millis(1000), "answered {took:?} after the stop, not at once: {stopped}");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    let message = stopped["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("after the action was taken") && message.contains("Do not repeat the action"), "{message}");
    assert!(!message.contains("set before the stop"), "{message}");

    assert_eq!(helper.response(stop)["ok"], true);
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    assert_eq!(find(&read_tree(&mut helper, &window_id), main_edit)["value"], "set before the stop");
}

/// The fixture's title, read directly so it works while the helper is busy.
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

/// The fixture's main edit: the first one, at the top.
fn main_edit(element: &Value) -> bool {
    element["role"] == "edit" && element["bounds"]["y"] == 10
}

fn set_value(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, value: &str) -> Value {
    helper.request("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": index, "value": value }))
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

/// The helper, with its error output kept so a test can look for a value in it.
struct Helper {
    child: Child,
    incoming: Receiver<Value>,
    stderr: Arc<Mutex<String>>,
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
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut pipe = child.stderr.take().expect("stderr");
        let kept = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = pipe.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                kept.lock().unwrap().push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        });
        let ready = incoming.recv_timeout(Duration::from_secs(10)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        Self { child, incoming, stderr, next_id: 1 }
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

    /// The lines the helper wrote since the last answer, without waiting for more.
    fn unsolicited(&mut self) -> Vec<Value> {
        self.incoming.try_iter().collect()
    }

    /// What the helper wrote to its error output so far.
    fn stderr(&self) -> String {
        std::thread::sleep(Duration::from_millis(100));
        self.stderr.lock().unwrap().clone()
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
