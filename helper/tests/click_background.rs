//! Ticket 140: `click` on an `element` with `delivery_mode: "background"` on the fixture window.
//! A left single click without modifiers is done through the element's first click pattern
//! (Invoke, Toggle, SelectionItem.Select, ExpandCollapse), while another window holds the
//! foreground. The fixture's Win32 controls are brought forward by UI Automation's Win32 proxy,
//! so the helper restores the foreground (ADR 0002): the foreground window, the visible windows
//! above the target and the cursor end as they were, also when another window covers the target,
//! and keys the user types meanwhile land in the user's window. A click that needs the real
//! pointer is `needs_pointer`, an element with no click pattern `no_click_pattern`; neither sends
//! any input. `delivery_mode: "foreground"` is the real mouse, as before.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow, IsIconic, MoveWindow, IsWindowVisible, SetCursorPos, ShowWindow, WindowFromPoint, GA_ROOT, GW_HWNDPREV, SW_MINIMIZE,
    SW_RESTORE,
};

#[path = "physical_mouse.rs"]
mod physical_mouse;
#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The user's window, a fixture of another process with an edit box, holds the foreground; another
/// process's ordinary window sits between it and the target in the z-order, covering part of the
/// target. While the test types into the user's window with SendInput every 15 ms, background
/// clicks press the "Click me" button and tick the check box (a Win32 check box has Invoke as well as
/// Toggle, and Invoke comes first): each answers the pattern it used, the foreground, the nearest
/// visible window above the target and the cursor are as before, the button and the check box took
/// effect, and the typed keys land in the user's edit: all of them, but for at most one per restore
/// whose plain call was refused (ticket 150: the key typed while the target held the foreground goes
/// to the target, and it is what makes the call refused; only ticket 155 avoids it).
#[test]
fn background_clicks_while_the_user_types_use_the_pattern_and_leave_the_user_window_in_front() {
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
    let left = info["result"]["x"].as_i64().expect("x");
    let width = info["result"]["width"].as_i64().expect("width");

    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
    let user_id = wait_for_new_fixture(&mut helper, &window_id);
    let cover_rect = (left + width - 320, 300, 320, 200);
    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id, &user_id]);
    place_window(&cover, cover_rect);
    for id in [&window_id, &cover, &user_id] {
        let activated = helper.request("activate_window", json!({ "window_id": id }));
        assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
    }

    let target = hwnd(&window_id);
    let user = hwnd(&user_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window is not in front, so the test does not show a target out of front");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the cover is not just above the fixture, so the z-order check proves nothing");
    let user_len_before = edit_length(&title(&mut helper, &user_id));
    let target_len_before = edit_length(&title(&mut helper, &window_id));
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();

    let button = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let refused_before = refused_restores(&mut helper);
    let typist = Typist::start();
    std::thread::sleep(Duration::from_millis(60));
    let pressed = click(&mut helper, &window_id, &tree, button, json!({}));
    let ticked = click(&mut helper, &window_id, &tree, check, json!({}));
    std::thread::sleep(Duration::from_millis(60));
    let typed = typist.stop();
    let refused = refused_restores(&mut helper) - refused_before;

    assert_eq!(pressed["ok"], true, "{pressed}");
    assert_eq!(pressed["result"]["action"], "invoke", "{pressed}");
    assert_eq!(ticked["ok"], true, "{ticked}");
    // The Win32 check box has Invoke as well as Toggle, and Invoke comes first; it reads back no state.
    assert_eq!(ticked["result"]["action"], "invoke", "{ticked}");
    assert_eq!(ticked["result"]["foreground_restored"], true, "{ticked}");
    assert!(ticked["result"].get("foreground_changed").is_none(), "{ticked}");
    for answer in [&pressed, &ticked] {
        assert!(answer["result"].get("restore_failed").is_none(), "{answer}");
    }
    assert!(typed >= 20, "only {typed} keys were typed, so the typing did not span the clicks");

    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground did not end with the user's window");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the target was not put back below the window that was above it");
    assert_eq!(cursor(), cursor_before, "a background click moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let shown = assert_typed_keys_kept(&mut helper, (&user_id, user_len_before), (&window_id, target_len_before), typed, refused);
    assert!(shown.contains("click:left"), "the button was not pressed: {shown}");
    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "on"), "the check box did not change");
}

/// Ticket 150: the same background clicks while the user types, behind a covering window, with the
/// first `SetForegroundWindow` of every restore refused (`debug_refuse_first_restore_call`, test
/// builds only). The attached fallback such a refusal took made the user's window's thread share
/// the busy target's input queue, and the keys typed meanwhile went to the target's control or
/// arrived as system keys (tens of keys in every run): the typed keys must land in the user's edit,
/// all but at most one per refused restore (the key typed while the target held the foreground), no
/// restore may fail, the user's window must end in front, and the target must be back below the
/// cover.
#[test]
fn background_clicks_whose_restore_is_first_refused_lose_no_typed_keys() {
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
    let left = info["result"]["x"].as_i64().expect("x");
    let width = info["result"]["width"].as_i64().expect("width");
    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
    let user_id = wait_for_new_fixture(&mut helper, &window_id);
    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id, &user_id]);
    place_window(&cover, (left + width - 320, 300, 320, 200));
    for id in [&window_id, &cover, &user_id] {
        let activated = helper.request("activate_window", json!({ "window_id": id }));
        assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
    }
    let target = hwnd(&window_id);
    let user = hwnd(&user_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window is not in front, so the test does not show a target out of front");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the cover is not just above the fixture, so the z-order check proves nothing");
    let refusing = helper.request("debug_refuse_first_restore_call", json!({}));
    assert_eq!(refusing["ok"], true, "{refusing}");
    let refused_before = refusing["result"]["refused_restores"].as_u64().expect("refused_restores") as usize;
    let user_len_before = edit_length(&title(&mut helper, &user_id));
    let target_len_before = edit_length(&title(&mut helper, &window_id));

    let button = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let typist = Typist::start();
    std::thread::sleep(Duration::from_millis(60));
    let pressed = click(&mut helper, &window_id, &tree, button, json!({}));
    let ticked = click(&mut helper, &window_id, &tree, check, json!({}));
    std::thread::sleep(Duration::from_millis(60));
    let typed = typist.stop();

    assert!(typed >= 20, "only {typed} keys were typed, so the typing did not span the clicks");
    let refused = refused_restores(&mut helper) - refused_before;
    assert!(refused > 0, "no restore had its first call refused, so the test shows nothing about it");
    assert_typed_keys_kept(&mut helper, (&user_id, user_len_before), (&window_id, target_len_before), typed, refused);
    for answer in [&pressed, &ticked] {
        assert_eq!(answer["ok"], true, "{answer}");
        assert!(answer["result"].get("restore_failed").is_none(), "{answer}");
    }
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground did not end with the user's window");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the target was not put back below the window that was above it");
}

/// Another process's ordinary window covers the whole fixture and holds the foreground. A
/// background click on the drop-down list expands it and a second click collapses it again (the
/// state decides); a click on the "Fruits" tree item, which has both SelectionItem and
/// ExpandCollapse, selects it and leaves it expanded (the first supported pattern wins). The
/// foreground window, the visible windows above the fixture and the cursor are as before, the
/// fixture is still covered, and no keyboard or pointer input reached it.
#[test]
fn a_background_click_on_a_covered_window_expands_by_state_and_prefers_select_to_expand() {
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

    let cover_rect = (left, top, width, height);
    let _cover_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("cover"));
    let cover = wait_for_fixture_besides(&mut helper, &[&window_id]);
    place_window(&cover, cover_rect);
    let activated = helper.request("activate_window", json!({ "window_id": cover }));
    assert_eq!(activated["ok"], true, "could not foreground the cover: {activated}");

    let target = hwnd(&window_id);
    let middle = POINT { x: (left + width / 2) as i32, y: (top + height / 2) as i32 };
    unsafe { SetCursorPos(middle.x, middle.y) }.expect("SetCursorPos over the fixture");
    let physical = physical_mouse::PhysicalMouse::start();
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(middle), GA_ROOT) }, hwnd(&cover), "the cover does not cover the fixture, so the test proves nothing");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, hwnd(&cover), "the cover is not in front, so the test does not show a target out of front");
    let above = windows_above(target);
    assert!(above.contains(&hwnd(&cover)), "the cover is not above the fixture in the z-order");
    let cursor_before = cursor();

    let combo = index_of(&tree, |element| element["role"] == "combo box");
    let opened = click(&mut helper, &window_id, &tree, combo, json!({}));
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(opened["result"]["action"], "expand", "{opened}");
    assert_eq!(opened["result"]["state"], "expanded", "{opened}");
    let closed = click(&mut helper, &window_id, &tree, combo, json!({}));
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(closed["result"]["action"], "collapse", "{closed}");
    assert_eq!(closed["result"]["state"], "collapsed", "{closed}");

    let fruits = index_of(&tree, |element| element["role"] == "tree item" && element["name"] == "Fruits");
    let chosen = click(&mut helper, &window_id, &tree, fruits, json!({}));
    assert_eq!(chosen["ok"], true, "{chosen}");
    assert_eq!(chosen["result"]["action"], "select", "{chosen}");
    assert_eq!(chosen["result"]["state"], "selected", "{chosen}");
    let after = read_tree(&mut helper, &window_id);
    let fruits_now = find(&after, |element| element["role"] == "tree item" && element["name"] == "Fruits");
    assert!(has(fruits_now, "states", "expanded"), "the click collapsed the tree item instead of selecting it: {fruits_now}");

    // A Win32 list item reports Invoke as well as SelectionItem, but its provider answers Invoke
    // "Pattern not found" now and then; the click then goes on to Select. Either way it ends selected.
    let row = index_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    let listed = click(&mut helper, &window_id, &tree, row, json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    let used = listed["result"]["action"].as_str().unwrap_or("");
    assert!(used == "invoke" || (used == "select" && listed["result"]["state"] == "selected"), "{listed}");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Row 2"), "states", "selected"), "the row is not selected");

    for answer in [&opened, &closed, &chosen, &listed] {
        assert!(answer["result"].get("restore_failed").is_none(), "{answer}");
        assert!(answer["result"].get("foreground_changed").is_none(), "{answer}");
    }
    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "a background click changed the foreground window");
    assert_eq!(windows_above(target), above, "a background click changed the z-order above the fixture (the cover is {:?}, the foreground {:?})", hwnd(&cover), unsafe { GetForegroundWindow() });
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(middle), GA_ROOT) }, hwnd(&cover), "the fixture was raised over the cover");
    assert_eq!(cursor(), cursor_before, "a background click moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");
}

/// Clicks that need the real pointer (right button, double click, held modifiers) are
/// `background_unavailable` with reason `needs_pointer`; an edit box, which has no click pattern,
/// with `no_click_pattern`. Neither sends input: the check box stays off, the fixture records
/// nothing, and the foreground, the focus and the cursor are as before.
#[test]
fn clicks_that_need_the_pointer_or_have_no_pattern_are_background_unavailable_and_send_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("user window"));
    let user_id = wait_for_new_fixture(&mut helper, &window_id);
    let activated = helper.request("activate_window", json!({ "window_id": user_id }));
    assert_eq!(activated["ok"], true, "could not foreground the user's window: {activated}");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, hwnd(&user_id), "the user's window is not in front");
    let cursor_before = cursor();
    let physical = physical_mouse::PhysicalMouse::start();
    let focused_before = read_tree(&mut helper, &window_id)["focused_index"].clone();

    let button = index_of(&tree, |element| element["role"] == "button" && element["name"] == "Click me");
    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let edit = index_of(&tree, |element| element["role"] == "edit" && element["bounds"]["y"] == 10);
    for (index, extra) in [
        (button, json!({ "button": "right" })),
        (button, json!({ "button": "middle" })),
        (button, json!({ "click_count": 2 })),
        (check, json!({ "click_count": 3 })),
        (check, json!({ "modifiers": ["shift"] })),
        (check, json!({ "button": "right", "click_count": 2, "modifiers": ["ctrl"] })),
    ] {
        let refused = click(&mut helper, &window_id, &tree, index, extra.clone());
        assert_eq!(refused["error"]["code"], "background_unavailable", "{extra}: {refused}");
        assert_eq!(refused["error"]["data"]["reason"], "needs_pointer", "{extra}: {refused}");
        assert_eq!(refused["error"]["data"]["recommended"], "foreground", "{extra}: {refused}");
        assert_eq!(refused["error"]["data"]["input_sent"], false, "{extra}: {refused}");
        assert!(refused["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{refused}");
    }
    let none = click(&mut helper, &window_id, &tree, edit, json!({}));
    assert_eq!(none["error"]["code"], "background_unavailable", "{none}");
    assert_eq!(none["error"]["data"]["reason"], "no_click_pattern", "{none}");
    assert_eq!(none["error"]["data"]["recommended"], "foreground", "{none}");
    assert_eq!(none["error"]["data"]["input_sent"], false, "{none}");

    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "a refused click changed the foreground window");
    assert_eq!(cursor(), cursor_before, "a refused click moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let after = read_tree(&mut helper, &window_id);
    assert_eq!(after["focused_index"], focused_before, "a refused click moved the focus");
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "off"), "a refused click changed the check box");
    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");

    let unknown = click(&mut helper, &window_id, &tree, button, json!({ "delivery_mode": "auto" }));
    assert_eq!(unknown["error"]["code"], "invalid_params", "{unknown}");
}

/// A minimized target is `background_unavailable` (`minimized`) and stays minimized.
#[test]
fn a_background_click_on_a_minimized_window_is_background_unavailable() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let target = hwnd(&window_id);
    let _ = unsafe { ShowWindow(target, SW_MINIMIZE) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !unsafe { IsIconic(target) }.as_bool() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(unsafe { IsIconic(target) }.as_bool(), "the fixture did not minimize");
    std::thread::sleep(Duration::from_millis(200));

    let refused = click(&mut helper, &window_id, &tree, check, json!({}));
    assert_eq!(refused["error"]["code"], "background_unavailable", "{refused}");
    assert_eq!(refused["error"]["data"]["reason"], "minimized", "{refused}");
    assert_eq!(refused["error"]["data"]["input_sent"], false, "{refused}");
    assert!(unsafe { IsIconic(target) }.as_bool(), "the refused click restored the fixture");
    let _ = unsafe { ShowWindow(target, SW_RESTORE) };
    std::thread::sleep(Duration::from_millis(300));
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "off"), "the refused click was done");
}

/// `delivery_mode: "foreground"` is the click with the real mouse: the target is brought forward
/// from behind the user's window, the cursor goes to the element, the check box is ticked, and the
/// answer has none of the background fields.
#[test]
fn a_foreground_click_by_element_still_uses_the_real_mouse() {
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

    let check = index_of(&tree, |element| element["name"] == "Remember me");
    unsafe { SetCursorPos(0, 0) }.expect("SetCursorPos");
    let clicked = click(&mut helper, &window_id, &tree, check, json!({ "delivery_mode": "foreground" }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    assert!(clicked["result"].get("action").is_none() && clicked["result"].get("waited_for_idle_ms").is_none(), "{clicked}");
    let now = cursor();
    assert_ne!((now.x, now.y), (0, 0), "a foreground click did not move the cursor to the element");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(now), GA_ROOT) }, hwnd(&window_id), "the cursor is not over the target after a foreground click");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&window_id), "the foreground click left the target out of front");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "on"), "the foreground click did not tick the check box");
}

fn click(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, extra: Value) -> Value {
    let mut params = json!({ "window_id": window_id, "element": { "tree_id": tree["tree_id"], "index": index }, "delivery_mode": "background" });
    for (key, value) in extra.as_object().expect("extra params") {
        params[key] = value.clone();
    }
    helper.request("click", params)
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

/// The length of the fixture's edit text, from its title (`| len=<n> |`).
fn edit_length(title: &str) -> usize {
    title.split(" | ").find_map(|part| part.strip_prefix("len=")).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no len= in {title}"))
}

/// How many restores had their first call refused so far, in this test build of the helper.
fn refused_restores(helper: &mut Helper) -> usize {
    let answer = helper.request("debug_refused_restores", json!({}));
    answer["result"]["refused_restores"].as_u64().unwrap_or_else(|| panic!("{answer}")) as usize
}

/// Ticket 150: of `typed` keys, the user's edit (`user`: window id, length before) got them all but
/// at most one per refused restore (`refused`), the key typed while the target held the foreground,
/// and the target's edit (`target`, likewise) got no more than that. Answers the target's title.
fn assert_typed_keys_kept(helper: &mut Helper, user: (&str, usize), target: (&str, usize), typed: usize, refused: usize) -> String {
    let (user_id, user_len_before) = user;
    let (target_id, target_len_before) = target;
    // Ticket 156: the user's window is a debug fixture that, while background clicks run, can take
    // longer per key than the Typist's pace, so keys typed near the end may still sit in its queue
    // when typing stops (none is lost); its thread can also stall for seconds before taking the
    // first of them. Waits until every key landed, or, once the length has grown at least once,
    // until it has not grown for `TYPED_SETTLE` (a refused restore can keep a key out); at most
    // `TYPED_WAIT_LIMIT`. `title` paces the reads (it waits before each request).
    const TYPED_SETTLE: Duration = Duration::from_secs(1);
    const TYPED_WAIT_LIMIT: Duration = Duration::from_secs(10);
    let deadline = Instant::now() + TYPED_WAIT_LIMIT;
    let mut user_title = title(helper, user_id);
    let mut length = edit_length(&user_title);
    let mut last_growth = (length > user_len_before).then(Instant::now);
    while length < user_len_before + typed && Instant::now() < deadline && last_growth.map_or(true, |at| at.elapsed() < TYPED_SETTLE) {
        user_title = title(helper, user_id);
        let now = edit_length(&user_title);
        if now > length {
            length = now;
            last_growth = Some(Instant::now());
        }
    }
    let shown = title(helper, target_id);
    let landed = edit_length(&user_title) - user_len_before;
    let in_target = edit_length(&shown).saturating_sub(target_len_before);
    assert!(
        typed - landed.min(typed) <= refused,
        "{} of {typed} typed keys did not land in the user's window, more than the {refused} restores refused first (the target got {in_target}): {user_title} / target: {shown}",
        typed - landed.min(typed)
    );
    assert!(in_target <= refused, "{in_target} typed keys landed in the target, more than the {refused} restores refused first: {shown}");
    if refused == 0 {
        assert!(!shown.contains("key:") && !shown.contains("char:"), "keys reached the fixture: {shown}");
    }
    shown
}

/// Types `k` with SendInput every 15 ms on a thread of its own, into whatever window holds the
/// foreground, as a user typing during the action; `stop` answers how many were typed.
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

fn wait_for_new_fixture(helper: &mut Helper, seen: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                let title = window["title"].as_str().unwrap_or("");
                let id = window["window_id"].as_str().unwrap_or("").to_string();
                (title.starts_with("CUMCP_FIXTURE") && id != seen && !id.is_empty()).then_some(id)
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("the second fixture was not listed");
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
