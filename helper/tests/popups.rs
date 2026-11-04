use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::core::w;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetAncestor, GetForegroundWindow, GetWindowRect, IsWindowVisible, WindowFromPoint, GA_ROOT};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The color of canvas pixels that neither the target nor one of its popups covers.
const FILL: [u8; 3] = [0, 0, 0];

/// The fixture opens a classic `#32768` menu (`TrackPopupMenu`, unowned, topmost tool window of
/// its own thread) that sticks out 60 pixels past its left edge and well past its bottom edge,
/// like Explorer's context menu. The screenshot shows the menu next to the window on one canvas
/// that starts left of the window (negative window coordinates); pointer input at the menu,
/// inside or outside the window, is not `target_obscured`; and a click on the last item, which
/// lies outside the window, picks that command.
#[test]
fn the_targets_own_menu_is_captured_hovered_and_clicked_even_outside_the_window() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--popup-menu").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));

    // A right-click on the fixture's background (below the Dialog button) opens the menu.
    expect_foreground(&main, "before opening the menu");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 300 / 480, "y": height * 340 / 360, "button": "right" }));
    assert_eq!(opened["ok"], true, "{opened}");
    let shown = wait_for_title(&mut helper, &main, "menulast:");
    let last = numbers_after(&shown, "menulast:");
    let menu_window: isize = numbers_after(&shown, "menuwin:")[0] as isize;
    let menu = HWND(menu_window as *mut core::ffi::c_void);
    assert!(last[1] >= top + height, "the fixture's last menu item must lie below the window: {shown}");
    // Let the menu's fade-in finish before it is captured.
    std::thread::sleep(Duration::from_millis(400));
    assert!(unsafe { IsWindowVisible(menu) }.as_bool(), "the menu closed: {}", title(&mut helper, &main));

    // The screenshot: one canvas over the window and the menu, its origin left of the window.
    let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": 4000, "target_height": 4000 }));
    assert_eq!(shot["ok"], true, "{shot}");
    let result = &shot["result"];
    let popups = result["popups"].as_array().cloned().unwrap_or_default();
    assert_eq!(popups.len(), 1, "the open menu must be in the screenshot: {}", without_image(result));
    let popup = &popups[0];
    assert_eq!(popup["kind"], "menu", "{}", without_image(result));
    let (popup_x, popup_y) = (popup["x"].as_i64().expect("popup x"), popup["y"].as_i64().expect("popup y"));
    let (popup_width, popup_height) = (popup["width"].as_i64().expect("popup width"), popup["height"].as_i64().expect("popup height"));
    assert!(popup_x < 0 && popup_y < height && popup_y + popup_height > height, "the menu sticks out left and below: {}", without_image(result));
    assert_eq!(result["origin_x"].as_i64(), Some(popup_x), "{}", without_image(result));
    assert_eq!(result["origin_y"].as_i64(), Some(0), "{}", without_image(result));
    let (canvas_width, canvas_height) = (width - popup_x.min(0), (popup_y + popup_height).max(height));
    assert_eq!(result["original_width"].as_i64(), Some(canvas_width), "{}", without_image(result));
    assert_eq!(result["original_height"].as_i64(), Some(canvas_height), "{}", without_image(result));
    // The window's own size is unchanged: stale_screenshot compares it.
    assert_eq!(result["window_width"].as_i64(), Some(width), "{}", without_image(result));
    assert_eq!(result["window_height"].as_i64(), Some(height), "{}", without_image(result));
    assert!(result.get("popups_not_captured").is_none(), "{}", without_image(result));
    let bytes = base64::engine::general_purpose::STANDARD.decode(result["image_base64"].as_str().expect("image")).expect("base64");
    let image = image::load_from_memory(&bytes).expect("jpeg").to_rgb8();
    assert_eq!((image.width() as i64, image.height() as i64), (canvas_width, canvas_height));
    // Canvas pixel of window pixel (x, y).
    let canvas = |x: i64, y: i64| ((x - popup_x) as u32, y as u32);
    // Left of the window and above the menu: neither covers it, so it is the fill.
    let empty = mean(&image, canvas(popup_x + 4, 4), 8);
    assert!(close(empty, FILL, 16), "uncovered canvas must be the fill color, got {empty:?}");
    // The menu's last item, below the window and partly left of it: the menu's own pixels.
    let item = canvas(last[0] - left + 4, (last[1] + last[3]) / 2 - top - 4);
    let item_color = mean(&image, item, 8);
    assert!(!close(item_color, FILL, 24), "the menu's pixels below the window must be in the image, got {item_color:?}");

    // Activating the fixture with its menu open does not count the menu as covering it.
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "{activated}");
    assert!(activated["result"].get("covered_by").is_none(), "the fixture's own menu counted as covering it: {activated}");
    assert!(unsafe { IsWindowVisible(menu) }.as_bool(), "activation closed the menu: {}", title(&mut helper, &main));

    // Hovering the part of the menu over the window.
    expect_foreground(&main, "before hovering the menu");
    let hovered = helper.request("move_mouse", json!({ "window_id": main, "x": popup_x + popup_width - 10, "y": popup_y + 10 }));
    assert_eq!(hovered["ok"], true, "hovering the fixture's own menu: {hovered}");

    // Clicking the last item, below the window and left of it: a negative window x.
    let (x, y) = (last[0] - left + 6, (last[1] + last[3]) / 2 - top);
    assert!(x < 0 && y >= height, "the click must be outside the window: ({x}, {y})");
    expect_foreground(&main, "before clicking the menu item");
    let clicked = helper.request("click", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(clicked["ok"], true, "clicking the fixture's own menu item: {clicked}");
    let chosen = wait_for_title(&mut helper, &main, "menu:cmd:");
    assert!(chosen.contains("menu:cmd:108"), "the last item (108) was not chosen: {chosen}");

    // With the menu closed, a point outside the window is refused again and the screenshot is
    // the window alone.
    let outside = helper.request("move_mouse", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(outside["error"]["code"], "input_failed", "{outside}");
    let alone = helper.request("capture_window", json!({ "window_id": main, "target_width": width, "target_height": height }));
    assert_eq!(alone["ok"], true, "{alone}");
    assert_eq!(alone["result"]["original_width"].as_i64(), Some(width), "{}", without_image(&alone["result"]));
    assert!(alone["result"].get("popups").is_none() && alone["result"].get("origin_x").is_none(), "{}", without_image(&alone["result"]));
    drop(helper);
    drop(fixture);
}

/// Another process's unowned always-on-top tool window, shaped like a popup menu, over the
/// fixture's button: it is not the fixture's popup, so input there is still `target_obscured`.
#[test]
fn another_apps_tool_window_over_the_target_is_still_obscured() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    // Covers fixture pixels x 0..70, y 40..200: the left of the button (10..130 x 48..76).
    let cover_rect = format!("{},{},{},{}", left, top + height * 40 / 360, width * 70 / 480, height * 160 / 360);
    let cover_process =
        KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--cover", &cover_rect, "--tool-window"]).spawn().expect("cover"));
    // A tool window is not listed, so it is found by its class.
    let deadline = Instant::now() + Duration::from_secs(5);
    let cover = loop {
        let found = unsafe { FindWindowW(w!("CUMCP_COVER"), None) };
        if let Ok(found) = found {
            if unsafe { IsWindowVisible(found) }.as_bool() {
                break found;
            }
        }
        assert!(Instant::now() < deadline, "the covering tool window did not appear");
        std::thread::sleep(Duration::from_millis(50));
    };
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    expect_foreground(&main, "before clicking the covered part of the button");
    let refused = helper.request("click", json!({ "window_id": main, "x": width * 40 / 480, "y": height * 62 / 360 }));
    assert_eq!(refused["error"]["code"], "target_obscured", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!((cover.0 as isize).to_string()), "{refused}");
    expect_foreground(&main, "before moving onto the covered part of the button");
    let moved = helper.request("move_mouse", json!({ "window_id": main, "x": width * 40 / 480, "y": height * 62 / 360 }));
    assert_eq!(moved["error"]["code"], "target_obscured", "{moved}");
    assert!(!title(&mut helper, &main).contains("click:"), "refused input reached the fixture");
    drop(helper);
    drop(cover_process);
    drop(fixture);
}

/// The fixture's own unowned tool window lies behind it and sticks out past its left edge, like
/// a palette an app keeps under its main window. Only popups above the target join its
/// screenshot: this one is neither reported nor drawn over the fixture, the point where it sticks
/// out is outside the target, and a point where the fixture covers it reaches the fixture.
#[test]
fn the_targets_own_tool_window_behind_it_is_not_captured_or_targeted() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--tool-window-behind").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let tool = wait_for_class(w!("CUMCP_BEHIND"));
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let mut rect = RECT::default();
    unsafe { GetWindowRect(tool, &mut rect) }.expect("tool window rect");
    let (tool_left, tool_right) = (i64::from(rect.left), i64::from(rect.right));
    let tool_middle = i64::from(rect.top + rect.bottom) / 2;
    assert!(tool_left < left && tool_right > left + 20, "the tool window must stick out past the fixture's left edge and lie under it: {rect:?}");

    let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    let result = &shot["result"];
    assert!(result.get("popups").is_none() && result.get("origin_x").is_none(), "a tool window behind the target joined its screenshot: {}", without_image(result));
    assert_eq!(result["original_width"].as_i64(), Some(width), "{}", without_image(result));
    assert_eq!(result["original_height"].as_i64(), Some(height), "{}", without_image(result));

    // Where it sticks out, left of the fixture: outside the target.
    let (x, y) = (tool_left - left + 10, tool_middle - top);
    expect_foreground(&main, "before moving onto the tool window outside the fixture");
    let outside = helper.request("move_mouse", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(outside["error"]["code"], "input_failed", "{outside}");
    // Where the fixture covers it: the fixture's own scroll area.
    let (x, y) = (tool_right - left - 20, tool_middle - top);
    expect_foreground(&main, "before moving onto the fixture over the tool window");
    let inside = helper.request("move_mouse", json!({ "window_id": main, "x": x, "y": y }));
    assert_eq!(inside["ok"], true, "{inside}");
    wait_for_title(&mut helper, &main, &format!("hover:{x},{y}"));
    drop(helper);
    drop(fixture);
}

/// Ticket 51 (F16): the fixture's own flyout, an unowned tool window that is not always on top
/// (like the menus and flyouts apps draw themselves), opened above the fixture and sticking out
/// past its left edge. Raising the fixture before input keeps it above the fixture: a click where
/// it sticks out reaches it instead of being `target_obscured` by the fixture's own app;
/// activating the fixture neither counts it as covering nor hides it, so the screenshot still
/// shows it; and a click on its part over the fixture reaches it, as the screenshot shows.
#[test]
fn the_targets_own_flyout_stays_above_it_through_activation_and_input() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--flyout").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    expect_foreground(&main, "before opening the flyout");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 300 / 480, "y": height * 340 / 360, "button": "right" }));
    assert_eq!(opened["ok"], true, "{opened}");
    wait_for_title(&mut helper, &main, "flyout:open");
    wait_for_class(w!("CUMCP_FLYOUT"));
    // Its place in window pixels, as the screenshot shows it (this test process is not DPI aware).
    let flyout = |helper: &mut Helper, when: &str| {
        let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": 4000, "target_height": 4000 }));
        assert_eq!(shot["ok"], true, "{shot}");
        let popups = shot["result"]["popups"].as_array().cloned().unwrap_or_default();
        let popup = popups.iter().find(|popup| popup["kind"] == "popup").unwrap_or_else(|| panic!("{when}, the flyout is not in the screenshot: {}", without_image(&shot["result"])));
        ["x", "y", "width", "height"].map(|key| popup[key].as_i64().expect("popup rect"))
    };
    let [x, y, popup_width, popup_height] = flyout(&mut helper, "once opened");
    assert!(x < 0 && x + popup_width > 20, "the flyout must stick out past the fixture's left edge and overlap it: {x}, {popup_width}");

    // Where it sticks out, left of the fixture.
    expect_foreground(&main, "before clicking the flyout outside the fixture");
    let outside = helper.request("click", json!({ "window_id": main, "x": x + 10, "y": y + popup_height / 2 }));
    assert_eq!(outside["ok"], true, "clicking the fixture's own flyout outside the fixture: {outside}");
    assert!(outside["result"].get("foreground_changed").is_none(), "{outside}");
    wait_for_title(&mut helper, &main, "flyout:down");

    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "{activated}");
    assert!(activated["result"].get("covered_by").is_none(), "the fixture's own flyout counted as covering it: {activated}");
    assert_eq!(flyout(&mut helper, "after activation"), [x, y, popup_width, popup_height]);

    // Where it lies over the fixture: the flyout, as the screenshot shows, not the fixture.
    expect_foreground(&main, "before clicking the flyout over the fixture");
    let inside = helper.request("click", json!({ "window_id": main, "x": x + popup_width - 20, "y": y + popup_height / 2 }));
    assert_eq!(inside["ok"], true, "clicking the fixture's own flyout over the fixture: {inside}");
    std::thread::sleep(Duration::from_millis(300));
    let events = title(&mut helper, &main);
    drop(helper);
    drop(fixture);
    assert_eq!(events.matches("flyout:down").count(), 2, "the click over the fixture did not reach the flyout: {events}");
}

/// The fixture's own always-on-top tool windows that show nothing, like an app's drag or helper
/// layer: a click-through one over the whole fixture and well past it, and one with alpha 0 over
/// its bottom-left corner. Neither joins the screenshot, which stays the fixture alone.
#[test]
fn the_targets_invisible_layers_are_not_captured() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--invisible-layers").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    wait_for_class(w!("CUMCP_CLICKTHROUGH"));
    wait_for_class(w!("CUMCP_ALPHA0"));
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    let result = &shot["result"];
    assert!(result.get("popups").is_none() && result.get("origin_x").is_none(), "an invisible layer joined the screenshot: {}", without_image(result));
    assert_eq!(result["original_width"].as_i64(), Some(width), "{}", without_image(result));
    assert_eq!(result["original_height"].as_i64(), Some(height), "{}", without_image(result));
    // Ticket 51 (F17): neither covers the fixture for activation or pointer input. Pointer input
    // passes through both (`WindowFromPoint` finds the fixture under them), as the screenshot,
    // which leaves them out, shows: a click on the button under the click-through layer reaches
    // the button, and a point under the alpha-0 layer is the fixture's.
    assert!(activated["result"].get("covered_by").is_none(), "an invisible layer counted as covering the fixture: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (left, top) = (info["result"]["x"].as_i64().expect("x"), info["result"]["y"].as_i64().expect("y"));
    let under_alpha0 = (width * 70 / 480, height * 350 / 360);
    assert_eq!(root_at(left + under_alpha0.0, top + under_alpha0.1), hwnd(&main), "the fixture is not under its alpha-0 layer's point");
    expect_foreground(&main, "before moving under the alpha-0 layer");
    let moved = helper.request("move_mouse", json!({ "window_id": main, "x": under_alpha0.0, "y": under_alpha0.1 }));
    assert_eq!(moved["ok"], true, "{moved}");
    expect_foreground(&main, "before clicking the button under the click-through layer");
    let clicked = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 62 / 360 }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    wait_for_title(&mut helper, &main, "click:left");
    drop(helper);
    drop(fixture);
}

/// Ticket 51 (F17): the fixture's own unowned, always-on-top tooltip (`tooltips_class32`) sticks
/// out past its top edge. The screenshot's canvas starts above the window (a negative
/// `origin_y`), the tooltip is named `tooltip`, it does not count as covering the fixture, and
/// pointer input at its part above the window (a negative window y) reaches it.
#[test]
fn the_targets_own_tooltip_above_its_top_edge_is_captured_and_hovered() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--tooltip").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    assert!(activated["result"].get("covered_by").is_none(), "the fixture's own tooltip counted as covering it: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": 4000, "target_height": 4000 }));
    assert_eq!(shot["ok"], true, "{shot}");
    let result = &shot["result"];
    let popups = result["popups"].as_array().cloned().unwrap_or_default();
    assert_eq!(popups.len(), 1, "the tooltip must be in the screenshot: {}", without_image(result));
    assert_eq!(popups[0]["kind"], "tooltip", "{}", without_image(result));
    let [x, y, popup_width, popup_height] = ["x", "y", "width", "height"].map(|key| popups[0][key].as_i64().expect("popup rect"));
    assert!(y < 0 && y + popup_height > 0 && x >= 0 && x + popup_width <= width, "the tooltip sticks out above the window only: {}", without_image(result));
    assert_eq!(result["origin_x"].as_i64(), Some(0), "{}", without_image(result));
    assert_eq!(result["origin_y"].as_i64(), Some(y), "{}", without_image(result));
    assert_eq!(result["original_width"].as_i64(), Some(width), "{}", without_image(result));
    assert_eq!(result["original_height"].as_i64(), Some(height - y), "{}", without_image(result));
    assert_eq!(result["window_height"].as_i64(), Some(height), "{}", without_image(result));
    assert!(result.get("popups_not_captured").is_none(), "{}", without_image(result));

    expect_foreground(&main, "before moving onto the tooltip above the fixture");
    let moved = helper.request("move_mouse", json!({ "window_id": main, "x": x + 20, "y": y + 10 }));
    assert_eq!(moved["ok"], true, "moving onto the fixture's own tooltip above it: {moved}");
    wait_for_title(&mut helper, &main, "tooltip:move");
    drop(helper);
    drop(fixture);
}

/// Ticket 51 (F17): the drop-down list (`ComboLBox`) of the fixture's own combo box sticks out
/// past its bottom edge. The screenshot names it `dropdown`, and a click on an item below the
/// window chooses that item.
#[test]
fn the_targets_own_dropdown_list_below_it_is_captured_and_clicked() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--combo").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    expect_foreground(&main, "before opening the combo box");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 405 / 480, "y": height * 310 / 360 }));
    assert_eq!(opened["ok"], true, "{opened}");
    wait_for_title(&mut helper, &main, "combo:open");
    // Let the list's slide-in finish before it is captured.
    std::thread::sleep(Duration::from_millis(400));
    let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": 4000, "target_height": 4000 }));
    assert_eq!(shot["ok"], true, "{shot}");
    let result = &shot["result"];
    let popups = result["popups"].as_array().cloned().unwrap_or_default();
    let list = popups.iter().find(|popup| popup["kind"] == "dropdown").unwrap_or_else(|| panic!("the open list must be in the screenshot: {}", without_image(result)));
    let [x, y, list_width, list_height] = ["x", "y", "width", "height"].map(|key| list[key].as_i64().expect("popup rect"));
    assert!(y < height && y + list_height > height + 20, "the list must stick out below the window: {}", without_image(result));
    assert_eq!(result["original_height"].as_i64(), Some(y + list_height), "{}", without_image(result));

    let (item_x, item_y) = (x + list_width / 3, y + list_height - 10);
    expect_foreground(&main, "before clicking the list's item below the fixture");
    let clicked = helper.request("click", json!({ "window_id": main, "x": item_x, "y": item_y }));
    assert_eq!(clicked["ok"], true, "clicking the fixture's own list below it: {clicked}");
    let chosen = wait_for_title(&mut helper, &main, "combo:sel:");
    drop(helper);
    drop(fixture);
    assert!(!chosen.contains("combo:sel:0"), "the first item, not one below the window, was chosen: {chosen}");
}

/// Ticket 51 (F17), counterexample: an unowned, always-on-top tool window of another thread of
/// the fixture's own process over its button is not one of its popups (they are the target's UI
/// thread's): activation names it in `covered_by`, the screenshot leaves it out, and a click
/// under it is `target_obscured` naming it, with nothing reaching the fixture.
#[test]
fn a_tool_window_of_another_thread_of_the_targets_process_is_not_its_popup() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--other-thread-tool-window").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let main = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    wait_for_class(w!("CUMCP_OTHER_THREAD"));
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let covered_by = activated["result"]["covered_by"].as_array().cloned().unwrap_or_default();
    assert_eq!(covered_by.len(), 1, "the other thread's tool window must cover the fixture: {activated}");
    let other = covered_by[0]["window_id"].as_str().expect("window_id").to_string();
    assert_eq!(covered_by[0]["topmost"], true, "{activated}");
    assert!(covered_by[0]["app"].as_str().is_some_and(|app| app.to_ascii_lowercase().ends_with("fixture-window.exe")), "{activated}");
    let info = helper.request("get_window", json!({ "window_id": main }));
    let (width, height) = (info["result"]["width"].as_i64().expect("width"), info["result"]["height"].as_i64().expect("height"));
    let shot = helper.request("capture_window", json!({ "window_id": main, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    assert!(shot["result"].get("popups").is_none(), "another thread's window joined the screenshot: {}", without_image(&shot["result"]));
    expect_foreground(&main, "before clicking under the other thread's tool window");
    let refused = helper.request("click", json!({ "window_id": main, "x": width * 40 / 480, "y": height * 62 / 360 }));
    assert_eq!(refused["error"]["code"], "target_obscured", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], other.as_str(), "{refused}");
    std::thread::sleep(Duration::from_millis(300));
    let events = title(&mut helper, &main);
    drop(helper);
    drop(fixture);
    assert!(!events.contains("click:"), "refused input reached the fixture: {events}");
}

/// The root window at screen point (x, y) in physical pixels, as the helper (per-monitor DPI
/// aware) sees it; this test thread is made DPI aware just for the lookup.
fn root_at(x: i64, y: i64) -> HWND {
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let root = GetAncestor(WindowFromPoint(POINT { x: x as i32, y: y as i32 }), GA_ROOT);
        SetThreadDpiAwarenessContext(previous);
        root
    }
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

/// The visible top-level window of `class`, waiting for it: tool windows are not listed.
fn wait_for_class(class: windows::core::PCWSTR) -> HWND {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(found) = unsafe { FindWindowW(class, None) } {
            if unsafe { IsWindowVisible(found) }.as_bool() {
                return found;
            }
        }
        assert!(Instant::now() < deadline, "no visible window of the fixture's class appeared");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Mean color of the `size`-pixel square at `(x, y)`.
fn mean(image: &image::RgbImage, (x, y): (u32, u32), size: u32) -> [u8; 3] {
    let mut sum = [0u64; 3];
    for dy in 0..size {
        for dx in 0..size {
            let pixel = image.get_pixel(x + dx, y + dy).0;
            for channel in 0..3 {
                sum[channel] += pixel[channel] as u64;
            }
        }
    }
    sum.map(|total| (total / (size * size) as u64) as u8)
}

fn close(color: [u8; 3], expected: [u8; 3], tolerance: u8) -> bool {
    color.iter().zip(expected).all(|(value, expected)| value.abs_diff(expected) <= tolerance)
}

fn without_image(result: &Value) -> Value {
    let mut copy = result.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("image_base64");
    }
    copy
}

/// The comma-separated whole numbers right after the last `marker` in `title`.
fn numbers_after(title: &str, marker: &str) -> Vec<i64> {
    let rest = title.rsplit(marker).next().unwrap_or("");
    rest.split(|character: char| character != '-' && !character.is_ascii_digit())
        .take_while(|part| !part.is_empty())
        .map(|part| part.parse().expect("number"))
        .collect()
}

fn wait_for_title(helper: &mut Helper, window_id: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let current = title(helper, window_id);
        if current.contains(needle) {
            return current;
        }
        assert!(Instant::now() < deadline, "{needle} never appeared in the fixture's title: {current}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Fails, without sending anything, unless the foreground window is `expected`.
fn expect_foreground(expected: &str, when: &str) {
    let foreground = unsafe { GetForegroundWindow() };
    let expected_hwnd = HWND(expected.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void);
    assert!(expected_hwnd == foreground, "{when}: the foreground window {:?} is not {expected}, so no input is sent", foreground.0);
}

fn title(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

/// The window id of the first listed window whose title starts with `prefix`.
fn wait_for_window(helper: &mut Helper, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_windows", json!({}));
        if let Some(found) = last["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with(prefix)).and_then(|_| Some(window["window_id"].as_str()?.to_string()))
            })
        }) {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{prefix} was not listed: {last}");
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
