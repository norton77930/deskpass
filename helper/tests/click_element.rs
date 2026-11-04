//! Ticket 62: `click` on an `element` of a UI tree, on the fixture window. The element's position
//! is read when it is clicked, and the click then takes the same path as a click at a point.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, PostMessageW, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WM_APP};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn a_click_by_element_index_changes_the_fixture_with_its_button_count_and_modifiers() {
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
    let button = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let check = index_of(&tree, |element| element["role"] == "check box" && element["name"] == "Remember me");

    for (button_name, count, expected) in [("left", 1, "click:left"), ("right", 1, "click:right"), ("left", 2, "click:double")] {
        let clicked = click_element(&mut helper, &window_id, &tree, button, json!({ "button": button_name, "click_count": count }));
        assert_eq!(clicked["ok"], true, "{button_name} x{count}: {clicked}");
        assert!(title(&mut helper, &window_id).contains(expected), "{expected} was not recorded");
    }
    let clicked = click_element(&mut helper, &window_id, &tree, check, json!({ "modifiers": ["shift"] }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let after = read_tree(&mut helper, &window_id);
    let checked = find(&after, |element| element["name"] == "Remember me");
    assert!(has(checked, "states", "on"), "the check box was not checked: {checked}");
}

/// The tree is read, then the window moves: the click goes to where the element is now.
#[test]
fn after_the_window_moves_a_click_by_element_index_still_hits() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    dpi_aware();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    activate(&mut helper, &window_id);
    let tree = read_tree(&mut helper, &window_id);
    let button = index_of(&tree, |element| element["name"] == "Click me");

    let hwnd = hwnd_of(&window_id);
    let before = window_rect(hwnd);
    let _restore = Restore { hwnd, rect: before };
    move_window(hwnd, before.left + 160, before.top + 90);
    assert_eq!(window_rect(hwnd).left, before.left + 160, "the fixture did not move");
    // The button's old place is now the fixture's background (left of the edit's old place).
    let clicked = click_element(&mut helper, &window_id, &tree, button, json!({}));
    assert_eq!(clicked["ok"], true, "{clicked}");
    assert!(title(&mut helper, &window_id).contains("click:left"), "the click missed the moved button");
}

/// A button that closes the section it is in: clicked once by its number it closes the section;
/// the same number again is `element_not_found`, and nothing is sent.
#[test]
fn an_element_of_a_closed_section_is_element_not_found() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-section").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    activate(&mut helper, &window_id);
    let tree = read_tree(&mut helper, &window_id);
    let close = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Close section");

    let clicked = click_element(&mut helper, &window_id, &tree, close, json!({}));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !read_tree(&mut helper, &window_id)["elements"].as_array().expect("elements").iter().all(|element| element["name"] != "Close section") {
        assert!(Instant::now() < deadline, "the section did not close");
        std::thread::sleep(Duration::from_millis(50));
    }
    let events = title(&mut helper, &window_id);
    assert!(events.contains("section:closed"), "{events}");

    let again = click_element(&mut helper, &window_id, &tree, close, json!({}));
    assert_eq!(again["ok"], false, "{again}");
    assert_eq!(again["error"]["code"], "element_not_found", "{again}");
    assert!(again["error"]["message"].as_str().unwrap_or("").contains("new tree"), "{again}");
    assert_eq!(title(&mut helper, &window_id), events, "something reached the fixture");
}

/// Another app's always-on-top window covers the whole button: the click on its centre is
/// refused as a click at that point is.
#[test]
fn a_covered_element_is_target_obscured() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    dpi_aware();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let button = find(&tree, |element| element["name"] == "Click me");
    let index = button["index"].as_u64().expect("index");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let bounds = &button["bounds"];
    let (x, y, width, height) = (bounds["x"].as_i64().unwrap(), bounds["y"].as_i64().unwrap(), bounds["width"].as_i64().unwrap(), bounds["height"].as_i64().unwrap());
    let cover_rect = format!("{},{},{},{}", left + x - 4, top + y - 4, width + 8, height + 8);
    let _cover = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", &cover_rect]).spawn().expect("cover"));
    let cover = wait_for_window(&mut helper, "CUMCP_COVER");
    activate(&mut helper, &window_id);

    let refused = click_element(&mut helper, &window_id, &tree, index, json!({}));
    assert_eq!(refused["error"]["code"], "target_obscured", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(cover), "{refused}");
    assert!(!title(&mut helper, &window_id).contains("click:"), "the refused click reached the fixture");
    assert_eq!(title(&mut helper, &cover), "CUMCP_COVER | ev=", "the refused click reached the cover");
}

/// Row 5 of the list shows only its top half: the click goes to that half and selects it. Once
/// the list is scrolled, Row 1 (read before) is out of view: `input_failed` suggesting
/// `scroll_into_view`, and nothing is sent.
#[test]
fn a_partly_visible_row_is_clicked_where_it_shows_and_a_row_scrolled_away_is_input_failed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    activate(&mut helper, &window_id);
    let tree = read_tree(&mut helper, &window_id);
    let row1 = index_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 1");
    let row5 = index_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 5");

    let clicked = click_element(&mut helper, &window_id, &tree, row5, json!({}));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let after = read_tree(&mut helper, &window_id);
    let selected = find(&after, |element| element["name"] == "Row 5");
    assert!(has(selected, "states", "selected"), "the click on the visible half did not select Row 5: {selected}");

    // Wheel notches over the list scroll Rows 1 to 6 or more out of it.
    let list = find(&tree, |element| element["name"] == "Row 2");
    let (x, y) = (list["bounds"]["x"].as_i64().unwrap() + 20, list["bounds"]["y"].as_i64().unwrap() + 5);
    let scrolled = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "down", "amount": 3 }));
    assert_eq!(scrolled["ok"], true, "{scrolled}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_tree(&mut helper, &window_id)["elements"].as_array().expect("elements").iter().any(|element| element["name"] == "Row 1") {
        assert!(Instant::now() < deadline, "the list did not scroll Row 1 away");
        std::thread::sleep(Duration::from_millis(50));
    }
    let before = title(&mut helper, &window_id);
    let hidden = click_element(&mut helper, &window_id, &tree, row1, json!({}));
    assert_eq!(hidden["error"]["code"], "input_failed", "{hidden}");
    assert!(hidden["error"]["message"].as_str().unwrap_or("").contains("scroll_into_view"), "{hidden}");
    assert_eq!(title(&mut helper, &window_id), before, "something reached the fixture");
    let still = read_tree(&mut helper, &window_id);
    assert!(!still["elements"].as_array().expect("elements").iter().any(|element| element["name"] == "Row 1" && has(element, "states", "selected")), "{still}");
}

/// A tree view item's rectangle is only its own row, so the items under it lie outside it: a
/// double click by number expands "Vegetables", and a click by number on its child "Carrot", read
/// from a new tree, selects it.
#[test]
fn a_nested_tree_view_item_is_clicked_after_its_parent_is_expanded() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    activate(&mut helper, &window_id);
    let tree = read_tree(&mut helper, &window_id);
    let vegetables = index_of(&tree, |element| element["role"] == "tree item" && element["name"] == "Vegetables");
    let expanded = click_element(&mut helper, &window_id, &tree, vegetables, json!({ "click_count": 2 }));
    assert_eq!(expanded["ok"], true, "{expanded}");
    let deadline = Instant::now() + Duration::from_secs(5);
    let tree = loop {
        let tree = read_tree(&mut helper, &window_id);
        if tree["elements"].as_array().expect("elements").iter().any(|element| element["name"] == "Carrot") {
            break tree;
        }
        assert!(Instant::now() < deadline, "Vegetables did not expand: {tree}");
        std::thread::sleep(Duration::from_millis(50));
    };
    let carrot = index_of(&tree, |element| element["role"] == "tree item" && element["name"] == "Carrot");
    let clicked = click_element(&mut helper, &window_id, &tree, carrot, json!({}));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let after = read_tree(&mut helper, &window_id);
    let selected = find(&after, |element| element["name"] == "Carrot");
    assert!(has(selected, "states", "selected"), "the click did not select Carrot: {selected}");
}

/// An item of the window's open menu (its popup, partly outside the window) is clicked by number.
#[test]
fn an_item_of_an_open_menu_is_clicked_by_element_index() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--popup-menu"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    activate(&mut helper, &window_id);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    // A right-click on the background below the Dialog button opens the menu.
    let opened = helper.request("click", json!({ "window_id": window_id, "x": width * 300 / 480, "y": height * 340 / 360, "button": "right" }));
    assert_eq!(opened["ok"], true, "{opened}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !title(&mut helper, &window_id).contains("menulast:") {
        assert!(Instant::now() < deadline, "the menu did not open");
    }
    let tree = read_tree(&mut helper, &window_id);
    let item = index_of(&tree, |element| element["role"] == "menu item" && element["name"] == "Menu item 2");
    let clicked = click_element(&mut helper, &window_id, &tree, item, json!({}));
    if clicked["ok"] != true {
        let _ = helper.request("press_key", json!({ "window_id": window_id, "keys": ["escape"] }));
    }
    assert_eq!(clicked["ok"], true, "{clicked}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !title(&mut helper, &window_id).contains("menu:cmd:102") {
        assert!(Instant::now() < deadline, "Menu item 2 was not chosen");
    }
}

/// The fixture is moved so its top 60 rows are above the topmost monitor: its edit is wholly off
/// every screen (`offscreen`), its button only in part, and a click on the button goes to the
/// part on the screen.
#[test]
fn an_element_off_every_screen_is_offscreen_and_one_partly_on_screen_is_clicked_there() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    dpi_aware();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = hwnd_of(&window_id);
    let before = window_rect(hwnd);
    let _restore = Restore { hwnd, rect: before };
    let tree = read_tree(&mut helper, &window_id);
    let edit = find(&tree, |element| element["role"] == "edit" && !has(element, "states", "password"));
    let button = find(&tree, |element| element["name"] == "Click me");
    let edit_bottom = edit["bounds"]["y"].as_i64().unwrap() + edit["bounds"]["height"].as_i64().unwrap();
    let button_top = button["bounds"]["y"].as_i64().unwrap();
    let button_bottom = button_top + button["bounds"]["height"].as_i64().unwrap();
    // The window's top is moved this far above the monitor's top: below the edit, inside the button.
    let above = ((button_top + button_bottom) / 2 + edit_bottom) / 2;
    assert!(above > edit_bottom && above < button_bottom, "{edit} {button}");

    let monitor = monitors().into_iter().min_by_key(|rect| (rect.top, rect.left)).expect("a monitor");
    let left = monitor.left + 40;
    move_window(hwnd, left, monitor.top - above as i32);
    activate(&mut helper, &window_id);

    let off = click_element(&mut helper, &window_id, &tree, edit["index"].as_u64().unwrap(), json!({}));
    assert_eq!(off["error"]["code"], "offscreen", "{off}");
    let clicked = click_element(&mut helper, &window_id, &tree, button["index"].as_u64().unwrap(), json!({}));
    assert_eq!(clicked["ok"], true, "{clicked}");
    assert!(title(&mut helper, &window_id).contains("click:left"), "the click missed the part of the button on screen");
}

/// The stop refuses an element click before anything is read, a window that is no longer the
/// approved app's is `app_changed`, and a tree the helper does not have is `no_ui_tree` (not
/// `internal`).
#[test]
fn a_stopped_helper_refuses_an_element_click_and_an_unknown_tree_is_no_ui_tree() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let tree = read_tree(&mut helper, &window_id);
    let button = index_of(&tree, |element| element["name"] == "Click me");
    let stopped = helper.request("debug_stop", json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let refused = click_element(&mut helper, &window_id, &tree, button, json!({}));
    assert_eq!(refused["error"]["code"], "interrupted", "{refused}");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");

    let changed = helper.request("click", json!({ "window_id": window_id, "expected_app": "C:\\Apps\\other.exe", "element": { "tree_id": tree["tree_id"], "index": button } }));
    assert_eq!(changed["error"]["code"], "app_changed", "{changed}");

    let unknown = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": "t999-zzzzz", "index": 1 } }));
    assert_eq!(unknown["error"]["code"], "no_ui_tree", "{unknown}");
    let past = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": tree["tree_id"], "index": 999 } }));
    assert_eq!(past["error"]["code"], "invalid_params", "{past}");
    let both = helper.request("click", json!({ "window_id": window_id, "x": 5, "y": 5, "element": { "tree_id": tree["tree_id"], "index": button } }));
    assert_eq!(both["error"]["code"], "invalid_params", "{both}");
    assert!(!title(&mut helper, &window_id).contains("click:"), "a refused click reached the fixture");
}

/// The fixture's `HANG_UI`: its UI thread stops for `wparam` milliseconds.
const HANG_UI: u32 = WM_APP + 2;

/// Ticket 64 (m5, spec-uia story 54): the fixture's UI thread stops for 6 seconds. A click by
/// element waits 3 seconds for UI Automation to say where the element is, then fails with
/// `input_failed`, nothing sent. The stuck worker is left behind: when its call returns it says
/// nothing and clicks nothing, and the next request gets a new worker.
#[test]
fn a_click_by_element_on_a_hung_app_is_input_failed_after_three_seconds_and_nothing_follows() {
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

    let hang = Duration::from_secs(6);
    unsafe { PostMessageW(Some(hwnd_of(&window_id)), HANG_UI, WPARAM(hang.as_millis() as usize), LPARAM(0)) }.expect("post the hang");
    let hang_started = Instant::now();
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let hung = click_element(&mut helper, &window_id, &tree, button, json!({}));
    let waited = started.elapsed();
    assert_eq!(hung["error"]["code"], "input_failed", "{hung}");
    let message = hung["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("within 3 seconds") && message.contains("no input was sent"), "{message}");
    assert!(waited >= Duration::from_millis(2900) && waited < Duration::from_millis(4500), "answered after {waited:?}");

    // The hang ends, and the left-behind call with it.
    while hang_started.elapsed() < hang + Duration::from_millis(700) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let late = helper.unsolicited();
    assert!(late.iter().all(|line| line.get("id").is_none()), "the left-behind worker answered: {late:?}");
    assert!(!title(&mut helper, &window_id).contains("click:"), "the timed-out click reached the fixture");
    let after = read_tree(&mut helper, &window_id);
    let button = index_of(&after, |element| element["name"] == "Click me");
    let clicked = click_element(&mut helper, &window_id, &after, button, json!({}));
    assert_eq!(clicked["ok"], true, "{clicked}");
    assert!(title(&mut helper, &window_id).contains("click:left"), "the click after the hang missed");
}

fn click_element(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, extra: Value) -> Value {
    let mut params = json!({ "window_id": window_id, "element": { "tree_id": tree["tree_id"], "index": index } });
    for (key, value) in extra.as_object().expect("extra") {
        params[key] = value.clone();
    }
    helper.request("click", params)
}

fn activate(helper: &mut Helper, window_id: &str) {
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
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

fn dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn hwnd_of(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void)
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.expect("window rect");
    rect
}

fn move_window(hwnd: HWND, left: i32, top: i32) {
    unsafe { SetWindowPos(hwnd, None, left, top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) }.expect("move the fixture");
}

/// Puts the fixture back where it was, should the test fail with it moved.
struct Restore {
    hwnd: HWND,
    rect: RECT,
}

impl Drop for Restore {
    fn drop(&mut self) {
        move_window(self.hwnd, self.rect.left, self.rect.top);
    }
}

fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let monitors = &mut *(data.0 as *mut Vec<RECT>);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            monitors.push(info.rcMonitor);
        }
        true.into()
    }
    let mut monitors: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut monitors as *mut Vec<RECT> as isize));
    }
    monitors
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    wait_for_window(helper, "CUMCP_FIXTURE")
}

fn wait_for_window(helper: &mut Helper, title_prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        let found = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with(title_prefix)).map(|_| window["window_id"].as_str().unwrap_or("").to_string())
            })
        });
        if let Some(found) = found {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{title_prefix} was not listed");
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
            .stderr(Stdio::inherit())
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
