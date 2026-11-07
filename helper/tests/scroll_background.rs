//! Reference-gaps G5 (ticket 09): `scroll` with `delivery_mode: "background"` on the fixture window.
//! The helper looks for the deepest element of the target's own UI tree that contains the point (as
//! a background click does), goes up from it to the first element whose ScrollPattern scrolls that
//! way. It first posts `amount` WM_MOUSEWHEEL (WM_MOUSEHWHEEL) notches to the window under the
//! point (`via: "wheel_message"`); when the position did not move, it calls the ScrollPattern's
//! SmallIncrement or SmallDecrement `amount` times (`via: "scroll_pattern"`). Neither is the real
//! wheel, moves the cursor or activates anything, so the foreground, the windows above the target
//! and the cursor end as they were (ADR 0002). The answer has the scroll position in percent before
//! and after, and `moved`: an element already at the end of its range is a success with `moved:
//! false`. A point with nothing that
//! scrolls that way is `background_unavailable` (`no_scroll_pattern`) and sends nothing.
//! `delivery_mode: "foreground"` (or none) is the real wheel, as before.
//!
//! The scrollable element is the fixture's Win32 list box (`--ui-content`): 20 rows "Row 1".., of
//! which about 5 show. The user's window and the cover are ordinary (activatable) fixture windows.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, POINT, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetDlgItem, GetForegroundWindow, GetWindow, IsWindowVisible, MoveWindow, SendMessageW, SetCursorPos, WindowFromPoint, CB_GETDROPPEDSTATE, CB_SHOWDROPDOWN, GA_ROOT, GW_HWNDPREV,
};

#[path = "physical_mouse.rs"]
mod physical_mouse;
#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The fixture (target), the user's window in front of it and another window between them, with
/// their processes kept alive.
struct Stage {
    window_id: String,
    user_id: String,
    cover_id: String,
    tree: Value,
    _processes: Vec<KillOnDrop>,
}

/// The target is behind a cover which is behind the user's window, which holds the foreground.
fn stage(helper: &mut Helper) -> Stage {
    stage_with(helper, &[])
}

/// `stage` with more fixture flags for the target.
fn stage_with(helper: &mut Helper, flags: &[&str]) -> Stage {
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).args(flags).spawn().expect("fixture"));
    let window_id = wait_for_window(helper, "CUMCP_FIXTURE");
    let tree = read_tree(helper, &window_id);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let left = info["result"]["x"].as_i64().expect("x");
    let width = info["result"]["width"].as_i64().expect("width");
    let user = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
    let user_id = wait_for_fixture_besides(helper, &[&window_id]);
    let cover = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover_id = wait_for_fixture_besides(helper, &[&window_id, &user_id]);
    place_window(&cover_id, (left + width - 320, 300, 320, 200));
    for id in [&window_id, &cover_id, &user_id] {
        let activated = helper.request("activate_window", json!({ "window_id": id }));
        assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
    }
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the user's window is not in front, so the test does not show a target out of front");
    assert_eq!(nearest_visible_above(hwnd(&window_id)), Some(hwnd(&cover_id)), "the cover is not just above the fixture, so the z-order check proves nothing");
    Stage { window_id, user_id, cover_id, tree, _processes: vec![fixture, user, cover] }
}

/// A background scroll at the list's second row moves the list: three small increments down take
/// it from the top, the rows scrolled in and out are those of the tree, and the answer says where
/// it went. The user's window keeps the foreground, the cover stays just above the target, the
/// cursor does not move, and no physical mouse input was seen; scrolling up goes back.
#[test]
fn a_background_scroll_moves_a_list_and_leaves_the_foreground_the_z_order_and_the_cursor() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let stage = stage(&mut helper);
    let target = hwnd(&stage.window_id);
    let user = hwnd(&stage.user_id);
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();
    let row = centre_of(&stage.tree, |element| element["role"] == "list item" && element["name"] == "Row 2");

    let down = scroll_at(&mut helper, &stage.window_id, row, "down", json!({}));
    assert_eq!(down["ok"], true, "{down}");
    let result = &down["result"];
    assert_eq!(result["action"], "scroll", "{down}");
    assert_eq!(result["via"], "wheel_message", "{down}");
    assert_eq!(result["steps"], 3, "{down}");
    assert_eq!(result["moved"], true, "{down}");
    assert_eq!(result["percent_before"].as_f64(), Some(0.0), "{down}");
    let after = result["percent_after"].as_f64().unwrap_or_else(|| panic!("no percent_after: {down}"));
    assert!(after > 0.0 && after < 100.0, "three notches of twenty rows are partway: {down}");
    assert!(result["waited_for_idle_ms"].is_u64(), "{down}");
    assert!(result.get("restore_failed").is_none() && result.get("foreground_changed").is_none(), "{down}");
    let shown = read_tree(&mut helper, &stage.window_id);
    assert!(!has_row(&shown, 1), "Row 1 is still shown after scrolling down: {shown}");
    // A notch scrolls as many lines as the system setting says (three by default), so how far the
    // list went is read from the tree, not counted.
    assert!(first_row(&shown) > 1 && first_row(&shown) < 20, "the list did not scroll partway: {shown}");

    assert_eq!(unsafe { GetForegroundWindow() }, user, "a background scroll changed the foreground window");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&stage.cover_id)), "the target was not left below the window that was above it");
    assert_eq!(cursor(), cursor_before, "a background scroll moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let title = title(&mut helper, &stage.window_id);
    assert!(!title.contains("key:") && !title.contains("char:") && !title.contains("click:"), "input reached the fixture: {title}");

    let up = scroll_at(&mut helper, &stage.window_id, row, "up", json!({}));
    assert_eq!(up["ok"], true, "{up}");
    assert_eq!(up["result"]["moved"], true, "{up}");
    assert_eq!(up["result"]["percent_before"].as_f64(), Some(after), "{up}");
    assert_eq!(up["result"]["percent_after"].as_f64(), Some(0.0), "{up}");
    assert!(has_row(&read_tree(&mut helper, &stage.window_id), 1), "Row 1 did not come back");
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground changed");
    assert_eq!(cursor(), cursor_before, "the cursor moved (physical mouse input meanwhile: {})", physical.seen());
}

/// Past the end of the list a scroll is a success that moved nothing: the position stays 100 (or 0
/// at the top) and `moved` is false. Fifty notches reach the end from the top. At the end the wheel
/// moves nothing, so the scroll falls back to the ScrollPattern and says so.
#[test]
fn a_scroll_at_the_end_of_the_list_succeeds_with_moved_false() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let stage = stage(&mut helper);
    let row = centre_of(&stage.tree, |element| element["role"] == "list item" && element["name"] == "Row 2");

    let to_end = scroll_at(&mut helper, &stage.window_id, row, "down", json!({ "amount": 50 }));
    assert_eq!(to_end["ok"], true, "{to_end}");
    assert_eq!(to_end["result"]["moved"], true, "{to_end}");
    assert_eq!(to_end["result"]["percent_after"].as_f64(), Some(100.0), "{to_end}");
    // The point is a row that is no longer under it: the one now at the same place is.
    let at_end = scroll_at(&mut helper, &stage.window_id, row, "down", json!({}));
    assert_eq!(at_end["ok"], true, "{at_end}");
    assert_eq!(at_end["result"]["moved"], false, "{at_end}");
    assert_eq!(at_end["result"]["via"], "scroll_pattern", "{at_end}");
    assert_eq!(at_end["result"]["percent_before"].as_f64(), Some(100.0), "{at_end}");
    assert_eq!(at_end["result"]["percent_after"].as_f64(), Some(100.0), "{at_end}");

    let to_top = scroll_at(&mut helper, &stage.window_id, row, "up", json!({ "amount": 50 }));
    assert_eq!(to_top["ok"], true, "{to_top}");
    assert_eq!(to_top["result"]["percent_after"].as_f64(), Some(0.0), "{to_top}");
    let at_top = scroll_at(&mut helper, &stage.window_id, row, "up", json!({}));
    assert_eq!(at_top["ok"], true, "{at_top}");
    assert_eq!(at_top["result"]["moved"], false, "{at_top}");
}

/// An app that ignores a posted wheel message (Edge) is scrolled through the ScrollPattern: the
/// list box of this fixture swallows the wheel, the position does not move, and the scroll falls
/// back to SmallIncrement calls, which do move it. The answer says which way worked.
#[test]
fn a_list_that_ignores_the_wheel_message_is_scrolled_through_the_scroll_pattern() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let stage = stage_with(&mut helper, &["--list-ignores-wheel"]);
    let cursor_before = cursor();
    let row = centre_of(&stage.tree, |element| element["role"] == "list item" && element["name"] == "Row 2");

    let down = scroll_at(&mut helper, &stage.window_id, row, "down", json!({}));
    assert_eq!(down["ok"], true, "{down}");
    assert_eq!(down["result"]["via"], "scroll_pattern", "{down}");
    assert_eq!(down["result"]["moved"], true, "{down}");
    assert_eq!(down["result"]["steps"], 3, "{down}");
    assert_eq!(down["result"]["percent_before"].as_f64(), Some(0.0), "{down}");
    assert!(down["result"]["percent_after"].as_f64().is_some_and(|after| after > 0.0), "{down}");
    assert!(!has_row(&read_tree(&mut helper, &stage.window_id), 1), "the list did not scroll");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&stage.user_id), "a background scroll changed the foreground window");
    assert_eq!(cursor(), cursor_before, "a background scroll moved the cursor");
}

/// A point on the "Click me" button (no element from it up to the window scrolls) is
/// `background_unavailable` with `no_scroll_pattern`, a point outside the window `input_failed` as
/// in the foreground, an unknown `delivery_mode` `invalid_params`. None sends input: the list stays
/// at the top, the fixture records nothing, and the foreground and the cursor are as before.
#[test]
fn a_point_with_nothing_that_scrolls_is_background_unavailable_and_sends_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let stage = stage(&mut helper);
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();
    let button = centre_of(&stage.tree, |element| element["role"] == "button" && element["name"] == "Click me");

    for direction in ["down", "up", "left", "right"] {
        let refused = scroll_at(&mut helper, &stage.window_id, button, direction, json!({}));
        assert_eq!(refused["error"]["code"], "background_unavailable", "{direction}: {refused}");
        assert_eq!(refused["error"]["data"]["reason"], "no_scroll_pattern", "{direction}: {refused}");
        assert_eq!(refused["error"]["data"]["recommended"], "foreground", "{direction}: {refused}");
        assert_eq!(refused["error"]["data"]["input_sent"], false, "{direction}: {refused}");
        assert!(refused["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{refused}");
    }
    let outside = scroll_at(&mut helper, &stage.window_id, (-50, -50), "down", json!({}));
    assert_eq!(outside["error"]["code"], "input_failed", "{outside}");
    assert!(outside["error"]["message"].as_str().unwrap_or("").contains("outside the target window"), "{outside}");
    let unknown = scroll_at(&mut helper, &stage.window_id, button, "down", json!({ "delivery_mode": "auto" }));
    assert_eq!(unknown["error"]["code"], "invalid_params", "{unknown}");

    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&stage.user_id), "a refused scroll changed the foreground window");
    assert_eq!(cursor(), cursor_before, "a refused scroll moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let after = read_tree(&mut helper, &stage.window_id);
    assert!(has_row(&after, 1) && !has_row(&after, 6), "a refused scroll moved the list: {after}");
    let shown = title(&mut helper, &stage.window_id);
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");
}

/// A fixture window the cover lies over completely (same rectangle, in front) is scrolled all the
/// same: the element is found in the target's own tree, not by what the screen shows at the point,
/// and the cover is still in front and still covers the point afterwards.
#[test]
fn a_background_scroll_at_a_point_another_window_covers_scrolls_the_target() {
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
    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id]);
    place_window(&cover, (left, top, width, height));
    let activated = helper.request("activate_window", json!({ "window_id": cover }));
    assert_eq!(activated["ok"], true, "could not foreground the cover: {activated}");

    let row = centre_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    let over = POINT { x: (left + row.0) as i32, y: (top + row.1) as i32 };
    unsafe { SetCursorPos(over.x, over.y) }.expect("SetCursorPos over the row");
    let physical = physical_mouse::PhysicalMouse::start();
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(over), GA_ROOT) }, hwnd(&cover), "the cover does not cover the row, so the test proves nothing");
    let above = windows_above(hwnd(&window_id));
    let cover_before = title(&mut helper, &cover);
    let cursor_before = cursor();

    let down = scroll_at(&mut helper, &window_id, row, "down", json!({}));
    assert_eq!(down["ok"], true, "{down}");
    assert_eq!(down["result"]["moved"], true, "{down}");
    assert!(!has_row(&read_tree(&mut helper, &window_id), 1), "the target did not scroll");
    assert_eq!(title(&mut helper, &cover), cover_before, "the covering window received something");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&cover), "a background scroll changed the foreground window");
    assert_eq!(windows_above(hwnd(&window_id)), above, "a background scroll changed the z-order above the fixture");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(over), GA_ROOT) }, hwnd(&cover), "the fixture was raised over the cover");
    assert_eq!(cursor(), cursor_before, "a background scroll moved the cursor (physical mouse input meanwhile: {})", physical.seen());
}

/// A point inside a popup of the target (an opened drop-down list that lies over the right end of the
/// list box) is the popup's to scroll: the wheel would go to the list box behind it, which is not
/// what the point shows, so the scroll is done through the popup's ScrollPattern alone and the list
/// box does not move.
#[test]
fn a_background_scroll_at_a_point_in_a_popup_scrolls_the_popup_and_not_the_window_behind_it() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let stage = stage_with(&mut helper, &["--ui-long-colours"]);
    let row = find(&stage.tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    let combo = find(&stage.tree, |element| element["role"] == "combo box");
    let number = |element: &Value, name: &str| element["bounds"][name].as_i64().unwrap_or_else(|| panic!("no {name} in {element}"));
    // Left end of the drop-down, which is also the right end of the row.
    let point = (number(combo, "x") + 5, number(row, "y") + number(row, "height") / 2);
    assert!(point.0 < number(row, "x") + number(row, "width"), "the point is not on the list row, so the test proves nothing: {row} {combo}");
    let combo_window = unsafe { GetDlgItem(Some(hwnd(&stage.window_id)), 1103) }.expect("the colour combo box");
    unsafe { SendMessageW(combo_window, CB_SHOWDROPDOWN, Some(WPARAM(1)), None) };
    std::thread::sleep(Duration::from_millis(500));
    assert!(unsafe { SendMessageW(combo_window, CB_GETDROPPEDSTATE, None, None) }.0 != 0, "the drop-down did not open");

    let down = scroll_at(&mut helper, &stage.window_id, point, "down", json!({}));
    unsafe { SendMessageW(combo_window, CB_SHOWDROPDOWN, Some(WPARAM(0)), None) };
    assert_eq!(down["ok"], true, "{down}");
    assert_eq!(down["result"]["via"], "scroll_pattern", "{down}");
    assert_eq!(down["result"]["moved"], true, "{down}");
    assert_eq!(down["result"]["percent_before"].as_f64(), Some(0.0), "{down}");
    assert!(has_row(&read_tree(&mut helper, &stage.window_id), 1), "the list box behind the popup was scrolled");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&stage.user_id), "a background scroll changed the foreground window");
}

/// `delivery_mode: "foreground"` (or none) is the wheel, as before: the target is brought forward
/// from behind the user's window, the cursor goes to the point, and the answer has none of the
/// background fields.
#[test]
fn a_foreground_scroll_still_uses_the_real_wheel() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");

    let row = centre_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    unsafe { SetCursorPos(0, 0) }.expect("SetCursorPos");
    let scrolled = helper.request("scroll", json!({ "window_id": window_id, "x": row.0, "y": row.1, "direction": "down", "amount": 3, "delivery_mode": "foreground" }));
    assert_eq!(scrolled["ok"], true, "{scrolled}");
    assert!(scrolled["result"].get("action").is_none() && scrolled["result"].get("percent_before").is_none() && scrolled["result"].get("waited_for_idle_ms").is_none(), "{scrolled}");
    let now = cursor();
    assert_ne!((now.x, now.y), (0, 0), "a foreground scroll did not move the cursor to the point");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(now), GA_ROOT) }, hwnd(&window_id), "the cursor is not over the target after a foreground scroll");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&window_id), "the foreground scroll left the target out of front");
    assert!(!has_row(&read_tree(&mut helper, &window_id), 1), "the wheel did not scroll the list");

    let without = helper.request("scroll", json!({ "window_id": window_id, "x": row.0, "y": row.1, "direction": "up", "amount": 3 }));
    assert_eq!(without["ok"], true, "{without}");
    assert!(without["result"].get("percent_before").is_none(), "no delivery_mode is the wheel: {without}");
}

/// A background scroll at window pixel `point` toward `direction`; `extra` adds or overrides params.
fn scroll_at(helper: &mut Helper, window_id: &str, point: (i64, i64), direction: &str, extra: Value) -> Value {
    let mut params = json!({ "window_id": window_id, "x": point.0, "y": point.1, "direction": direction, "amount": 3, "delivery_mode": "background" });
    for (key, value) in extra.as_object().expect("extra params") {
        params[key] = value.clone();
    }
    helper.request("scroll", params)
}

/// The centre of the first element `matches`, in window pixels (the tree's bounds).
fn centre_of(tree: &Value, matches: impl Fn(&Value) -> bool) -> (i64, i64) {
    let element = find(tree, matches);
    let bounds = &element["bounds"];
    let number = |name: &str| bounds[name].as_i64().unwrap_or_else(|| panic!("no {name} in {element}"));
    (number("x") + number("width") / 2, number("y") + number("height") / 2)
}

/// The number of the first list row the tree shows.
fn first_row(tree: &Value) -> usize {
    tree["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .filter_map(|element| element["name"].as_str()?.strip_prefix("Row ")?.parse::<usize>().ok())
        .min()
        .unwrap_or_else(|| panic!("no list row in {tree}"))
}

/// Whether the tree shows list row `row` ("Row <row>"; rows scrolled out of the list are left out).
fn has_row(tree: &Value, row: usize) -> bool {
    let name = format!("Row {row}");
    tree["elements"].as_array().expect("elements").iter().any(|element| element["name"] == name)
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
