//! Ticket 138: `element_action` with `delivery_mode: "background"` on the fixture window. The
//! fixture's Win32 controls are brought forward by UI Automation's Win32 proxy on every action, so
//! the helper restores the foreground (ADR 0002): the foreground window, the visible windows above
//! the target and the cursor end as they were, also when another window covers the target, and
//! keys the user types meanwhile land in the user's window. A minimized target and the focus
//! action are `background_unavailable`; an open dialog is `dialog_open`, as in the foreground.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindow, IsIconic, MoveWindow, IsWindowVisible, SendMessageW, SetCursorPos, ShowWindow, WindowFromPoint, GA_ROOT, GW_HWNDPREV,
    SW_MINIMIZE, SW_RESTORE,
};

#[path = "physical_mouse.rs"]
mod physical_mouse;
#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The user's window, a fixture of another process with an edit box, holds the foreground; another
/// process's ordinary window sits between it and the target in the z-order, covering part of the
/// target. While the test types into the user's window with SendInput every 15 ms, a background
/// toggle changes the check box: the fixture is fronted by the Win32 proxy and the helper gives the
/// foreground back at once, then puts the target back below the window that was above it. Every
/// typed key lands in the user's edit, the foreground, the nearest visible window above the target
/// and the cursor are as before, and the result says `foreground_restored: true`.
#[test]
fn a_background_action_while_the_user_types_restores_the_foreground_and_every_key_lands_in_the_user_window() {
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

    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let typist = Typist::start();
    std::thread::sleep(Duration::from_millis(60));
    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    std::thread::sleep(Duration::from_millis(60));
    let typed = typist.stop();

    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert_eq!(toggled["result"]["foreground_restored"], true, "{toggled}");
    assert!(toggled["result"].get("foreground_changed").is_none(), "{toggled}");
    assert!(toggled["result"].get("restore_failed").is_none(), "{toggled}");
    assert!(typed >= 20, "only {typed} keys were typed, so the typing did not span the action");

    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground did not end with the user's window");
    assert_eq!(nearest_visible_above(target), Some(hwnd(&cover)), "the target was not put back below the window that was above it");
    assert_eq!(cursor(), cursor_before, "a background action moved the cursor (physical mouse input meanwhile: {})", physical.seen());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut user_title = title(&mut helper, &user_id);
    while edit_length(&user_title) < user_len_before + typed && Instant::now() < deadline {
        user_title = title(&mut helper, &user_id);
    }
    assert_eq!(edit_length(&user_title), user_len_before + typed, "not every typed key landed in the user's window: {user_title}");
    assert_eq!(edit_length(&title(&mut helper, &window_id)), target_len_before, "typed keys landed in the target");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "on"), "the toggle did not take effect");
}

/// A background action the target fronts itself for, with the helper's debug hook making the next
/// restore fail: the action is still reported done, with `foreground_changed` and
/// `restore_failed: true`, and the element changed.
#[test]
fn a_failed_restore_still_reports_the_action_done_with_foreground_changed_and_restore_failed() {
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
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the user's window is not in front");

    let armed = helper.request("debug_fail_next_restore", json!({}));
    assert_eq!(armed["ok"], true, "{armed}");
    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(toggled["ok"], true, "the action was not reported done: {toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert_eq!(toggled["result"]["restore_failed"], true, "{toggled}");
    assert!(toggled["result"]["foreground_changed"].is_object(), "{toggled}");
    assert!(toggled["result"].get("foreground_restored").is_none(), "{toggled}");
    // The fixture, or the check box child the Win32 proxy fronted, holds the foreground now.
    assert_ne!(toggled["result"]["foreground_changed"]["foreground"]["window_id"], json!(user_id), "{toggled}");
    assert_ne!(unsafe { GetForegroundWindow() }, hwnd(&user_id), "the restore was not made to fail");
}

/// The user's own foreground window IS the target (the user launched the app and watches the AI):
/// the Win32 proxy fronts the check box child inside it, but nothing is taken from the user, so the
/// background toggle reports neither `restore_failed` nor `foreground_changed`.
#[test]
fn a_background_action_on_the_users_own_foreground_window_reports_no_foreground_trouble() {
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
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let target = hwnd(&window_id);
    assert_eq!(unsafe { GetForegroundWindow() }, target, "the fixture is not in front, so the test does not show the user's window as the target");

    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert!(toggled["result"].get("restore_failed").is_none(), "nothing was taken from the user, yet: {toggled}");
    assert!(toggled["result"].get("foreground_changed").is_none(), "the user's window kept the foreground, yet: {toggled}");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "on"), "the toggle did not take effect");

    let row = index_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    let selected = act(&mut helper, &window_id, &tree, row, "select");
    assert_eq!(selected["ok"], true, "{selected}");
    assert_eq!(selected["result"]["state"], "selected", "{selected}");
    assert!(selected["result"].get("restore_failed").is_none(), "nothing was taken from the user, yet: {selected}");
    assert!(selected["result"].get("foreground_changed").is_none(), "the user's window kept the foreground, yet: {selected}");
}

/// Ticket 153: as above, but the check box child itself takes the foreground when toggled
/// (`--check-takes-foreground`), as a control a proxy or a UWP app fronts does. The child is inside
/// the user's window, so nothing was taken from the user: neither `restore_failed` nor
/// `foreground_changed`, and the child is left in front.
#[test]
fn a_child_of_the_users_own_window_taking_the_foreground_reports_no_foreground_trouble() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--check-takes-foreground"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let target = hwnd(&window_id);
    assert_eq!(unsafe { GetForegroundWindow() }, target, "the fixture is not in front, so the test does not show the user's window as the target");

    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    let front = unsafe { GetForegroundWindow() };
    assert!(front != target && unsafe { GetAncestor(front, GA_ROOT) } == target, "the check box did not take the foreground, so the test shows nothing: {}", title(&mut helper, &window_id));
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert!(toggled["result"].get("restore_failed").is_none(), "nothing was taken from the user, yet: {toggled}");
    assert!(toggled["result"].get("foreground_changed").is_none(), "the user's window kept the foreground, yet: {toggled}");
}

/// Another process's ordinary window covers the whole fixture and holds the foreground, and the
/// cursor rests over the covered fixture. A background toggle and select change the elements'
/// states and read them back; the fixture is fronted by the Win32 proxy and restored each time, so
/// afterwards the foreground window, the visible windows above the fixture and the cursor are as
/// before, the fixture is still covered, no keyboard or pointer input reached it, and the results
/// say `foreground_restored: true` without `foreground_changed`. Focus is refused with
/// `focus_needs_foreground`.
#[test]
fn a_background_action_on_a_covered_window_changes_the_element_and_leaves_it_covered() {
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
    let root_at_middle = unsafe { GetAncestor(WindowFromPoint(middle), GA_ROOT) };
    assert_eq!(root_at_middle, hwnd(&cover), "the cover does not cover the fixture, so the test proves nothing");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, hwnd(&cover), "the cover is not in front, so the test does not show a target out of front");
    let above = windows_above(target);
    assert!(above.contains(&hwnd(&cover)), "the cover is not above the fixture in the z-order");
    let cursor_before = cursor();

    let check = index_of(&tree, |element| element["name"] == "Remember me");
    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(toggled["ok"], true, "{toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert_eq!(toggled["result"]["previous_state"], "off", "{toggled}");
    assert_eq!(toggled["result"]["foreground_restored"], true, "{toggled}");
    assert!(toggled["result"].get("foreground_changed").is_none(), "{toggled}");

    let row = index_of(&tree, |element| element["role"] == "list item" && element["name"] == "Row 2");
    let selected = act(&mut helper, &window_id, &tree, row, "select");
    assert_eq!(selected["ok"], true, "{selected}");
    assert_eq!(selected["result"]["state"], "selected", "{selected}");
    assert_eq!(selected["result"]["foreground_restored"], true, "{selected}");
    assert!(selected["result"].get("foreground_changed").is_none(), "{selected}");

    let after = read_tree(&mut helper, &window_id);
    assert!(has(find(&after, |element| element["name"] == "Remember me"), "states", "on"), "{after}");
    assert!(has(find(&after, |element| element["name"] == "Row 2"), "states", "selected"), "{after}");

    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "a background action changed the foreground window");
    assert_eq!(windows_above(target), above, "a background action changed the z-order above the fixture");
    assert_eq!(unsafe { GetAncestor(WindowFromPoint(middle), GA_ROOT) }, hwnd(&cover), "the fixture was raised over the cover");
    assert_eq!(cursor(), cursor_before, "a background action moved the cursor (physical mouse input meanwhile: {})", physical.seen());

    let edit = index_of(&tree, |element| element["role"] == "edit");
    let focused_before = read_tree(&mut helper, &window_id)["focused_index"].clone();
    let refused = act(&mut helper, &window_id, &tree, edit, "focus");
    assert_eq!(refused["error"]["code"], "background_unavailable", "{refused}");
    assert_eq!(refused["error"]["data"]["reason"], "focus_needs_foreground", "{refused}");
    assert_eq!(refused["error"]["data"]["recommended"], "foreground", "{refused}");
    assert_eq!(refused["error"]["data"]["input_sent"], false, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{refused}");
    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "the refused focus changed the foreground window");
    assert_eq!(read_tree(&mut helper, &window_id)["focused_index"], focused_before, "the refused focus moved the focus");

    let shown = title(&mut helper, &window_id);
    assert!(!shown.contains("key:") && !shown.contains("char:") && !shown.contains("click:"), "input reached the fixture: {shown}");
}

/// Ticket 151: the user's window (another process's ordinary, activatable window over the
/// target) holds the foreground and another process's ordinary window sits between it and the
/// target. Background actions that keep the target's
/// thread busy (a list row select, a check box toggle) front the target through the Win32 proxy;
/// after each one the middle window is still above the target, and the visible windows above the
/// target are exactly as before, so the target did not end directly below the user's window.
#[test]
fn a_background_action_leaves_a_window_between_the_user_window_and_the_target_above_the_target() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let mut tree = read_tree(&mut helper, &window_id);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));

    // As in the covered-window tests: the user's window is another app's ordinary window over the
    // target, and an ordinary window of a third process sits between them.
    let _middle_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("middle window"));
    let middle = wait_for_new_fixture(&mut helper, &window_id);
    let user_rect = (left, top, width, height);
    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("user window"));
    let user_id = wait_for_fixture_besides(&mut helper, &[&window_id, &middle]);
    place_window(&user_id, user_rect);
    for id in [&window_id, &middle, &user_id] {
        let activated = helper.request("activate_window", json!({ "window_id": id }));
        assert_eq!(activated["ok"], true, "could not foreground {id}: {activated}");
    }

    let target = hwnd(&window_id);
    let user = hwnd(&user_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window is not in front, so the test does not show a target out of front");
    // Windows higher than the user's window (the taskbar, other apps' always-on-top windows) may
    // reorder on their own; what the action must keep is the chain from the target up to the user.
    let up_to_user = |target: HWND| -> Vec<HWND> {
        let above = windows_above(target);
        let end = above.iter().position(|&window| window == user).map_or(above.len(), |at| at + 1);
        above[..end].to_vec()
    };
    let above = up_to_user(target);
    assert_eq!(above.first().copied(), Some(hwnd(&middle)), "the middle window is not just above the target, so the test proves nothing");
    assert_eq!(above.last().copied(), Some(user), "the user's window is not above the target");

    // The user keeps moving the mouse over their window while the actions run. Without this the
    // z-order held in about 400 actions; with it, it is lost in most runs.
    unsafe { SetCursorPos((left + width / 2) as i32, (top + height / 2) as i32) }.expect("SetCursorPos");
    let wiggle_stop = Arc::new(AtomicBool::new(false));
    let wiggle = {
        let stop = wiggle_stop.clone();
        std::thread::spawn(move || {
            use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
            let mut sign = 1;
            while !stop.load(Ordering::SeqCst) {
                let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 2 * sign, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } } };
                unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
                sign = -sign;
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    // Stops the moves even when an assertion fails, so later tests do not get them.
    struct StopOnDrop(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
            if let Some(thread) = self.1.take() {
                let _ = thread.join();
            }
        }
    }
    let _wiggle = StopOnDrop(wiggle_stop, Some(wiggle));
    let rows = ["Row 2", "Row 1", "Row 3", "Row 2", "Row 4", "Row 1", "Row 3", "Row 2", "Row 4", "Row 1", "Row 3", "Row 2"];
    for (step, row_name) in rows.iter().enumerate() {
        let row = index_of(&tree, |element| element["role"] == "list item" && element["name"] == *row_name);
        let selected = act(&mut helper, &window_id, &tree, row, "select");
        assert_eq!(selected["ok"], true, "{selected}");
        assert!(selected["result"].get("restore_failed").is_none(), "{selected}");
        // As the covered-window test does: the tree is read back before the z-order is looked at.
        tree = read_tree(&mut helper, &window_id);
        assert_eq!(unsafe { GetForegroundWindow() }, user, "select {step} ({row_name}) did not end with the user's window in front: {selected}");
        assert_eq!(
            up_to_user(target),
            above,
            "select {step} ({row_name}) changed the z-order above the target (middle {:?}, user {:?}): {selected}",
            hwnd(&middle),
            user
        );
    }
    // The target's thread is busy right as it is fronted: the toggle's click hangs it for a while.
    for hang_ms in [50usize, 150, 300, 600, 1000] {
        let armed = unsafe { SendMessageW(target, ARM_COMMAND_HANG, Some(WPARAM(hang_ms)), Some(LPARAM(0))) };
        assert_eq!(armed.0, 1, "the fixture did not arm the hang");
        let check = index_of(&tree, |element| element["name"] == "Remember me");
        let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
        assert_eq!(toggled["ok"], true, "{toggled}");
        assert!(toggled["result"].get("restore_failed").is_none(), "{toggled}");
        tree = read_tree(&mut helper, &window_id);
        assert_eq!(unsafe { GetForegroundWindow() }, user, "the toggle with a {hang_ms} ms hang did not end with the user's window in front: {toggled}");
        assert_eq!(
            up_to_user(target),
            above,
            "the toggle with a {hang_ms} ms hang changed the z-order above the target (middle {:?}, user {:?}): {toggled}",
            hwnd(&middle),
            user
        );
    }
}

/// The fixture's `ARM_COMMAND_HANG`: the next button click (BN_CLICKED) its window is told of
/// stops its UI thread for `wparam` milliseconds.
const ARM_COMMAND_HANG: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;

/// A minimized target is `background_unavailable` (`minimized`) and stays minimized, with the
/// foreground unchanged; an unknown `delivery_mode` is `invalid_params`.
#[test]
fn a_background_action_on_a_minimized_window_is_background_unavailable_and_restores_nothing() {
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

    let unknown = helper.request("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": check, "action": "toggle", "delivery_mode": "auto" }));
    assert_eq!(unknown["error"]["code"], "invalid_params", "{unknown}");

    let target = hwnd(&window_id);
    let _ = unsafe { ShowWindow(target, SW_MINIMIZE) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !unsafe { IsIconic(target) }.as_bool() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(unsafe { IsIconic(target) }.as_bool(), "the fixture did not minimize");
    std::thread::sleep(Duration::from_millis(200));
    let foreground = unsafe { GetForegroundWindow() };

    let refused = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(refused["error"]["code"], "background_unavailable", "{refused}");
    assert_eq!(refused["error"]["data"]["reason"], "minimized", "{refused}");
    assert_eq!(refused["error"]["data"]["recommended"], "foreground", "{refused}");
    assert_eq!(refused["error"]["data"]["input_sent"], false, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("Retry this action with delivery_mode: \"foreground\""), "{refused}");
    assert!(unsafe { IsIconic(target) }.as_bool(), "the refused action restored the fixture");
    assert_eq!(unsafe { GetForegroundWindow() }, foreground, "the refused action changed the foreground window");
    // A minimized window's tree leaves its controls out: it is restored to read the check box.
    let _ = unsafe { ShowWindow(target, SW_RESTORE) };
    std::thread::sleep(Duration::from_millis(300));
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "off"), "the refused toggle was done");
}

/// The fixture's modal dialog is open while another window holds the foreground: a background
/// action on the fixture is `dialog_open` naming the dialog, and the foreground stays.
#[test]
fn a_background_action_on_a_window_with_an_open_dialog_is_dialog_open() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let first = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture A"));
    let mut helper = Helper::spawn();
    let a = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tree = read_tree(&mut helper, &a);
    let second = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture B"));
    let b = wait_for_new_fixture(&mut helper, &a);

    let activated = helper.request("activate_window", json!({ "window_id": a }));
    assert_eq!(activated["ok"], true, "could not foreground fixture A: {activated}");
    let info = helper.request("get_window", json!({ "window_id": a }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let opened = helper.request("click", json!({ "window_id": a, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "clicking the button that opens the dialog failed: {opened}");
    let dialog = wait_for_window(&mut helper, "CUMCP_DIALOG");
    let activated = helper.request("activate_window", json!({ "window_id": b }));
    assert_eq!(activated["ok"], true, "could not foreground fixture B: {activated}");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&b), "B is not in front, so the test does not show a target out of front");

    let refused = act(&mut helper, &a, &tree, 1, "invoke");
    assert_eq!(refused["error"]["code"], "dialog_open", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(dialog), "{refused}");
    assert_eq!(refused["error"]["data"]["input_sent"], json!(false), "{refused}");
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd(&b), "the refused action changed the foreground window");
    drop(helper);
    drop(first);
    drop(second);
}

/// Ticket 154: a visible, enabled, titled window the target owns that cannot be activated
/// (`WS_EX_NOACTIVATE`), like the popup host a XAML menu leaves behind, is no dialog: a background
/// action on the target is done, not `dialog_open`.
#[test]
fn a_background_action_on_a_window_with_an_owned_no_activate_popup_is_done() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--noactivate-popup"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    // The target is the owner of the popup, so another fixture window on the desktop is not taken for it.
    let popup = wait_for_window(&mut helper, "CUMCP_NOACTIVATE_POPUP");
    let described = helper.request("get_window", json!({ "window_id": popup }));
    let window_id = described["result"]["owner_window_id"].as_str().unwrap_or_else(|| panic!("the popup has no owner: {described}")).to_string();
    let tree = read_tree(&mut helper, &window_id);
    let check = index_of(&tree, |element| element["name"] == "Remember me");

    // Any fixture window already on the desktop is not the user's window.
    let before = helper.request("list_windows", json!({}));
    let seen: Vec<String> = before["result"]["windows"]
        .as_array()
        .map(|windows| windows.iter().filter(|window| window["title"].as_str().unwrap_or("").starts_with("CUMCP_FIXTURE")).filter_map(|window| Some(window["window_id"].as_str()?.to_string())).collect())
        .unwrap_or_default();
    let _user_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--edit-keeps-focus").spawn().expect("user window"));
    let user_id = wait_for_fixture_besides(&mut helper, &seen.iter().collect::<Vec<_>>());
    let activated = helper.request("activate_window", json!({ "window_id": user_id }));
    assert_eq!(activated["ok"], true, "could not foreground the user's window: {activated}");
    let user = hwnd(&user_id);
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the user's window is not in front, so the test does not show a target out of front");
    assert_ne!(popup, window_id, "the popup is the target");

    let toggled = act(&mut helper, &window_id, &tree, check, "toggle");
    assert_eq!(toggled["ok"], true, "an owned no-activate popup made the action fail: {toggled}");
    assert_eq!(toggled["result"]["state"], "on", "{toggled}");
    assert_eq!(unsafe { GetForegroundWindow() }, user, "the foreground did not end with the user's window");
    assert!(has(find(&read_tree(&mut helper, &window_id), |element| element["name"] == "Remember me"), "states", "on"), "the toggle did not take effect");
}

fn act(helper: &mut Helper, window_id: &str, tree: &Value, index: u64, action: &str) -> Value {
    helper.request("element_action", json!({ "window_id": window_id, "tree_id": tree["tree_id"], "index": index, "action": action, "delivery_mode": "background" }))
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
