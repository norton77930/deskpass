//! Ticket 92: with the keyboard focus in a password field (for `set_value` and `element_action`:
//! with a password field as the element), every tool that sends content is refused with
//! `password_field` before any input, so the field receives nothing; navigation keys still go.
//! Ticket 101: `element_action` `focus` is the one action done to a password field.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn every_input_tool_refuses_a_focused_password_field_and_it_receives_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let password = password_index(&tree);
    let element = json!({ "tree_id": tree["tree_id"], "index": password });
    let clicked = helper.request("click", json!({ "window_id": window_id, "element": element }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    // The fixture records each change of the password box as `pw:len=N` in its title (another
    // process can read neither a password box's text nor its length).
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("pw:"), "{shown}");

    let refused = |answer: &Value, what: &str| {
        assert_eq!(answer["error"]["code"], "password_field", "{what}: {answer}");
        assert_eq!(answer["error"]["data"]["input_sent"], false, "{what}: {answer}");
        let message = answer["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains("password field") && message.contains("user"), "{what}: {message}");
    };
    refused(&helper.request("type_text", json!({ "window_id": window_id, "text": "abc" })), "type_text");
    refused(&helper.request("paste_text", json!({ "window_id": window_id, "text": "abc" })), "paste_text");
    for keys in [json!(["a"]), json!(["shift", "a"]), json!(["7"]), json!(["space"]), json!(["ctrl", "alt", "q"]), json!(["ctrl", "v"]), json!(["shift", "insert"])] {
        refused(&helper.request("press_key", json!({ "window_id": window_id, "keys": keys })), &format!("press_key {keys}"));
    }
    let set = helper.request("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": password, "value": "abc" }));
    refused(&set, "set_value");
    // Ticket 101: every element action but focus, whether or not the field has its pattern.
    for action in ["invoke", "toggle", "expand", "collapse", "select", "scroll_into_view"] {
        let acted = helper.request("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": password, "action": action }));
        refused(&acted, &format!("element_action {action}"));
    }
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("pw:"), "the password field received characters: {shown}");

    // Navigation and editing shortcuts that produce no character still go, so the focus can leave.
    for keys in [json!(["end"]), json!(["left"]), json!(["home"]), json!(["ctrl", "a"]), json!(["tab"])] {
        let pressed = helper.request("press_key", json!({ "window_id": window_id, "keys": keys }));
        assert_eq!(pressed["ok"], true, "press_key {keys}: {pressed}");
    }
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("pw:"), "a navigation key changed the password field: {shown}");
}

/// Ticket 101: `element_action` `focus` puts the keyboard focus in a password field, so the user
/// can type the password; content sent after it is still refused and the field receives nothing.
#[test]
fn focus_is_done_to_a_password_field_and_content_after_it_is_still_refused() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let password = password_index(&tree);
    let focus = helper.request("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": password, "action": "focus" }));
    assert_eq!(focus["ok"], true, "{focus}");
    assert_eq!(focus["result"]["state"], "focused", "{focus}");

    let refused = |answer: &Value, what: &str| {
        assert_eq!(answer["error"]["code"], "password_field", "{what}: {answer}");
        assert_eq!(answer["error"]["data"]["input_sent"], false, "{what}: {answer}");
    };
    refused(&helper.request("type_text", json!({ "window_id": window_id, "text": "abc" })), "type_text");
    refused(&helper.request("paste_text", json!({ "window_id": window_id, "text": "abc" })), "paste_text");
    refused(&helper.request("set_value", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": password, "value": "abc" })), "set_value");
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("pw:"), "the password field received characters: {shown}");
}

/// A Tab typed in the main edit moves the focus to the password box (`--tab-to-password`, as a
/// login form's Tab order does): typing stops right after it, saying the text was partly sent.
#[test]
fn typing_stops_when_a_tab_in_the_text_moves_the_focus_into_a_password_field() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--tab-to-password"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let elements = tree["elements"].as_array().expect("elements");
    let edit = elements
        .iter()
        .find(|element| element["role"] == "edit" && element["bounds"]["y"] == 10)
        .unwrap_or_else(|| panic!("no main edit in {tree}"));
    let clicked = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": tree["tree_id"], "index": edit["index"] } }));
    assert_eq!(clicked["ok"], true, "{clicked}");

    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "abc\tsecret" }));
    assert_eq!(typed["error"]["code"], "password_field", "{typed}");
    assert_eq!(typed["error"]["data"]["input_sent"], true, "{typed}");
    let message = typed["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("4 of 10") && message.contains("user"), "{message}");
    assert!(!message.contains("secret") && !message.contains("abc"), "the message carries the text: {message}");
    let shown = title(&mut helper, &window_id);
    assert!(shown.contains("edit=abc |"), "{shown}");
    assert!(!shown.contains("pw:"), "the password field received characters: {shown}");
}

fn password_index(tree: &Value) -> u64 {
    let elements = tree["elements"].as_array().expect("elements");
    let password = elements
        .iter()
        .find(|element| element["role"] == "edit" && element["states"].as_array().is_some_and(|states| states.iter().any(|state| state == "password")))
        .unwrap_or_else(|| panic!("no password box in {tree}"));
    password["index"].as_u64().expect("index")
}

fn read_tree(helper: &mut Helper, window_id: &str) -> Value {
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], true, "{read}");
    read["result"].clone()
}

/// The fixture's events, in its title; read after the fixture had time to handle what was sent.
fn title(helper: &mut Helper, window_id: &str) -> String {
    std::thread::sleep(Duration::from_millis(150));
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
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
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
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
