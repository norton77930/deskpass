//! Ticket 59: `get_ui_tree` on the fixture window, read through UI Automation.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::w;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetWindowTextW};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The text the fixture puts in its password box (`--ui-controls`).
const SECRET: &str = "CUMCP-secret-42";

#[test]
fn the_fixture_tree_lists_its_edit_button_check_box_and_password_box() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the focus check must fail rather than skip: {activated}");
    // Activation focuses the fixture window itself; a click puts the focus in the edit.
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": width * 240 / 480, "y": height * 24 / 360 }));
    assert_eq!(clicked["ok"], true, "{clicked}");

    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], true, "{read}");
    let tree = &read["result"];
    assert!(tree["tree_id"].as_str().is_some_and(|id| id.starts_with('t')), "{tree}");
    assert!(tree["window_title"].as_str().is_some_and(|title| title.starts_with("CUMCP_FIXTURE")), "{tree}");
    assert!(tree.get("truncated").is_none(), "{tree}");
    let elements = tree["elements"].as_array().expect("elements");

    let edit = find(elements, |element| element["role"] == "edit" && !has(element, "states", "password"));
    assert_eq!(edit["value"], "", "{edit}");
    for action in ["click", "set_value", "focus"] {
        assert!(has(edit, "actions", action), "edit lacks {action}: {edit}");
    }
    assert!(has(edit, "states", "focused"), "{edit}");
    assert_eq!(tree["focused_index"], edit["index"], "{tree}");
    let bounds = &edit["bounds"];
    // The edit is 10, 10 and 460x28 in client pixels at 96 DPI; the fixture's popup frame is its client area.
    assert!(bounds["x"].as_i64().is_some_and(|x| (5..=40).contains(&x)), "{edit}");
    assert!(bounds["width"].as_i64().is_some_and(|width| width > 100), "{edit}");

    let button = find(elements, |element| element["role"] == "button" && element["name"] == "Click me");
    assert!(button["index"].as_u64().is_some(), "{button}");
    assert!(has(button, "actions", "invoke"), "{button}");
    assert!(has(button, "actions", "click"), "{button}");

    let check = find(elements, |element| element["role"] == "check box" && element["name"] == "Remember me");
    assert!(has(check, "states", "off"), "{check}");
    assert!(has(check, "actions", "toggle"), "{check}");

    let password = find(elements, |element| has(element, "states", "password"));
    assert_eq!(password["role"], "edit", "{password}");
    assert!(password.get("value").is_none(), "a password box must not carry a value: {password}");
    assert!(has(password, "actions", "set_value"), "{password}");
    assert!(!read.to_string().contains(SECRET), "the password text reached the result: {read}");

    // Indexes count from 1 in walk order.
    let indexes: Vec<u64> = elements.iter().filter_map(|element| element["index"].as_u64()).collect();
    assert_eq!(indexes, (1..=indexes.len() as u64).collect::<Vec<_>>(), "{tree}");
    // Each read is a new tree.
    let again = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(again["ok"], true, "{again}");
    assert_ne!(again["result"]["tree_id"], tree["tree_id"], "{again}");
}

#[test]
fn a_stopped_helper_answers_get_ui_tree_with_interrupted() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let stopped = helper.request("debug_stop", json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], false, "{read}");
    assert_eq!(read["error"]["code"], "interrupted", "{read}");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], true, "{read}");
}

/// The fixture's cover window is a bare popup with no Control View children, so the walk visits
/// no element: the stop is still answered `interrupted`, not with an empty tree.
#[test]
fn a_stopped_helper_answers_interrupted_for_a_window_without_elements() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _cover = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", "120,120,240,160", "--not-topmost"]).spawn().expect("cover"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_window(&mut helper, "CUMCP_COVER");
    let stopped = helper.request("debug_stop", json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["error"]["code"], "interrupted", "{read}");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["elements"], json!([]), "the fixture has elements, so this test proves nothing: {read}");
}

/// A tree id names one helper process's tree: a restarted helper never hands out an id the
/// previous one did, so an id kept from before cannot name a tree of the new one.
#[test]
fn two_helpers_do_not_issue_the_same_id_for_their_first_tree() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut first = Helper::spawn();
    let window_id = wait_for_fixture(&mut first);
    let earlier = first.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(earlier["ok"], true, "{earlier}");
    drop(first);
    let mut second = Helper::spawn();
    let later = second.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(later["ok"], true, "{later}");
    let (earlier, later) = (earlier["result"]["tree_id"].as_str().expect("tree_id"), later["result"]["tree_id"].as_str().expect("tree_id"));
    assert_ne!(earlier, later, "a restarted helper issued the same tree id");
    assert!(later.len() <= 12, "the tree id is not short: {later}");
}

/// The helper's own windows (here the overlay) are refused like `hwnd_from` refuses them for input.
#[test]
fn get_ui_tree_refuses_the_helpers_own_overlay() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let shown = helper.request("overlay_show", json!({ "window_id": window_id }));
    assert_eq!(shown["ok"], true, "{shown}");
    let deadline = Instant::now() + Duration::from_secs(5);
    let overlay = loop {
        if let Ok(overlay) = unsafe { FindWindowW(w!("DESKPASS_OVERLAY"), None) } {
            break overlay;
        }
        assert!(Instant::now() < deadline, "the overlay window was not created");
        std::thread::sleep(Duration::from_millis(50));
    };
    let refused = helper.request("get_ui_tree", json!({ "window_id": (overlay.0 as isize).to_string() }));
    let _ = helper.request("overlay_hide", json!({}));
    assert_eq!(refused["error"]["code"], "window_not_found", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or("").contains("belongs to Deskpass itself"), "{refused}");
}

#[test]
fn get_ui_tree_refuses_another_apps_window_and_a_closed_window() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-controls").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let app = info["result"]["app"].as_str().expect("app").to_string();
    let matching = helper.request("get_ui_tree", json!({ "window_id": window_id, "expected_app": app.to_uppercase() }));
    assert_eq!(matching["ok"], true, "the app is compared without regard to case: {matching}");
    let other = helper.request("get_ui_tree", json!({ "window_id": window_id, "expected_app": "C:\\Apps\\other.exe" }));
    assert_eq!(other["ok"], false, "{other}");
    assert_eq!(other["error"]["code"], "app_changed", "{other}");

    let _ = fixture.0.kill();
    let _ = fixture.0.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    let closed = loop {
        let closed = helper.request("get_ui_tree", json!({ "window_id": window_id, "expected_app": app }));
        if closed["ok"] == false || Instant::now() > deadline {
            break closed;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(closed["error"]["code"], "window_not_found", "{closed}");
}

/// The fixture's UI thread sleeps 5 s the first time another process asks the check box for its
/// name. Ticket 69: a tree read fetches a parent's children with their names in one call, and the
/// check box is one of the window's own children, so the call that hangs is the first one, for
/// those children: the partial tree has nothing in it yet.
#[test]
fn a_hung_app_gets_a_partial_tree_within_three_seconds_and_the_next_read_is_whole() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--hang-name", "5000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);

    let started = Instant::now();
    let partial = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    let waited = started.elapsed();
    assert_eq!(partial["ok"], true, "{partial}");
    assert!(waited < Duration::from_millis(3_500), "the helper waited {waited:?}");
    assert!(waited >= Duration::from_millis(2_900), "the read did not hang, so this test proves nothing ({waited:?}): {partial}");
    assert_eq!(partial["result"]["truncated"], json!(["timeout"]), "{partial}");
    assert_eq!(partial["result"]["elements"], json!([]), "{partial}");

    // The stuck thread is left behind; once the app answers again a read on a fresh one is whole.
    std::thread::sleep(Duration::from_millis(5_000).saturating_sub(started.elapsed()) + Duration::from_millis(300));
    let whole = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(whole["ok"], true, "{whole}");
    assert!(whole["result"].get("truncated").is_none(), "{whole}");
    let elements = whole["result"]["elements"].as_array().expect("elements");
    find(elements, |element| element["role"] == "check box" && element["name"] == "Remember me");
    assert_ne!(whole["result"]["tree_id"], partial["result"]["tree_id"]);
}

/// Ticket 69: the app hangs one level down. The fixture's UI thread sleeps 5 s the first time
/// another process asks the section's button for its name, which is part of the call that fetches
/// the section's children; the window's own children, the section included, are fetched by the call
/// before it and answer at once. The walk reads depth first, so what it reached before that call
/// stays in the partial tree and the section's children are what is lost.
#[test]
fn a_hang_below_the_top_level_keeps_the_levels_above_it_in_the_partial_tree() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-section", "--hang-nested", "5000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);

    let started = Instant::now();
    let partial = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    let waited = started.elapsed();
    eprintln!("69 nested hang: read took {waited:?}");
    assert_eq!(partial["ok"], true, "{partial}");
    assert!(waited < Duration::from_millis(3_500), "the helper waited {waited:?}");
    assert!(waited >= Duration::from_millis(2_900), "the read did not hang, so this test proves nothing ({waited:?}): {partial}");
    assert_eq!(partial["result"]["truncated"], json!(["timeout"]), "{partial}");
    let elements = partial["result"]["elements"].as_array().expect("elements");
    // The top level, the section included, was read; the section's own children were not.
    find(elements, |element| element["role"] == "edit" && !has(element, "states", "password"));
    find(elements, |element| element["role"] == "button" && element["name"] == "Click me");
    find(elements, |element| element["role"] == "check box" && element["name"] == "Remember me");
    find(elements, |element| element["name"] == "Section");
    for lost in ["Close section", "Section text"] {
        assert!(!elements.iter().any(|element| element["name"] == lost), "{lost} is below the level that hung: {partial}");
    }

    // The stuck thread is left behind; once the app answers again a read on a fresh one is whole.
    std::thread::sleep(Duration::from_millis(5_000).saturating_sub(started.elapsed()) + Duration::from_millis(300));
    let whole = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(whole["ok"], true, "{whole}");
    assert!(whole["result"].get("truncated").is_none(), "{whole}");
    let elements = whole["result"]["elements"].as_array().expect("elements");
    find(elements, |element| element["role"] == "button" && element["name"] == "Close section");
}

/// Ticket 60: the fixture's `--ui-content` controls, with their states and actions, the limits
/// on a parent's children, and what is left out.
#[test]
fn the_tree_shows_a_dropdown_list_tree_view_disabled_button_and_omits_children_past_sixty() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let started = Instant::now();
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    eprintln!("ui-content tree read in {:?}", started.elapsed());
    assert_eq!(read["ok"], true, "{read}");
    let tree = &read["result"];
    assert!(tree.get("truncated").is_none(), "{tree}");
    let elements = tree["elements"].as_array().expect("elements");
    eprintln!("{} lines: {}", elements.len(), Value::from(elements.iter().filter(|element| !element["name"].as_str().unwrap_or("").starts_with('B')).cloned().collect::<Vec<_>>()));

    let disabled = find(elements, |element| element["role"] == "button" && element["name"] == "Disabled");
    assert!(disabled["index"].as_u64().is_some(), "{disabled}");
    assert_eq!(disabled["states"], json!(["disabled"]), "{disabled}");
    assert_eq!(disabled["actions"], json!([]), "a disabled element lists no actions: {disabled}");

    let combo = find(elements, |element| element["role"] == "combo box");
    assert_eq!(combo["value"], "Red", "{combo}");
    assert!(has(combo, "states", "collapsed"), "{combo}");
    assert!(has(combo, "actions", "expand") && !has(combo, "actions", "collapse"), "only what the state allows: {combo}");
    assert!(has(combo, "actions", "click"), "{combo}");

    // Rows 1 to 4 are whole, row 5 is partly visible, rows 6 on are scrolled out of the list.
    for row in 1..=5 {
        let item = find(elements, |element| element["role"] == "list item" && element["name"] == format!("Row {row}"));
        assert!(has(item, "actions", "select"), "{item}");
    }
    let partial = find(elements, |element| element["name"] == "Row 5");
    assert!(partial["bounds"]["height"].as_i64().is_some_and(|height| height > 0), "{partial}");
    for row in 6..=20 {
        assert!(!elements.iter().any(|element| element["name"] == format!("Row {row}")), "Row {row} is scrolled out of the list, so it is not in the tree: {tree}");
    }

    let fruits = find(elements, |element| element["role"] == "tree item" && element["name"] == "Fruits");
    assert!(has(fruits, "states", "expanded"), "{fruits}");
    assert!(has(fruits, "actions", "collapse") && !has(fruits, "actions", "expand"), "{fruits}");
    let apple = find(elements, |element| element["role"] == "tree item" && element["name"] == "Apple");
    assert!(apple["depth"].as_u64() > fruits["depth"].as_u64(), "{apple} {fruits}");
    let vegetables = find(elements, |element| element["role"] == "tree item" && element["name"] == "Vegetables");
    assert!(has(vegetables, "states", "collapsed"), "{vegetables}");
    assert!(has(vegetables, "actions", "expand") && !has(vegetables, "actions", "collapse"), "{vegetables}");
    assert!(!elements.iter().any(|element| element["name"] == "Carrot"), "nothing is expanded to read the tree: {tree}");

    // The container shows 60 of its 70 buttons and counts the other ten.
    let at = elements.iter().position(|element| element["name"] == "Many buttons").unwrap_or_else(|| panic!("no container in {tree}"));
    let container = &elements[at];
    assert_eq!(container.get("index"), None, "a named container is a text line: {container}");
    assert_eq!(container["omitted_children"], 10, "{container}");
    let depth = container["depth"].as_u64().expect("depth");
    let buttons: Vec<&Value> = elements[at + 1..].iter().take_while(|element| element["depth"].as_u64() > Some(depth)).collect();
    assert_eq!(buttons.len(), 60, "{}", Value::from(buttons.iter().map(|button| (*button).clone()).collect::<Vec<_>>()));
    assert_eq!(buttons[59]["name"], "B60");
    assert!(!elements.iter().any(|element| element["name"] == "B61"), "{tree}");
}

#[test]
fn a_tree_past_five_hundred_lines_is_cut_with_truncated_nodes() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    // About 110 lines, then nine lists that show 60 rows each.
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-content", "--ui-lists", "9"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let started = Instant::now();
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    eprintln!("nine-list tree read in {:?}", started.elapsed());
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["truncated"], json!(["nodes"]), "{}", read["result"]["truncated"]);
    let elements = read["result"]["elements"].as_array().expect("elements");
    assert_eq!(elements.len(), 500);
    assert!(elements.iter().any(|element| element["name"] == "L1-60"), "the lists were reached");
    assert!(!elements.iter().any(|element| element["name"] == "L9-1"), "the walk stopped before the last list");
}

/// The fixture's `--popup-menu` menu is its own popup (ticket 38), so its items are in its tree,
/// even the ones below the window.
#[test]
fn an_open_menu_of_the_window_is_in_its_tree() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--popup-menu"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let closed = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(!closed.to_string().contains("Menu item"), "{closed}");

    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    // A right-click on the background below the Dialog button opens the menu.
    let opened = helper.request("click", json!({ "window_id": window_id, "x": width * 300 / 480, "y": height * 340 / 360, "button": "right" }));
    assert_eq!(opened["ok"], true, "{opened}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
        if title.contains("menulast:") {
            break;
        }
        assert!(Instant::now() < deadline, "the menu did not open: {title}");
        std::thread::sleep(Duration::from_millis(50));
    }

    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    let _ = helper.request("press_key", json!({ "window_id": window_id, "keys": ["escape"] }));
    assert_eq!(read["ok"], true, "{read}");
    let elements = read["result"]["elements"].as_array().expect("elements");
    let items: Vec<&Value> = elements.iter().filter(|element| element["role"] == "menu item").collect();
    let names: Vec<&str> = items.iter().filter_map(|item| item["name"].as_str()).collect();
    assert_eq!(names, (1..=8).map(|number| format!("Menu item {number}")).collect::<Vec<_>>(), "{}", read["result"]);
    assert!(items.iter().all(|item| item["index"].as_u64().is_some()), "{}", read["result"]);
    // The menu sticks out left of and below the window; its last item lies below it.
    let last = items[7];
    assert!(last["bounds"]["x"].as_i64().is_some_and(|x| x < 0), "{last}");
    assert!(last["bounds"]["y"].as_i64().is_some_and(|y| y >= height), "{last}");
    // The window's own elements come first.
    let edit = elements.iter().position(|element| element["role"] == "edit").expect("edit");
    let first_item = elements.iter().position(|element| element["role"] == "menu item").expect("item");
    assert!(edit < first_item, "{}", read["result"]);
}

/// Ticket 63: a query finds the 65th of the 70 buttons a plain tree cuts at 60, shown under its
/// container and clickable by its number; it matches values without regard to case, keeps the
/// numbers of ancestors, leaves out what neither matches nor holds a match, and never matches a
/// password box's text.
#[test]
fn a_query_finds_elements_past_the_limits_by_name_or_value_and_they_can_be_clicked() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--ui-read-only"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let started = Instant::now();
    let plain = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    eprintln!("plain tree read in {:?}", started.elapsed());
    assert_eq!(plain["ok"], true, "{plain}");
    assert!(!plain.to_string().contains("\"B65\""), "the plain tree must cut B65, or this test proves nothing: {plain}");

    let started = Instant::now();
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id, "query": "b65" }));
    eprintln!("query tree read in {:?}", started.elapsed());
    assert_eq!(read["ok"], true, "{read}");
    let tree = &read["result"];
    assert!(tree.get("truncated").is_none(), "{tree}");
    let elements = tree["elements"].as_array().expect("elements");
    let names: Vec<&str> = elements.iter().filter_map(|element| element["name"].as_str()).collect();
    assert_eq!(names, ["Many buttons", "B65"], "only the match and its ancestor: {tree}");
    let (container, button) = (&elements[0], &elements[1]);
    assert_eq!(container.get("index"), None, "{container}");
    assert_eq!(container.get("omitted_children"), None, "{container}");
    assert_eq!(button["depth"].as_u64(), container["depth"].as_u64().map(|depth| depth + 1), "{tree}");
    assert_eq!(button["index"], 1, "{button}");

    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the click must fail rather than skip: {activated}");
    let clicked = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": tree["tree_id"], "index": 1 } }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    std::thread::sleep(Duration::from_millis(150));
    let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
    assert!(title.contains("many:B65"), "the click by number did not reach B65: {title}");

    // An ancestor that is numbered keeps its number and actions; its other children are not shown.
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id, "query": "APPLE" }));
    assert_eq!(read["ok"], true, "{read}");
    let elements = read["result"]["elements"].as_array().expect("elements");
    let fruits = find(elements, |element| element["name"] == "Fruits");
    assert_eq!(fruits["index"], 1, "{fruits}");
    assert!(has(fruits, "actions", "collapse"), "{fruits}");
    let apple = find(elements, |element| element["name"] == "Apple");
    assert_eq!(apple["index"], 2, "{apple}");
    assert!(!elements.iter().any(|element| element["name"] == "Banana" || element["name"] == "Vegetables"), "{}", read["result"]);

    // By value, without regard to case.
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id, "query": "FIXED TEXT" }));
    assert_eq!(read["ok"], true, "{read}");
    let elements = read["result"]["elements"].as_array().expect("elements");
    let fixed = find(elements, |element| element["value"] == "Fixed text");
    assert!(!fixed["name"].as_str().unwrap_or("").to_lowercase().contains("fixed"), "matched by name, so this proves nothing: {fixed}");
    assert!(fixed["index"].as_u64().is_some(), "{fixed}");

    // The password box's text never takes part.
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id, "query": "secret-42" }));
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["elements"], json!([]), "{read}");
    assert!(!read.to_string().contains(SECRET), "{read}");

    // Not text: refused; empty: no query.
    let refused = helper.request("get_ui_tree", json!({ "window_id": window_id, "query": 5 }));
    assert_eq!(refused["error"]["code"], "invalid_params", "{refused}");
    let empty = helper.request("get_ui_tree", json!({ "window_id": window_id, "query": "" }));
    assert_eq!(empty["result"]["elements"].as_array().map(Vec::len), plain["result"]["elements"].as_array().map(Vec::len), "{empty}");
}

/// Ticket 66 (seam B, what the fixture can show): a window of class `Chrome_WidgetWin_1`, or one
/// with a child window of that class, whose tree has no web area is read again every 50 ms for
/// 500 ms and then answered with `web_content_empty: true`; any other window is answered at once
/// without the field. The real web area (a Document element) is only in the manual verification.
#[test]
fn a_chromium_window_without_a_web_area_waits_half_a_second_then_says_so() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut read_after = |args: &[&str]| {
        let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(args).spawn().expect("fixture"));
        let window_id = wait_for_fixture(&mut helper);
        let started = Instant::now();
        let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
        let waited = started.elapsed();
        assert_eq!(read["ok"], true, "{read}");
        (read["result"].clone(), waited)
    };
    let (plain, plain_took) = read_after(&["--ui-controls"]);
    assert!(plain.get("web_content_empty").is_none(), "{plain}");
    assert!(plain_took < Duration::from_millis(450), "a window that is not Chromium must not wait: {plain_took:?}");
    for mode in ["top", "child"] {
        let (tree, took) = read_after(&["--ui-controls", "--chromium-class", mode]);
        assert_eq!(tree["web_content_empty"], true, "{mode}: {tree}");
        assert!(took >= Duration::from_millis(500) && took < Duration::from_millis(2500), "{mode}: waited {took:?}");
        assert!(tree.get("truncated").is_none(), "{mode}: the wait is not a timeout: {tree}");
        assert!(tree["elements"].as_array().is_some_and(|elements| !elements.is_empty()), "{mode}: the tree is returned as it is: {tree}");
    }
}

/// Ticket 71 (checkbox 3): web content that appears while the wait runs ends the wait, and the
/// tree has it without `web_content_empty`. The fixture creates a large Document holding a button
/// 300 ms after its window, inside the 500 ms the read waits.
#[test]
fn web_content_that_appears_during_the_wait_is_in_the_tree_and_not_called_empty() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let _fixture = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fixture-window"))
            .args(["--chromium-class", "top", "--web-documents", "large-full", "--web-documents-after", "300"])
            .spawn()
            .expect("fixture"),
    );
    let window_id = wait_for_fixture(&mut helper);
    let (tree, took) = read_timed(&mut helper, &window_id);
    eprintln!("71 content during the wait: read took {took:?}");
    assert!(tree.get("web_content_empty").is_none(), "{tree}");
    let elements = tree["elements"].as_array().expect("elements");
    find(elements, |element| element["role"] == "document");
    find(elements, |element| element["role"] == "button" && element["name"] == "Inside");
    assert!(took < Duration::from_millis(2500), "{took:?}");
}

/// Ticket 71 (checkbox 1): a Chromium window whose web content was still empty after the wait is
/// remembered with its title; the next read of it does not wait, still says it is empty, and a read
/// after the title changed (a new page) waits again.
#[test]
fn a_window_found_empty_is_not_waited_for_again_until_its_title_changes() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--chromium-class", "top"]).spawn().expect("fixture"));
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void);

    let (first, first_took) = read_timed(&mut helper, &window_id);
    assert_eq!(first["web_content_empty"], true, "{first}");
    assert!(first_took >= Duration::from_millis(500), "the first read waits: {first_took:?}");
    let (second, second_took) = read_timed(&mut helper, &window_id);
    assert_eq!(second["web_content_empty"], true, "still empty, still said so: {second}");
    assert!(second["elements"].as_array().is_some_and(|elements| !elements.is_empty()), "{second}");
    assert!(second_took + Duration::from_millis(300) < first_took, "the second read must not wait again: {second_took:?} after {first_took:?}");

    unsafe { SetWindowTextW(hwnd, w!("CUMCP_FIXTURE page two")).expect("title") };
    let (third, third_took) = read_timed(&mut helper, &window_id);
    assert_eq!(third["window_title"], "CUMCP_FIXTURE page two", "{third}");
    assert_eq!(third["web_content_empty"], true, "{third}");
    assert!(third_took >= Duration::from_millis(500), "a new title waits again: {third_took:?}");
    let (_, fourth_took) = read_timed(&mut helper, &window_id);
    assert!(fourth_took + Duration::from_millis(300) < third_took, "{fourth_took:?} after {third_took:?}");
    eprintln!("71 empty window: reads took {first_took:?}, {second_took:?}, then after a new title {third_took:?}, {fourth_took:?}");
}

/// Ticket 71: a window remembered as empty whose web content has come since (same title) is not
/// called empty any more, and is not waited for either.
#[test]
fn a_window_found_empty_is_not_called_empty_once_its_web_content_has_come() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let _fixture = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fixture-window"))
            .args(["--chromium-class", "top", "--web-documents", "large-full", "--web-documents-after", "1200"])
            .spawn()
            .expect("fixture"),
    );
    let window_id = wait_for_fixture(&mut helper);
    let (first, first_took) = read_timed(&mut helper, &window_id);
    assert_eq!(first["web_content_empty"], true, "the web content comes after the wait: {first}");
    std::thread::sleep(Duration::from_millis(1000));
    let (second, second_took) = read_timed(&mut helper, &window_id);
    eprintln!("71 content after the wait: reads took {first_took:?}, {second_took:?}");
    assert!(second.get("web_content_empty").is_none(), "{second}");
    find(second["elements"].as_array().expect("elements"), |element| element["role"] == "button" && element["name"] == "Inside");
    assert!(second_took < Duration::from_millis(450), "{second_took:?}");
}

/// Ticket 71 (checkbox 2): the web content is ready when the largest Document has children, not
/// when any Document has. A small Document with content beside a large empty one is not ready (the
/// read waits, then says empty); the other way round it is, at once.
#[test]
fn only_the_largest_document_having_content_makes_the_web_content_ready() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut read_with = |documents: &str| {
        let _fixture = KillOnDrop(
            Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--chromium-class", "top", "--web-documents", documents]).spawn().expect("fixture"),
        );
        let window_id = wait_for_fixture(&mut helper);
        read_timed(&mut helper, &window_id)
    };
    let (side_bar_only, waited) = read_with("small-full,large-empty");
    eprintln!("71 small full + large empty: read took {waited:?}");
    assert_eq!(side_bar_only["web_content_empty"], true, "{side_bar_only}");
    assert!(waited >= Duration::from_millis(500), "a side bar with content is not the main content: {waited:?}");
    let elements = side_bar_only["elements"].as_array().expect("elements");
    find(elements, |element| element["role"] == "button" && element["name"] == "Inside");

    let (main_content, ready) = read_with("small-empty,large-full");
    eprintln!("71 small empty + large full: read took {ready:?}");
    assert!(main_content.get("web_content_empty").is_none(), "{main_content}");
    assert!(ready < Duration::from_millis(450), "ready at once: {ready:?}");
}

/// Reference-gaps G2: `find_in_ui_tree` answers whether a query matches by `get_ui_tree`'s rule
/// (name or value, without regard to case, past the limits a plain tree cuts at, never a password
/// box's text), keeps no tree and issues no id, and so leaves the trees a caller holds alone: more
/// polls than the five trees a window keeps, and an element of a tree read before them still acts.
#[test]
fn find_in_ui_tree_polls_by_the_query_rule_and_keeps_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--ui-read-only"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let app = helper.request("get_window", json!({ "window_id": window_id }))["result"]["app"].as_str().expect("app").to_string();

    let held = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(held["ok"], true, "{held}");
    let held_id = held["result"]["tree_id"].clone();
    let check = find(held["result"]["elements"].as_array().expect("elements"), |element| element["name"] == "Remember me")["index"].clone();
    assert!(check.as_u64().is_some(), "{held}");

    // Nine polls: more than the five trees a window keeps. Each answers only whether it found, with no tree.
    for _ in 0..9 {
        let polled = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "expected_app": app, "query": "REMEMBER" }));
        assert_eq!(polled["ok"], true, "{polled}");
        assert_eq!(polled["result"]["found"], true, "{polled}");
        for field in ["tree_id", "elements", "window_title", "truncated"] {
            assert!(polled["result"].get(field).is_none(), "a poll must not carry {field}: {polled}");
        }
    }
    // The tree read before them is still kept, so its element still takes an action (a dropped tree is refused).
    let toggled = helper.request("element_action", json!({ "window_id": window_id, "tree_id": held_id, "index": check, "action": "toggle" }));
    assert_eq!(toggled["ok"], true, "the polls dropped the tree the caller holds: {toggled}");

    // Not there; by value and without regard to case; past the 60 children a plain tree shows; never a password box's text.
    let miss = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "query": "no such text anywhere" }));
    assert_eq!(miss["result"]["found"], false, "{miss}");
    for (query, label) in [("FIXED TEXT", "by value"), ("b65", "past the limits")] {
        let hit = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "query": query }));
        assert_eq!(hit["result"]["found"], true, "{label}: {hit}");
    }
    let secret = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "query": "secret-42" }));
    assert_eq!(secret["result"]["found"], false, "{secret}");
    assert!(!secret.to_string().contains(SECRET), "{secret}");

    // A query is required, and is text.
    for params in [json!({ "window_id": window_id }), json!({ "window_id": window_id, "query": "" }), json!({ "window_id": window_id, "query": 5 })] {
        let refused = helper.request("find_in_ui_tree", params.clone());
        assert_eq!(refused["error"]["code"], "invalid_params", "{params}: {refused}");
    }
    // The app is compared as get_ui_tree compares it.
    let other = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "expected_app": "C:\\Apps\\other.exe", "query": "x" }));
    assert_eq!(other["error"]["code"], "app_changed", "{other}");

    // A stopped helper answers interrupted, and one that is let go on answers again.
    assert_eq!(helper.request("debug_stop", json!({}))["ok"], true);
    let stopped = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "query": "x" }));
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    assert_eq!(helper.request("resume", json!({}))["ok"], true);

    // A window that is closed is window_not_found, not an empty answer.
    let _ = fixture.0.kill();
    let _ = fixture.0.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    let closed = loop {
        let closed = helper.request("find_in_ui_tree", json!({ "window_id": window_id, "expected_app": app, "query": "x" }));
        if closed["ok"] == false || Instant::now() > deadline {
            break closed;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(closed["error"]["code"], "window_not_found", "{closed}");
}

/// Reference-gaps G3: `max_depth` shows that many levels from the top and counts what is below on
/// the last one (`truncated: depth`), at any depth up to the 30 the walk always had; the new
/// parameters are checked; every read is a tree of its own.
#[test]
fn max_depth_cuts_the_levels_below_it_and_counts_the_children_left_out() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--ui-read-only"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let plain = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(plain["ok"], true, "{plain}");
    let plain_elements = plain["result"]["elements"].as_array().expect("elements");
    let fruits = find(plain_elements, |element| element["role"] == "tree item" && element["name"] == "Fruits");
    let apple = find(plain_elements, |element| element["role"] == "tree item" && element["name"] == "Apple");
    let (fruits_depth, apple_depth) = (fruits["depth"].as_u64().expect("depth"), apple["depth"].as_u64().expect("depth"));
    assert!(apple_depth > fruits_depth, "{fruits} {apple}");
    let container = find(plain_elements, |element| element["name"] == "Many buttons");
    let container_depth = container["depth"].as_u64().expect("depth");

    // Down to the tree item and no further: Apple is counted on Fruits, and the cut is named.
    let cut = helper.request("get_ui_tree", json!({ "window_id": window_id, "max_depth": fruits_depth + 1 }));
    assert_eq!(cut["ok"], true, "{cut}");
    let tree = &cut["result"];
    assert_eq!(tree["truncated"], json!(["depth"]), "{tree}");
    let elements = tree["elements"].as_array().expect("elements");
    assert!(elements.iter().all(|element| element["depth"].as_u64().is_some_and(|depth| depth <= fruits_depth)), "{tree}");
    assert!(!elements.iter().any(|element| element["name"] == "Apple"), "{tree}");
    let cut_fruits = find(elements, |element| element["name"] == "Fruits");
    assert!(cut_fruits["omitted_children"].as_u64().is_some_and(|count| count >= 1), "the children below the limit are counted: {cut_fruits}");
    assert_ne!(tree["tree_id"], plain["result"]["tree_id"], "{tree}");
    // Numbered from 1 in walk order, as always.
    assert!(elements.iter().filter_map(|element| element["index"].as_u64()).zip(1..).all(|(index, expected)| index == expected), "{tree}");

    // The 70 buttons are the container's children, none shown at the level above them, all counted.
    let top = helper.request("get_ui_tree", json!({ "window_id": window_id, "max_depth": container_depth + 1 }));
    let elements = top["result"]["elements"].as_array().expect("elements");
    let shut = find(elements, |element| element["name"] == "Many buttons");
    assert_eq!(shut["omitted_children"], 70, "{shut}");
    assert!(!elements.iter().any(|element| element["name"] == "B1"), "{}", top["result"]);

    // The most there is (30) reads what no max_depth reads.
    let deepest = helper.request("get_ui_tree", json!({ "window_id": window_id, "max_depth": 30 }));
    assert_eq!(deepest["ok"], true, "{deepest}");
    assert!(deepest["result"].get("truncated").is_none(), "{}", deepest["result"]);
    let outline = |tree: &Value| tree["elements"].as_array().expect("elements").iter().map(|element| (element["depth"].clone(), element["role"].clone(), element["name"].clone())).collect::<Vec<_>>();
    assert_eq!(outline(&deepest["result"]), outline(&plain["result"]));

    // Outside 1 to 30, or not a whole number, is refused before anything is read.
    for bad in [json!(0), json!(31), json!(-1), json!(1.5), json!("3"), json!(null)] {
        let refused = helper.request("get_ui_tree", json!({ "window_id": window_id, "max_depth": bad.clone() }));
        assert_eq!(refused["error"]["code"], "invalid_params", "{bad}: {refused}");
    }
    let refused = helper.request("get_ui_tree", json!({ "window_id": window_id, "interactive_only": "yes" }));
    assert_eq!(refused["error"]["code"], "invalid_params", "{refused}");
}

/// Reference-gaps G3: `interactive_only` keeps the numbered elements, numbered as the whole tree
/// numbers them, and the lines above them; it works with `query` and `max_depth`, and a number of
/// the tree it returns clicks.
#[test]
fn interactive_only_keeps_numbered_elements_and_their_ancestors_and_its_numbers_click() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--ui-read-only"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let plain = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(plain["ok"], true, "{plain}");
    assert!(plain["result"].get("truncated").is_none(), "the plain tree is whole, so the two can be compared: {}", plain["result"]);
    let plain_elements = plain["result"]["elements"].as_array().expect("elements");
    let container_depth = find(plain_elements, |element| element["name"] == "Many buttons")["depth"].as_u64().expect("depth");

    let read = helper.request("get_ui_tree", json!({ "window_id": window_id, "interactive_only": true }));
    assert_eq!(read["ok"], true, "{read}");
    let tree = &read["result"];
    assert_ne!(tree["tree_id"], plain["result"]["tree_id"], "{tree}");
    let elements = tree["elements"].as_array().expect("elements");
    assert!(elements.len() < plain_elements.len(), "labels were left out: {} lines of {}", elements.len(), plain_elements.len());
    // Every line is numbered or has a numbered line below it.
    for (at, element) in elements.iter().enumerate() {
        if element.get("index").is_some() {
            continue;
        }
        let depth = element["depth"].as_u64().expect("depth");
        let has_numbered_below = elements[at + 1..].iter().take_while(|next| next["depth"].as_u64() > Some(depth)).any(|next| next.get("index").is_some());
        assert!(has_numbered_below, "a line with nothing numbered below it: {element}");
    }
    // The same elements are numbered, with the same numbers, as in the whole tree.
    let numbered = |list: &[Value]| list.iter().filter(|element| element.get("index").is_some()).map(|element| (element["index"].clone(), element["role"].clone(), element["name"].clone())).collect::<Vec<_>>();
    assert_eq!(numbered(elements), numbered(plain_elements), "{tree}");
    // The 60-child limit is the same: ten buttons are counted on the container.
    assert_eq!(find(elements, |element| element["name"] == "Many buttons")["omitted_children"], 10, "{tree}");

    // With a query: only a numbered element that matches, and its ancestors. A label that matches is not one.
    let found = helper.request("get_ui_tree", json!({ "window_id": window_id, "interactive_only": true, "query": "b65", "max_depth": container_depth + 2 }));
    assert_eq!(found["ok"], true, "{found}");
    let found_tree = &found["result"];
    assert!(found_tree.get("truncated").is_none(), "{found_tree}");
    let names: Vec<&str> = found_tree["elements"].as_array().expect("elements").iter().filter_map(|element| element["name"].as_str()).collect();
    assert_eq!(names, ["Many buttons", "B65"], "{found_tree}");
    let label_only = helper.request("get_ui_tree", json!({ "window_id": window_id, "interactive_only": true, "query": "Many buttons" }));
    assert_eq!(label_only["result"]["elements"], json!([]), "{label_only}");

    // The returned tree is the window's own tree: its number acts.
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the click must fail rather than skip: {activated}");
    let clicked = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": found_tree["tree_id"], "index": 1 } }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    std::thread::sleep(Duration::from_millis(150));
    let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
    assert!(title.contains("many:B65"), "the click by number did not reach B65: {title}");
}

/// Reference-gaps G3 (ticket 06): a subtree is the root at depth 0 and what is below it, nothing
/// of the rest of the window; `max_depth` counts from the root; the tree has its own id and
/// describes its root; its numbers click; it works with `interactive_only` and a `query`.
#[test]
fn a_subtree_starts_at_its_root_and_its_numbers_click() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--ui-read-only"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let plain = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(plain["ok"], true, "{plain}");
    let plain_id = plain["result"]["tree_id"].clone();
    let plain_elements = plain["result"]["elements"].as_array().expect("elements");
    let fruits = find(plain_elements, |element| element["role"] == "tree item" && element["name"] == "Fruits");
    let fruits_index = fruits["index"].clone();
    assert!(fruits_index.as_u64().is_some(), "{fruits}");

    let read = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": fruits_index, "tree_id": plain_id }));
    assert_eq!(read["ok"], true, "{read}");
    let tree = &read["result"];
    assert_ne!(tree["tree_id"], plain_id, "{tree}");
    assert_eq!(tree["root"], json!({ "role": "tree item", "name": "Fruits" }), "{tree}");
    let elements = tree["elements"].as_array().expect("elements");
    // The root is the first line, at depth 0 and numbered 1; everything else is below it.
    assert_eq!(elements[0]["name"], "Fruits", "{tree}");
    assert_eq!((elements[0]["depth"].as_u64(), elements[0]["index"].as_u64()), (Some(0), Some(1)), "{tree}");
    assert!(elements[1..].iter().all(|element| element["depth"].as_u64().is_some_and(|depth| depth >= 1)), "{tree}");
    let names: Vec<&str> = elements.iter().filter_map(|element| element["name"].as_str()).collect();
    assert_eq!(names, ["Fruits", "Apple", "Banana"], "{tree}");
    assert!(tree.get("truncated").is_none(), "{tree}");

    // A subtree is a tree like any: a root can be chosen in it, and number 1 of it is Fruits itself.
    let latest = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": 1, "tree_id": tree["tree_id"] }));
    assert_eq!(latest["ok"], true, "{latest}");
    assert_eq!(latest["result"]["root"]["name"], "Fruits", "{latest}");

    // max_depth counts from the root: 1 is the root alone, with its children counted; 2 reaches them.
    let one = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": fruits_index, "tree_id": plain_id, "max_depth": 1 }));
    assert_eq!(one["ok"], true, "{one}");
    let one_elements = one["result"]["elements"].as_array().expect("elements");
    assert_eq!(one_elements.len(), 1, "{}", one["result"]);
    assert!(one_elements[0]["omitted_children"].as_u64().is_some_and(|count| count >= 2), "{}", one["result"]);
    assert_eq!(one["result"]["truncated"], json!(["depth"]), "{}", one["result"]);
    let two = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": fruits_index, "tree_id": plain_id, "max_depth": 2 }));
    assert_eq!(two["result"]["elements"].as_array().map(Vec::len), Some(3), "{}", two["result"]);
    assert!(two["result"].get("truncated").is_none(), "{}", two["result"]);

    // With interactive_only and a query: the root is the ancestor of what is kept.
    let found = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": fruits_index, "tree_id": plain_id, "interactive_only": true, "query": "banana" }));
    assert_eq!(found["ok"], true, "{found}");
    let found_names: Vec<&str> = found["result"]["elements"].as_array().expect("elements").iter().filter_map(|element| element["name"].as_str()).collect();
    assert_eq!(found_names, ["Fruits", "Banana"], "{}", found["result"]);

    // The subtree is the window's own tree: its number acts. Banana is the third line, numbered 3.
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the click must fail rather than skip: {activated}");
    // A fresh tree to root it in: the reads above have pushed `plain_id` out of the five a window keeps.
    let fresh = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(fresh["ok"], true, "{fresh}");
    let fresh_fruits = find(fresh["result"]["elements"].as_array().expect("elements"), |element| element["role"] == "tree item" && element["name"] == "Fruits")["index"].clone();
    let subtree = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": fresh_fruits, "tree_id": fresh["result"]["tree_id"] }));
    assert_eq!(subtree["ok"], true, "{subtree}");
    let banana = find(subtree["result"]["elements"].as_array().expect("elements"), |element| element["name"] == "Banana")["index"].clone();
    let clicked = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": subtree["result"]["tree_id"], "index": banana } }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let after = helper.request("get_ui_tree", json!({ "window_id": window_id }));
        let selected = after["result"]["elements"].as_array().expect("elements").iter().any(|element| element["name"] == "Banana" && has(element, "states", "selected"));
        if selected {
            break;
        }
        assert!(Instant::now() < deadline, "the click by number did not reach Banana: {after}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Reference-gaps G3 (ticket 06): the target's popups are not part of a subtree. With the fixture's
/// menu open, a root in the window reads without a menu item; a root in the menu reads that item
/// and not its siblings.
#[test]
fn a_subtree_leaves_out_the_popups_of_the_window() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--popup-menu"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let opened = helper.request("click", json!({ "window_id": window_id, "x": width * 300 / 480, "y": height * 340 / 360, "button": "right" }));
    assert_eq!(opened["ok"], true, "{opened}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let title = helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string();
        if title.contains("menulast:") {
            break;
        }
        assert!(Instant::now() < deadline, "the menu did not open: {title}");
        std::thread::sleep(Duration::from_millis(50));
    }

    let plain = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(plain["ok"], true, "{plain}");
    let plain_id = plain["result"]["tree_id"].clone();
    let plain_elements = plain["result"]["elements"].as_array().expect("elements");
    assert_eq!(plain_elements.iter().filter(|element| element["role"] == "menu item").count(), 8, "the whole tree has the menu: {}", plain["result"]);
    let button = find(plain_elements, |element| element["role"] == "button")["index"].clone();
    let third = find(plain_elements, |element| element["name"] == "Menu item 3")["index"].clone();

    let from_window = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": button, "tree_id": plain_id }));
    let from_menu = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": third, "tree_id": plain_id }));
    let _ = helper.request("press_key", json!({ "window_id": window_id, "keys": ["escape"] }));
    assert_eq!(from_window["ok"], true, "{from_window}");
    assert!(!from_window.to_string().contains("Menu item"), "the menu is in a subtree of the window: {from_window}");
    assert_eq!(from_menu["ok"], true, "{from_menu}");
    let names: Vec<&str> = from_menu["result"]["elements"].as_array().expect("elements").iter().filter_map(|element| element["name"].as_str()).collect();
    assert_eq!(names, ["Menu item 3"], "{}", from_menu["result"]);
}

/// Reference-gaps G3 (ticket 06): a root that is gone is `element_not_found`, as for any element
/// tool, and issues no tree; a tree the helper does not have is `no_ui_tree`; an index past the
/// tree, another window's tree and parameters of the wrong kind are `invalid_params`.
#[test]
fn a_stale_root_is_element_not_found_and_a_bad_root_is_refused() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--ui-section").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    let first = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    assert_eq!(first["ok"], true, "{first}");
    let tree_id = first["result"]["tree_id"].clone();
    let elements = first["result"]["elements"].as_array().expect("elements");
    let close = find(elements, |element| element["role"] == "button" && element["name"] == "Close section")["index"].clone();
    let count = elements.iter().filter(|element| element.get("index").is_some()).count();

    // Alive, the button is a root of its own.
    let alive = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": close, "tree_id": tree_id }));
    assert_eq!(alive["ok"], true, "{alive}");
    assert_eq!(alive["result"]["root"], json!({ "role": "button", "name": "Close section" }), "{alive}");

    let clicked = helper.request("click", json!({ "window_id": window_id, "element": { "tree_id": tree_id, "index": close } }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !helper.request("get_ui_tree", json!({ "window_id": window_id }))["result"]["elements"].as_array().expect("elements").iter().all(|element| element["name"] != "Close section") {
        assert!(Instant::now() < deadline, "the section did not close");
        std::thread::sleep(Duration::from_millis(50));
    }
    let stale = helper.request("get_ui_tree", json!({ "window_id": window_id, "root_element_index": close, "tree_id": tree_id }));
    assert_eq!(stale["ok"], false, "{stale}");
    assert_eq!(stale["error"]["code"], "element_not_found", "{stale}");
    assert!(stale["error"]["message"].as_str().unwrap_or("").contains("new tree"), "{stale}");

    let refusals = [
        (json!({ "root_element_index": 1, "tree_id": "t999-zzzzz" }), "no_ui_tree"),
        (json!({ "root_element_index": count + 1, "tree_id": tree_id }), "invalid_params"),
        (json!({ "root_element_index": 0, "tree_id": tree_id }), "invalid_params"),
        (json!({ "root_element_index": 1.5, "tree_id": tree_id }), "invalid_params"),
        (json!({ "root_element_index": "1", "tree_id": tree_id }), "invalid_params"),
        (json!({ "root_element_index": 1 }), "invalid_params"),
        (json!({ "root_element_index": 1, "tree_id": "" }), "invalid_params"),
        (json!({ "tree_id": tree_id }), "invalid_params"),
    ];
    for (extra, code) in refusals {
        let mut params = json!({ "window_id": window_id });
        params.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
        let refused = helper.request("get_ui_tree", params);
        assert_eq!(refused["error"]["code"], code, "{extra}: {refused}");
    }
}

fn read_timed(helper: &mut Helper, window_id: &str) -> (Value, Duration) {
    let started = Instant::now();
    let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
    let took = started.elapsed();
    assert_eq!(read["ok"], true, "{read}");
    (read["result"].clone(), took)
}

fn find<'a>(elements: &'a [Value], matches: impl Fn(&Value) -> bool) -> &'a Value {
    elements.iter().find(|element| matches(element)).unwrap_or_else(|| panic!("no such element in {}", Value::from(elements.to_vec())))
}

fn has(element: &Value, list: &str, item: &str) -> bool {
    element[list].as_array().is_some_and(|items| items.iter().any(|value| value == item))
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
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
