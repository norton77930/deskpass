//! Ticket 65: `element_action` on elements of a UI tree, on the fixture window: invoke, toggle,
//! expand, collapse, select, scroll_into_view and focus through UI Automation's patterns, with
//! the state read back afterwards.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, SendMessageW, WM_APP};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The fixture's `ARM_COMMAND_HANG`: the next button click (BN_CLICKED) or combo box opening
/// (CBN_DROPDOWN) its window is told of stops its UI thread for `wparam` milliseconds.
const ARM_COMMAND_HANG: u32 = WM_APP + 4;
/// The fixture's `ARM_QUERY_HANG`: the next check-state request to its check box stops its UI
/// thread for `wparam` milliseconds.
const ARM_QUERY_HANG: u32 = WM_APP + 5;

/// The check box toggles, the combo box opens and closes, a list row is selected, a tree item
/// expands and the edit takes the focus; each answer reads the new state back, and a new tree
/// shows it.
#[test]
fn toggle_expand_collapse_select_and_focus_take_effect_and_are_read_back() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);

    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert_eq!(toggled["result"]["previous_state"], "off", "{toggled}");
    assert!(toggled["result"].get("foreground_changed").is_none(), "{toggled}");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "on"));

    let combo = index_of(&tree, |element| element["role"] == "combo box");
    let expanded = act(&mut helper, &window_id, &tree, combo, "expand");
    assert_eq!(expanded["ok"], true, "{expanded}");
    assert_eq!(expanded["result"]["state"], "expanded", "{expanded}");
    let collapsed = act(&mut helper, &window_id, &tree, combo, "collapse");
    assert_eq!(collapsed["ok"], true, "{collapsed}");
    assert_eq!(collapsed["result"]["state"], "collapsed", "{collapsed}");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["role"] == "combo box"), "states", "collapsed"));

    let row = index_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    let selected = act(&mut helper, &window_id, &tree, row, "select");
    assert_eq!(selected["ok"], true, "{selected}");
    assert_eq!(selected["result"]["state"], "selected", "{selected}");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Row 2"), "states", "selected"));

    let vegetables = index_of(&tree, |element| element["role"] == "tree item" && element["name"] == "Vegetables");
    let opened = act(&mut helper, &window_id, &tree, vegetables, "expand");
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(opened["result"]["state"], "expanded", "{opened}");
    let after = read_tree(&mut helper, &window_id);
    find(&after, |element| element["role"] == "tree item" && element["name"] == "Carrot");

    let edit = index_of(&tree, main_edit);
    let focused = act(&mut helper, &window_id, &tree, edit, "focus");
    assert_eq!(focused["ok"], true, "{focused}");
    assert_eq!(focused["result"]["state"], "focused", "{focused}");
    let after = read_tree(&mut helper, &window_id);
    assert_eq!(after["focused_index"], find(&after, main_edit)["index"], "{after}");

    // No keyboard or pointer input reached the fixture (the pointer may rest over it: `hover:`).
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");
}

/// Item 5 of the `--ui-scroll-tree` tree view shows only its top half; scroll_into_view scrolls
/// the tree until all of it shows. There is no state to read back.
#[test]
fn scroll_into_view_shows_the_half_visible_item_whole() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-content", "--ui-scroll-tree"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    // A tree item's rectangle is the part of it the tree shows.
    let height = |element: &Value| element["bounds"]["height"].as_i64().expect("height");
    let whole = height(find(&tree, |element| element["name"] == "Item 1"));
    let item5 = find(&tree, |element| element["name"] == "Item 5");
    assert!(height(item5) < whole, "Item 5 is not cut off, so this test proves nothing: {item5}");
    assert!(has(item5, "actions", "scroll_into_view"), "{item5}");
    let item5 = item5["index"].as_u64().expect("index");

    let scrolled = act(&mut helper, &window_id, &tree, item5, "scroll_into_view");
    assert_eq!(scrolled["ok"], true, "{scrolled}");
    assert!(scrolled["result"].get("state").is_none(), "{scrolled}");
    let after = read_tree(&mut helper, &window_id);
    let item5 = find(&after, |element| element["name"] == "Item 5");
    assert_eq!(height(item5), whole, "Item 5 is still cut off: {item5}");
    assert!(!after["elements"].as_array().expect("elements").iter().any(|element| element["name"] == "Item 1"), "the tree did not scroll: {after}");
}

/// Invoke presses the button (the fixture records the click); an action the element does not
/// have is `not_supported` and lists the ones it has; a disabled element is `not_supported`
/// before any pattern is used, and the target keeps the foreground.
#[test]
fn invoke_presses_the_button_and_unsupported_or_disabled_elements_are_not_supported() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);

    let button = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    assert!(!title(&mut helper, &window_id).contains("click:left"));
    let invoked = act(&mut helper, &window_id, &tree, button, "invoke");
    assert_eq!(invoked["ok"], true, "{invoked}");
    assert!(invoked["result"].get("state").is_none(), "{invoked}");
    let shown = title(&mut helper, &window_id);
    assert!(shown.contains("click:left"), "the button was not pressed: {shown}");

    let check = find(&tree, |element| element["name"] == "Remember me");
    let refused = act(&mut helper, &window_id, &tree, check["index"].as_u64().expect("index"), "expand");
    assert_eq!(refused["error"]["code"], "not_supported", "{refused}");
    let supported: Vec<&str> = refused["error"]["data"]["supported_actions"].as_array().expect("supported_actions").iter().filter_map(Value::as_str).collect();
    assert!(supported.contains(&"toggle") && !supported.contains(&"expand"), "{refused}");
    // As the tree lists them, without what computer_element_action does not do.
    let listed: Vec<&str> = check["actions"].as_array().expect("actions").iter().filter_map(Value::as_str).filter(|action| *action != "click" && *action != "set_value").collect();
    assert_eq!(supported, listed, "{refused} {check}");
    let message = refused["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("expand") && message.contains("toggle"), "{message}");
    assert_eq!(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me")["states"], check["states"], "the refused action changed the check box");

    let disabled = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Disabled");
    let refused = act(&mut helper, &window_id, &tree, disabled, "invoke");
    assert_eq!(refused["error"]["code"], "not_supported", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("disabled"), "{refused}");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd_of(&window_id), "the refusal moved the foreground");
}

/// An action that is not one of the seven is `invalid_params` before anything is sent; the window
/// of another app than the approved one is `app_changed`; a stopped helper answers `interrupted`.
#[test]
fn an_unknown_action_a_changed_app_and_a_stop_act_on_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");

    for action in [json!("explode"), json!(""), json!(5), Value::Null] {
        let refused = helper.request("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": check, "action": action }));
        assert_eq!(refused["error"]["code"], "invalid_params", "{action}: {refused}");
    }
    let changed = helper.request(
        "element_action",
        json!({ "window_id": window_id, "expected_app": "C:\\Apps\\other.exe", "tree_id": tree["tree_id"], "index": check, "action": "toggle" }),
    );
    assert_eq!(changed["error"]["code"], "app_changed", "{changed}");
    let stopped = helper.request("debug_stop", json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let refused = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(refused["error"]["code"], "interrupted", "{refused}");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let unknown = helper.request("element_action", json!({ "window_id": window_id, "tree_id": "t999-zzzzz", "index": 1, "action": "toggle" }));
    assert_eq!(unknown["error"]["code"], "no_ui_tree", "{unknown}");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "off"), "a refused toggle reached the check box");
}

/// Spec-uia story 54: the app hangs once the combo box is asked to open (Expand waits for it). The
/// answer is `input_failed` after 3 seconds, saying the app may still act; the stuck worker is
/// left behind and reports nothing when its call returns, and the next request gets a new worker.
#[test]
fn an_action_on_a_hung_app_is_input_failed_after_three_seconds_and_nothing_follows() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-content").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let combo = index_of(&tree, |element| element["role"] == "combo box");

    let hang = Duration::from_secs(6);
    let (hung, started) = hung_action(&mut helper, &window_id, &tree, combo, "expand", ARM_COMMAND_HANG, hang);
    assert_eq!(hung["error"]["code"], "input_failed", "{hung}");
    let message = hung["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("within 3 seconds") && message.contains("may still perform the action later"), "{message}");
    nothing_follows(&mut helper, &window_id, started, hang, "hang:command");

    // The late expand went through once the app answered; a collapse on a new worker closes it.
    let after = read_tree(&mut helper, &window_id);
    let combo = index_of(&after, |element| element["role"] == "combo box");
    let collapsed = act(&mut helper, &window_id, &after, combo, "collapse");
    assert_eq!(collapsed["ok"], true, "{collapsed}");
    assert_eq!(collapsed["result"]["state"], "collapsed", "{collapsed}");
}

/// The app takes the toggle (a Win32 check box's click is posted, so Toggle returns) and hangs
/// handling it, so reading the new state back does not answer: `input_failed` after 3 seconds
/// with its own text, which says the action was taken and is not to be repeated.
#[test]
fn a_read_back_on_a_hung_app_is_input_failed_saying_the_action_was_taken() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");

    let hang = Duration::from_secs(6);
    let (hung, started) = hung_action(&mut helper, &window_id, &tree, check, "toggle", ARM_COMMAND_HANG, hang);
    assert_eq!(hung["error"]["code"], "input_failed", "{hung}");
    let message = hung["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("took the action") && message.contains("within 3 seconds") && message.contains("Do not repeat"), "{message}");
    assert!(!message.contains("may still perform"), "{message}");
    nothing_follows(&mut helper, &window_id, started, hang, "hang:command");

    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "on"), "the toggle was not taken: {after}");
    let check = index_of(&after, |element| element["name"] == "Remember me");
    let toggled = act(&mut helper, &window_id, &after, check, "toggle");
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!((&toggled["result"]["previous_state"], &toggled["result"]["state"]), (&json!("on"), &json!("off")), "{toggled}");
}

/// Review minor 1: the app hangs before the action reaches it (reading the check box's state ahead
/// of the Toggle). The answer is `input_failed` after 3 seconds saying the action was not done, and
/// the worker left behind does not do it once the app answers: a retry cannot toggle twice.
#[test]
fn a_hang_before_the_action_is_input_failed_and_the_action_never_follows() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");

    let hang = Duration::from_secs(5);
    let (hung, started) = hung_action(&mut helper, &window_id, &tree, check, "toggle", ARM_QUERY_HANG, hang);
    assert_eq!(hung["error"]["code"], "input_failed", "{hung}");
    let message = hung["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("within 3 seconds") && message.contains("was not done"), "{message}");
    assert!(!message.contains("may still perform"), "{message}");
    nothing_follows(&mut helper, &window_id, started, hang, "hang:query");

    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "off"), "the abandoned worker toggled late: {after}");
}

/// Arms the fixture's hang `arm` (`ARM_COMMAND_HANG`, `ARM_QUERY_HANG`) for `hang` and does
/// `action`, which must answer after about 3 seconds; returns the answer and when it was sent.
fn hung_action(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, action: &str, arm: u32, hang: Duration) -> (Value, Instant) {
    let armed = unsafe { SendMessageW(hwnd_of(window_id), arm, Some(WPARAM(hang.as_millis() as usize)), Some(LPARAM(0))) };
    assert_eq!(armed.0, 1, "the fixture did not arm the hang");
    let started = Instant::now();
    let hung = act(helper, window_id, tree, index, action);
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(2900) && waited < Duration::from_millis(4500), "answered after {waited:?}: {hung}");
    (hung, started)
}

/// The helper's text for a stop that came before the action was taken.
const INTERRUPTED_MESSAGE: &str = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";

/// Ticket 70 (and the ticket 66 gap: a stop while an element action waits on its worker): the app
/// takes the toggle (its click is posted) and hangs handling it, so the read-back waits. Esc then
/// (`debug_stop`, acted on as it is read, like a physical Esc) is answered at once, while the
/// worker still waits, with the text for a stop after the action: it was taken and is not to be
/// repeated. The toggle stays done.
#[test]
fn a_stop_during_the_read_back_says_the_action_was_taken() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");

    let stopped = stop_at_hang(&mut helper, &window_id, &tree, check, "toggle", ARM_COMMAND_HANG, "hang:command");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    let message = stopped["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("after the action was taken") && message.contains("Do not repeat the action"), "{message}");
    assert!(message.contains("end this turn"), "{message}");

    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "on"), "the toggle was not taken: {after}");
}

/// Ticket 70: the app hangs reading the check box's state ahead of the Toggle, and Esc comes then:
/// the action was not taken, so the answer is the plain stop text, and once the app answers the
/// worker does not toggle.
#[test]
fn a_stop_before_the_action_is_the_plain_stop_and_nothing_is_done() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");

    let stopped = stop_at_hang(&mut helper, &window_id, &tree, check, "toggle", ARM_QUERY_HANG, "hang:query");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    assert_eq!(stopped["error"]["message"], INTERRUPTED_MESSAGE, "{stopped}");

    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "off"), "the stopped worker toggled: {after}");
}

/// Arms the fixture's hang `arm` for two seconds, sends `action`, and once the fixture shows it
/// reached the hang (`reached` in its title, read directly: the helper is busy) sends `debug_stop`.
/// The answer must come at once, well before the hang ends. Then waits for the stopped request to
/// end (the `debug_stop` answer follows it) and resumes; returns the answer.
fn stop_at_hang(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, action: &str, arm: u32, reached: &str) -> Value {
    let hwnd = hwnd_of(window_id);
    let hang = Duration::from_secs(2);
    let armed = unsafe { SendMessageW(hwnd, arm, Some(WPARAM(hang.as_millis() as usize)), Some(LPARAM(0))) };
    assert_eq!(armed.0, 1, "the fixture did not arm the hang");
    let acting = helper.send("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": index, "action": action }));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !window_title(hwnd).contains(reached) {
        assert!(Instant::now() < deadline, "the hang was not reached: {}", window_title(hwnd));
        std::thread::sleep(Duration::from_millis(20));
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

/// The fixture's title, read directly so it works while the helper is busy.
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

/// Once the app answers again, the worker left behind has written nothing, and the hang was
/// reached (the fixture recorded `reached`).
fn nothing_follows(helper: &mut Helper, window_id: &str, started: Instant, hang: Duration, reached: &str) {
    while started.elapsed() < hang + Duration::from_millis(700) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let late = helper.unsolicited();
    assert!(late.iter().all(|line| line.get("id").is_none()), "the left-behind worker answered: {late:?}");
    let shown = title(helper, window_id);
    assert!(shown.contains(reached), "the hang was not reached: {shown}");
}

/// The fixture's main edit: the first one, at the top.
fn main_edit(element: &Value) -> bool {
    element["role"] == "edit" && element["bounds"]["y"] == 10
}

fn act(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, action: &str) -> Value {
    helper.request("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": index, "action": action }))
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

    /// The lines the helper wrote since the last answer, without waiting for more.
    fn unsolicited(&mut self) -> Vec<Value> {
        self.incoming.try_iter().collect()
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
