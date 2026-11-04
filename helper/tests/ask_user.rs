use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW, GetClientRect, GetDlgItem, GetForegroundWindow, GetGUIThreadInfo,
    GetMessageW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostThreadMessageW, RegisterClassExW, SendMessageTimeoutW, SendMessageW,
    SetWindowPos, BM_GETSTATE, GUITHREADINFO, HWND_TOP, MSG, SWP_NOACTIVATE, SMTO_NORMAL, SWP_SHOWWINDOW, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_NCACTIVATE, WM_NULL, WM_QUIT,
    WNDCLASSEXW, WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The dialog's window class and control ids (`helper/src/ask.rs`).
const DIALOG_CLASS: &str = "DESKPASS_ASK_USER";
const TEXT_ID: i32 = 100;
const ALLOW_ID: i32 = 101;
const DENY_ID: i32 = 102;
const ALWAYS_ID: i32 = 103;
const CONTINUE_TEXT: &str = "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?";

/// The dialog names the app by name and path, takes the foreground, and while it is open every
/// request, input or not, is refused at once without running: the dialog is not listed, cannot be
/// described or targeted, and nothing reaches it. Enter alone answers deny (the default button).
#[test]
fn an_open_dialog_names_the_app_refuses_every_request_and_enter_denies() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send(
        "ask_user",
        json!({ "kind": "approve_app", "app_name": "CUMCP 測試程式", "app_path": "C:\\CUMCP\\cumcp-test.exe", "timeout_ms": 30_000 }),
    );
    let dialog = wait_for_dialog(helper.pid());
    let text = dialog_text(dialog);
    assert!(text.contains("CUMCP 測試程式"), "the dialog does not name the app: {text:?}");
    assert!(text.contains("C:\\CUMCP\\cumcp-test.exe"), "the dialog does not show the app path: {text:?}");
    wait_until(|| unsafe { GetForegroundWindow() } == dialog, "the dialog did not take the foreground");

    let id = window_id(dialog);
    let (x, y) = button_offset(dialog);
    let attempts = [
        ("list_windows", json!({})),
        ("get_window", json!({ "window_id": id })),
        ("activate_window", json!({ "window_id": id })),
        ("click", json!({ "window_id": id, "x": x, "y": y })),
        ("type_text", json!({ "window_id": id, "text": "\n" })),
        ("press_key", json!({ "window_id": id, "keys": ["return"] })),
        ("scroll", json!({ "window_id": id, "x": x, "y": y, "direction": "down", "amount": 1 })),
        ("drag", json!({ "window_id": id, "from_x": x, "from_y": y, "to_x": x, "to_y": y })),
        ("move_mouse", json!({ "window_id": id, "x": x, "y": y })),
        ("capture_window", json!({ "window_id": id, "target_width": 100, "target_height": 100 })),
        ("ask_user", json!({ "kind": "continue", "timeout_ms": 5_000 })),
    ];
    for (method, params) in attempts {
        let started = Instant::now();
        let sent = helper.send(method, params);
        let response = helper.wait(sent, Duration::from_secs(3));
        assert_eq!(response["ok"], false, "{method} ran while the dialog was open: {response}");
        assert_eq!(response["error"]["code"], "prompt_open", "{method}: {response}");
        assert!(started.elapsed() < Duration::from_secs(2), "{method} was not refused at once ({:?})", started.elapsed());
        assert!(unsafe { IsWindowVisible(dialog) }.as_bool(), "the dialog closed after {method}");
        assert!(!helper.answered(asked), "the dialog was answered by {method}");
    }

    let pressed = answer(helper.pid(), "enter");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "Enter alone must not allow: {response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the dialog is still open after it was answered");

    let listed = helper.request("list_windows", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    let helper_windows: Vec<&Value> = listed["result"]["windows"]
        .as_array()
        .expect("windows")
        .iter()
        .filter(|window| window["app"].as_str().is_some_and(|app| app.to_ascii_lowercase().ends_with("deskpass-helper.exe")))
        .collect();
    assert!(helper_windows.is_empty(), "the helper's own windows are listed: {helper_windows:?}");
    let described = helper.request("get_window", json!({ "window_id": id }));
    assert_eq!(described["error"]["code"], "window_not_found", "{described}");
}

/// Clicking allow answers allow; clicking deny answers deny. The continue question uses the
/// spec's wording. Each dialog is gone once answered.
#[test]
fn the_buttons_answer_allow_and_deny() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();

    let asked = helper.send("ask_user", json!({ "kind": "continue", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let text = dialog_text(dialog);
    assert!(text.contains(CONTINUE_TEXT), "the continue question is not the spec's: {text:?}");
    answer(helper.pid(), "allow");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "allow", "{response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the answered dialog is still open");

    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the answered dialog is still open");
}

/// Ticket 95: "Always allow" is there only when the question asks for it (`allow_always`); the allow
/// button is "Allow once" either way. With three buttons the refusal is still the default with the
/// focus, so Enter alone refuses, and a click on "Always allow" answers `allow_always`.
#[test]
fn the_always_allow_button_is_shown_only_when_asked_for_and_answers_allow_always() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let approve = |allow_always: bool| json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000, "allow_always": allow_always });

    for params in [approve(false), json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 })] {
        let asked = helper.send("ask_user", params.clone());
        let dialog = wait_for_dialog(helper.pid());
        assert!(unsafe { GetDlgItem(Some(dialog), ALWAYS_ID) }.is_err(), "永遠允許 shown though not asked for: {params}");
        assert_eq!(button_text(dialog, ALLOW_ID), "本次允許");
        answer(helper.pid(), "deny");
        assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "deny");
    }

    let asked = helper.send("ask_user", approve(true));
    let dialog = wait_for_dialog(helper.pid());
    assert_eq!(button_text(dialog, ALWAYS_ID), "永遠允許");
    assert_eq!(button_text(dialog, ALLOW_ID), "本次允許");
    assert_eq!(button_text(dialog, DENY_ID), "拒絕");
    let pressed = answer(helper.pid(), "enter");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "Enter alone must refuse with three buttons too: {response}");

    let asked = helper.send("ask_user", approve(true));
    wait_for_dialog(helper.pid());
    answer(helper.pid(), "alt-f4");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "closing the three-button dialog must refuse: {response}");

    let asked = helper.send("ask_user", approve(true));
    let dialog = wait_for_dialog(helper.pid());
    answer(helper.pid(), "always");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "allow_always", "{response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the answered dialog is still open");
}

/// Ticket 97: the clipboard question (kind `read_clipboard`) has two buttons only, "Deny" and
/// "Allow once", never "Always allow"; it says it is about the clipboard. The refusal is the default with
/// the focus (Enter refuses), Alt+F4 refuses, an early click on "Allow once" does not allow, and a
/// click once it is armed answers `allow`.
#[test]
fn the_clipboard_question_has_two_buttons_and_refuses_by_default() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let question = json!({ "kind": "read_clipboard", "timeout_ms": 30_000 });

    let asked = helper.send("ask_user", question.clone());
    let dialog = wait_for_dialog(helper.pid());
    assert!(dialog_text(dialog).contains("剪貼簿"), "the question does not name the clipboard: {:?}", dialog_text(dialog));
    assert!(unsafe { GetDlgItem(Some(dialog), ALWAYS_ID) }.is_err(), "永遠允許 shown on the clipboard question");
    assert_eq!(button_text(dialog, ALLOW_ID), "本次允許");
    assert_eq!(button_text(dialog, DENY_ID), "拒絕");
    let pressed = answer(helper.pid(), "enter");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "deny", "Enter alone must refuse");

    let asked = helper.send("ask_user", question.clone());
    wait_for_dialog(helper.pid());
    answer(helper.pid(), "alt-f4");
    assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "deny", "closing the clipboard question must refuse");

    let mut answerer = WaitingAnswerer::start(helper.pid(), "allow-early");
    let asked = helper.send("ask_user", question.clone());
    let dialog = wait_for_dialog(helper.pid());
    let early = answerer.result();
    assert_eq!(early["allow_enabled"], false, "the click was not early enough to test the arming delay: {early}");
    wait_until_armed(dialog);
    assert_unanswered(&mut helper, asked, dialog, "an early click on 本次允許");
    answer(helper.pid(), "allow");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "allow", "{response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the answered dialog is still open");
}

/// Ticket 95: "Always allow" is disarmed for the first 0.7 s like "Allow once": a click on it as soon as
/// the dialog appears does nothing, and the dialog stays open until a later answer.
#[test]
fn a_click_on_always_allow_right_after_the_dialog_appears_does_not_allow() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut answerer = WaitingAnswerer::start(helper.pid(), "always-early");
    let asked = helper.send(
        "ask_user",
        json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000, "allow_always": true }),
    );
    let dialog = wait_for_dialog(helper.pid());
    let early = answerer.result();
    assert_eq!(early["always_enabled"], false, "the click was not early enough to test the arming delay: {early}");
    wait_until_armed(dialog);
    assert_unanswered(&mut helper, asked, dialog, "an early click on 永遠允許");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 57: Alt+F4 (the window's close command) answers deny, and the dialog is gone.
#[test]
fn alt_f4_answers_deny() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let pressed = answer(helper.pid(), "alt-f4");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "Alt+F4 must not allow: {response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the dialog is still open after Alt+F4");
    assert_eq!(dialogs_of(helper.pid()), Vec::<isize>::new());
}

/// Ticket 57: Space with the default focus presses the focused button, which is the refusal.
#[test]
fn space_with_the_default_focus_answers_deny() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let pressed = answer(helper.pid(), "space");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "Space alone must not allow: {response}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the dialog is still open after Space");
}

/// Ticket 57: a click on allow while the button is still disarmed (the first 0.7 s, `ARM_DELAY` in
/// `helper/src/ask.rs`) does nothing: the dialog stays open and unanswered, and a later deny
/// still answers it. The answerer is already waiting when the question is sent, so the click
/// comes as soon as the dialog appears, however long the answerer process took to start.
/// Ticket 68: nor does it allow once the button is armed, even when the dialog only takes the
/// click from its queue after arming, so the check waits until well past the arming.
#[test]
fn a_click_on_allow_right_after_the_dialog_appears_does_not_allow() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut answerer = WaitingAnswerer::start(helper.pid(), "allow-early");
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let early = answerer.result();
    assert_eq!(early["allow_enabled"], false, "the click was not early enough to test the arming delay: {early}");
    wait_until_armed(dialog);
    assert_unanswered(&mut helper, asked, dialog, "an early click on allow");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 68: the same early click, made while the dialog's thread is held up past the arming,
/// still does not allow. The app in front when the question comes is slow to give up the
/// foreground (it takes 1.2 s over its deactivation), so the dialog only takes the click from its
/// queue after the allow button was enabled; before the fix that click allowed.
#[test]
fn an_early_click_the_dialog_takes_only_after_arming_does_not_allow() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let _slow = slow_app_in_front(&mut helper);
    let mut answerer = WaitingAnswerer::start_after(helper.pid(), "allow-early", HELD_CLICK_AFTER_MS);
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let appeared = Instant::now();
    let early = answerer.result();
    assert_eq!(early["allow_enabled"], false, "the click was not early enough to test the arming delay: {early}");
    assert_held_past_the_arming(dialog, appeared);
    wait_until_armed(dialog);
    assert_unanswered(&mut helper, asked, dialog, "an early click taken after the arming");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 68 review: a click on deny made while the dialog's thread is held up past the arming
/// leaves nothing half-pressed. The dialog arms before it takes that click from its queue, so the
/// stale press is dropped as a whole (it never pushes deny or takes the mouse capture) and the
/// release that follows does nothing; a later click on deny then answers deny. It also passed
/// before the review fix (the answerer's pointer move is dequeued first and arms the dialog, so
/// press and release were both dropped); it is kept as a regression guard, not as proof of the fix.
#[test]
fn an_early_click_on_deny_the_dialog_takes_only_after_arming_leaves_nothing_pressed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let _slow = slow_app_in_front(&mut helper);
    let mut answerer = WaitingAnswerer::start_after(helper.pid(), "deny-early", HELD_CLICK_AFTER_MS);
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let appeared = Instant::now();
    let early = answerer.result();
    assert_eq!(early["allow_enabled"], false, "the click was not early enough to cross the arming: {early}");
    assert_held_past_the_arming(dialog, appeared);
    wait_until_armed(dialog);
    if helper.answered(asked) {
        assert_eq!(helper.responses[&asked]["result"]["answer"], "deny", "{}", helper.responses[&asked]);
        return;
    }
    assert_eq!(pressed_state(dialog), (false, false), "(mouse capture held, deny pushed) after the dropped click");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 68 review: a press on deny made before the arming and released right at it (the release
/// stamped in the arming's tick) is never left stranded with deny pushed and holding the mouse
/// capture (which swallowed the next click): a release is never dropped, so the click answers
/// deny. (If the dialog's thread happened to be held up past the arming, the press is dropped
/// whole instead; then nothing is left pressed and a later click on deny answers deny.)
#[test]
fn a_press_on_deny_released_right_at_the_arming_is_not_left_stranded() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut answerer = WaitingAnswerer::start(helper.pid(), "deny-across-arm");
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let across = answerer.result();
    assert_eq!(across["allow_enabled_at_press"], false, "the press was not early enough to cross the arming: {across}");
    assert_eq!(across["allow_enabled"], true, "the release was not at the arming: {across}");
    wait_until_armed(dialog);
    if helper.answered(asked) {
        assert_eq!(helper.responses[&asked]["result"]["answer"], "deny", "{}", helper.responses[&asked]);
        return;
    }
    assert_eq!(pressed_state(dialog), (false, false), "(mouse capture held, deny pushed): the press on deny was left stranded");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 68 review: another program sending the dialog allow's click notification before the
/// arming does not allow (only `BN_CLICKED` after the arming counts).
#[test]
fn a_click_notification_for_allow_sent_before_the_arming_does_not_allow() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let allow = unsafe { GetDlgItem(Some(dialog), ALLOW_ID) }.expect("allow button");
    // BN_CLICKED is 0, so the notification's wParam is the control id alone.
    unsafe { SendMessageW(dialog, WM_COMMAND, Some(WPARAM(ALLOW_ID as usize)), Some(LPARAM(allow.0 as isize))) };
    assert!(!unsafe { IsWindowEnabled(allow) }.as_bool(), "the notification was handled after the arming, so it tests nothing");
    wait_until_armed(dialog);
    assert_unanswered(&mut helper, asked, dialog, "allow's click notification sent before the arming");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 95 review: "Always allow"'s click notification sent by another program to a dialog that was
/// not offered it answers nothing, even once armed.
#[test]
fn a_click_notification_for_always_allow_on_a_two_button_dialog_does_nothing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    wait_until_armed(dialog);
    assert!(unsafe { GetDlgItem(Some(dialog), ALWAYS_ID) }.is_err(), "the dialog was offered 永遠允許");
    // BN_CLICKED is 0, so the notification's wParam is the control id alone.
    unsafe { SendMessageW(dialog, WM_COMMAND, Some(WPARAM(ALWAYS_ID as usize)), Some(LPARAM(0))) };
    assert_unanswered(&mut helper, asked, dialog, "永遠允許's click notification on a two-button dialog");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// An app of the test's own put in front and made slow to give up the foreground: its next
/// deactivation takes 1.2 s. That held up the dialog's thread while it took the foreground as long
/// as the dialog ran on the request thread; on a thread of its own (ticket 82) it does not (measured:
/// the dialog was in front within a millisecond), so the next dialog's thread is held 1.2 s before
/// it takes the foreground with `debug_hold_dialog` (test builds only) instead.
fn slow_app_in_front(helper: &mut Helper) -> TestWindow {
    let held = helper.request("debug_hold_dialog", json!({ "ms": 1_200 }));
    assert_eq!(held["ok"], true, "{held}");
    let slow = TestWindow::show(
        "CUMCP_TEST_SLOW_FOREGROUND",
        WINDOW_EX_STYLE::default(),
        WS_OVERLAPPEDWINDOW,
        RECT { left: 50, top: 50, right: 350, bottom: 250 },
        None,
        Some(slow_to_deactivate_proc),
    );
    let activated = helper.request("activate_window", json!({ "window_id": slow.hwnd.to_string() }));
    assert_eq!(activated["ok"], true, "{activated}");
    wait_until(|| unsafe { GetForegroundWindow() }.0 as isize == slow.hwnd, "the slow app did not take the foreground");
    std::thread::sleep(Duration::from_millis(300));
    SLOW_DEACTIVATION.store(true, Ordering::SeqCst);
    slow
}

/// Fails unless the dialog's thread was held up past the arming: it first answers a sent message
/// once the hold is over, and that has to come well after `ARM_DELAY` (0.7 s) from the dialog's
/// appearing, or the test would pass without the hold. Measured on the dialog's thread itself:
/// with the dialog on a thread of its own (ticket 82) the early click hands it the foreground
/// while that thread is still held, so when the foreground came no longer shows the hold.
fn assert_held_past_the_arming(dialog: HWND, appeared: Instant) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while unsafe { SendMessageTimeoutW(dialog, WM_NULL, WPARAM(0), LPARAM(0), SMTO_NORMAL, 10, None) }.0 == 0 {
        assert!(Instant::now() < deadline, "the dialog's thread never answered");
    }
    let held = appeared.elapsed();
    wait_until(|| unsafe { GetForegroundWindow() } == dialog, "the dialog did not take the foreground");
    assert!(!SLOW_DEACTIVATION.load(Ordering::SeqCst), "the slow app was never asked to give up the foreground");
    assert!(held >= Duration::from_millis(900), "the dialog's thread was not held up past the arming: it answered after {held:?}");
}

/// How long after the dialog appears the held-up tests click: by then the dialog's thread is
/// held up, and it is still well before the arming.
const HELD_CLICK_AFTER_MS: u64 = 200;

/// Set once the slow app is in front; its next deactivation then takes 1.2 s.
static SLOW_DEACTIVATION: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn slow_to_deactivate_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCACTIVATE && wparam.0 == 0 && SLOW_DEACTIVATION.swap(false, Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1_200));
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

/// Ticket 68: pressing the mouse button on allow while it is disarmed and releasing it once it is
/// armed is not a click on allow: only a click whose press came after the arming counts. It also
/// passed before the fix (a button ignores a release it did not see pressed); it is kept as a
/// regression guard for that rule, not as proof of the fix.
#[test]
fn a_press_on_allow_before_it_is_armed_released_after_does_not_allow() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut answerer = WaitingAnswerer::start(helper.pid(), "allow-across-arm");
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    let across = answerer.result();
    assert_eq!(across["allow_enabled_at_press"], false, "the press was not early enough to test the arming delay: {across}");
    assert_eq!(across["allow_enabled"], true, "the release was not after the arming: {across}");
    wait_until_armed(dialog);
    assert_unanswered(&mut helper, asked, dialog, "a press before the arming released after it");

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Ticket 68: the dialog is excluded from screen capture (like the overlay), so a screenshot of
/// the screen where it is shows what is behind it. The test puts a magenta window of its own
/// behind the dialog and copies the dialog's client area from the screen.
#[test]
fn a_screen_capture_does_not_show_the_open_dialog() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    // Physical pixels on this thread (and the test window's), whatever the rest of the process uses.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    // Until then the app that had the foreground stays above any ordinary window the test shows.
    wait_until(|| unsafe { GetForegroundWindow() } == dialog, "the dialog did not take the foreground");
    let client = client_rect_on_screen(dialog);
    // Shown without activation at the top of the ordinary windows, right behind the topmost dialog.
    let backdrop = TestWindow::show(
        "CUMCP_TEST_BACKDROP",
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        WS_POPUP,
        client,
        Some(COLORREF(0x00FF_00FF)),
        Some(plain_proc),
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(unsafe { IsWindowVisible(dialog) }.as_bool(), "the dialog closed before the capture");
    let pixels = capture_screen(client);
    let magenta = pixels.iter().filter(|&&[r, g, b]| r > 230 && g < 25 && b > 230).count();
    let share = magenta as f64 / pixels.len().max(1) as f64;
    drop(backdrop);
    assert!(share > 0.95, "the capture shows the dialog: only {:.1}% of its client area is the window behind it", share * 100.0);

    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// An unanswered dialog closes itself when its time is up and answers `timeout`; nothing in the
/// request can answer it in advance.
#[test]
fn an_unanswered_dialog_times_out_and_closes() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let started = Instant::now();
    let asked = helper.send(
        "ask_user",
        json!({
            "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 1_500,
            "answer": "allow", "auto_answer": "allow", "skip": true, "default": "allow",
        }),
    );
    let dialog = wait_for_dialog(helper.pid());
    let response = helper.wait(asked, Duration::from_secs(10));
    let elapsed = started.elapsed();
    assert_eq!(response["result"]["answer"], "timeout", "{response}");
    assert!(elapsed >= Duration::from_millis(1_400) && elapsed < Duration::from_secs(4), "timed out after {elapsed:?}");
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool(), "the timed-out dialog is still open");
    assert_eq!(dialogs_of(helper.pid()), Vec::<isize>::new(), "a dialog window is left behind");
}

/// Ticket 86: a dialog whose thread is stuck (held 9 s with `debug_hold_dialog`, as a foreground
/// app that does not answer could) is not waited for past its own 1-second timeout plus the grace
/// (`DIALOG_GRACE` in `helper/src/ask.rs`, 5 s): the question ends `prompt_unavailable`, never
/// allow. Before the fix the request waited out the whole hold. The stuck dialog closes as soon as
/// its thread runs again, and a question asked while it is still stuck is answered normally.
#[test]
fn a_dialog_stuck_past_its_time_ends_unavailable_and_the_next_question_still_works() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let held = helper.request("debug_hold_dialog", json!({ "ms": 9_000 }));
    assert_eq!(held["ok"], true, "{held}");
    let started = Instant::now();
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 1_000 }));
    let stuck = wait_for_dialog(helper.pid());
    let response = helper.wait(asked, Duration::from_secs(8));
    let elapsed = started.elapsed();
    assert_eq!(response["ok"], false, "a stuck dialog answered: {response}");
    assert_eq!(response["error"]["code"], "prompt_unavailable", "{response}");
    assert!(elapsed >= Duration::from_millis(5_500), "given up before the timeout and grace were over ({elapsed:?})");

    // Asked while the stuck thread is still held.
    let next = helper.send("ask_user", json!({ "kind": "continue", "timeout_ms": 30_000 }));
    wait_until(|| !unsafe { IsWindow(Some(stuck)) }.as_bool(), "the stuck dialog stayed open once its thread ran again");
    let dialog = wait_for_dialog(helper.pid());
    assert_ne!(dialog, stuck);
    assert!(dialog_text(dialog).contains(CONTINUE_TEXT), "{:?}", dialog_text(dialog));
    answer(helper.pid(), "allow");
    let response = helper.wait(next, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "allow", "{response}");
    assert_eq!(dialogs_of(helper.pid()), Vec::<isize>::new(), "a dialog window is left behind");
}

/// Ticket 86 review: a dialog thread held up in the middle of taking the foreground (after it
/// attached to the foreground thread's input; `debug_hold_dialog` with `after_attach`) and given
/// up on meanwhile does not take the foreground once it runs again, so the next question's dialog
/// keeps it. Before the fix it brought itself in front on its way out.
#[test]
fn a_dialog_given_up_on_while_taking_the_foreground_leaves_it_to_the_next_question() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let foregrounds = ForegroundLog::start(helper.pid());
    let held = helper.request("debug_hold_dialog", json!({ "ms": 9_000, "after_attach": true }));
    assert_eq!(held["ok"], true, "{held}");
    let asked = helper.send("ask_user", json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 1_000 }));
    let stuck = wait_for_dialog(helper.pid());
    let response = helper.wait(asked, Duration::from_secs(8));
    assert_eq!(response["error"]["code"], "prompt_unavailable", "{response}");

    let next = helper.send("ask_user", json!({ "kind": "continue", "timeout_ms": 30_000 }));
    let dialog = wait_until_found(|| dialogs_of(helper.pid()).into_iter().map(|found| HWND(found as *mut core::ffi::c_void)).find(|&found| found != stuck), "the next question's dialog did not appear");
    wait_until(|| unsafe { GetForegroundWindow() } == dialog, "the next question's dialog did not take the foreground");
    assert!(unsafe { IsWindow(Some(stuck)) }.as_bool(), "the stuck dialog closed before the next question's was in front, so this tests nothing");
    // From here on: the stuck dialog may have been activated when it was shown, before the hold.
    foregrounds.clear();
    wait_until(|| !unsafe { IsWindow(Some(stuck)) }.as_bool(), "the stuck dialog stayed open once its thread ran again");
    std::thread::sleep(Duration::from_millis(500));
    let taken = foregrounds.stop();
    assert!(!taken.contains(&(stuck.0 as isize)), "the given-up dialog took the foreground once its thread ran again: {taken:?}");
    assert_eq!(unsafe { GetForegroundWindow() }, dialog, "the next question's dialog lost the foreground");

    answer(helper.pid(), "deny");
    let response = helper.wait(next, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "deny", "{response}");
}

/// Every window of one process that becomes the foreground window, as reported to a WinEvent hook
/// on a thread of its own.
struct ForegroundLog {
    thread: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

static FOREGROUNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

impl ForegroundLog {
    fn start(pid: u32) -> Self {
        FOREGROUNDS.lock().unwrap().clear();
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || unsafe {
            let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), pid, 0, WINEVENT_OUTOFCONTEXT);
            sender.send(GetCurrentThreadId()).expect("foreground log started");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                DispatchMessageW(&message);
            }
            let _ = UnhookWinEvent(hook);
        });
        let thread = receiver.recv_timeout(Duration::from_secs(5)).expect("foreground log started");
        Self { thread, handle: Some(handle) }
    }

    /// Forgets what was logged so far. Events already queued for the hook thread may still arrive;
    /// the caller clears only once what came before has long been delivered.
    fn clear(&self) {
        FOREGROUNDS.lock().unwrap().clear();
    }

    fn stop(mut self) -> Vec<isize> {
        self.end();
        FOREGROUNDS.lock().unwrap().clone()
    }

    fn end(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ForegroundLog {
    fn drop(&mut self) {
        self.end();
    }
}

unsafe extern "system" fn on_foreground(_hook: HWINEVENTHOOK, _event: u32, hwnd: HWND, _object: i32, _child: i32, _thread: u32, _time: u32) {
    FOREGROUNDS.lock().unwrap().push(hwnd.0 as isize);
}

fn wait_until_found<T>(mut find: impl FnMut() -> Option<T>, message: &str) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(found) = find() {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{message}");
}

/// A stop (the physical Esc; `debug_stop` in test builds) closes the dialog without allowing:
/// the request ends `interrupted`, like any other request a stop cuts short.
#[test]
fn a_stop_closes_the_dialog_without_allowing() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "continue", "timeout_ms": 30_000 }));
    let dialog = wait_for_dialog(helper.pid());
    helper.send("debug_stop", json!({}));
    // What the server sends when the stop ends its wait: it runs once the dialog has closed.
    let hidden = helper.send("overlay_hide", json!({}));
    let response = helper.wait(asked, Duration::from_secs(3));
    assert_eq!(response["error"]["code"], "interrupted", "{response}");
    let hidden = helper.wait(hidden, Duration::from_secs(3));
    assert_eq!(hidden["ok"], true, "overlay_hide sent as the dialog closed was refused: {hidden}");
    wait_until(|| !unsafe { IsWindow(Some(dialog)) }.as_bool(), "the dialog stayed open after the stop");
    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "the helper does not take requests after the dialog closed: {resumed}");
}

/// Ticket 56 (F4): a stop that comes after the `ask_user` line was read but before its dialog
/// appeared still counts: the request ends `interrupted` at once and no dialog is left open waiting
/// for an answer nobody will read. A request that arrives after the stop is told so, not
/// `prompt_open`.
#[test]
fn a_stop_right_after_the_question_was_sent_shows_no_dialog() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("ask_user", json!({ "kind": "continue", "timeout_ms": 30_000 }));
    let stopped = helper.send("debug_stop", json!({}));
    let response = helper.wait(asked, Duration::from_secs(3));
    assert_eq!(response["error"]["code"], "interrupted", "{response}");
    let stop_response = helper.wait(stopped, Duration::from_secs(3));
    assert_ne!(stop_response["error"]["code"], "prompt_open", "a request after the stop was refused as if the question were still open: {stop_response}");
    wait_until(|| dialogs_of(helper.pid()).is_empty(), "a dialog was left open after the stop");
}

#[test]
fn ask_user_refuses_bad_parameters_without_a_dialog() {
    let mut helper = Helper::spawn();
    let cases = [
        (json!({ "kind": "approve_app", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 5_000 }), "app_name"),
        (json!({ "kind": "approve_app", "app_name": "Demo", "timeout_ms": 5_000 }), "app_path"),
        (json!({ "kind": "format_disk", "timeout_ms": 5_000 }), "kind"),
        (json!({ "kind": "continue" }), "timeout_ms"),
        (json!({ "kind": "continue", "timeout_ms": 999 }), "timeout_ms"),
        (json!({ "kind": "continue", "timeout_ms": 120_001 }), "timeout_ms"),
        (json!({ "kind": "continue", "timeout_ms": "60000" }), "timeout_ms"),
        (json!({ "kind": "approve_app", "app_name": "Demo", "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 5_000, "allow_always": "yes" }), "allow_always"),
        (json!({ "kind": "continue", "timeout_ms": 5_000, "allow_always": true }), "allow_always"),
        (json!({ "kind": "read_clipboard", "timeout_ms": 5_000, "allow_always": true }), "allow_always"),
        (json!({ "kind": "read_clipboard" }), "timeout_ms"),
    ];
    for (params, named) in cases {
        let response = helper.request("ask_user", params.clone());
        assert_eq!(response["error"]["code"], "invalid_params", "{params}: {response}");
        let message = response["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains(named), "{params}: the error does not name {named}: {response}");
    }
    assert_eq!(dialogs_of(helper.pid()), Vec::<isize>::new());
}

fn answer(helper_pid: u32, press: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_prompt-answerer"))
        .args(["--pid", &helper_pid.to_string(), "--press", press, "--wait-ms", "10000"])
        .output()
        .expect("prompt-answerer");
    assert!(output.status.success(), "prompt-answerer --press {press} failed: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("prompt-answerer output")
}

/// A prompt-answerer started before the dialog exists, already looking for it.
struct WaitingAnswerer {
    child: Child,
    stdout: BufReader<ChildStdout>,
    press: String,
}

impl WaitingAnswerer {
    /// Returns once the answerer says it is looking for the helper's dialog.
    fn start(helper_pid: u32, press: &str) -> Self {
        Self::start_after(helper_pid, press, 0)
    }

    /// The same, acting `after_ms` after it finds the dialog.
    fn start_after(helper_pid: u32, press: &str, after_ms: u64) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_prompt-answerer"))
            .args(["--pid", &helper_pid.to_string(), "--press", press, "--wait-ms", "10000", "--after-ms", &after_ms.to_string(), "--announce"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("prompt-answerer");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut line = String::new();
        stdout.read_line(&mut line).expect("prompt-answerer output");
        assert_eq!(serde_json::from_str::<Value>(&line).ok(), Some(json!({ "waiting": true })), "prompt-answerer did not announce itself: {line:?}");
        Self { child, stdout, press: press.to_string() }
    }

    /// Its JSON line, once it has sent the input.
    fn result(&mut self) -> Value {
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).expect("prompt-answerer output");
        let status = self.child.wait().expect("prompt-answerer");
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        assert!(status.success(), "prompt-answerer --press {} failed: {stderr}", self.press);
        serde_json::from_str(rest.trim()).expect("prompt-answerer output")
    }
}

impl Drop for WaitingAnswerer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_dialog(pid: u32) -> HWND {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(&found) = dialogs_of(pid).first() {
            return HWND(found as *mut core::ffi::c_void);
        }
        assert!(Instant::now() < deadline, "no dialog appeared");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Visible dialog windows of the helper process.
fn dialogs_of(pid: u32) -> Vec<isize> {
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    all.into_iter()
        .filter(|hwnd| {
            let mut owner = 0u32;
            unsafe {
                GetWindowThreadProcessId(*hwnd, Some(&mut owner));
            }
            let mut class = [0u16; 128];
            let length = unsafe { GetClassNameW(*hwnd, &mut class) };
            owner == pid && String::from_utf16_lossy(&class[..length.max(0) as usize]) == DIALOG_CLASS && unsafe { IsWindowVisible(*hwnd) }.as_bool()
        })
        .map(|hwnd| hwnd.0 as isize)
        .collect()
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
    windows::Win32::Foundation::TRUE
}

fn dialog_text(dialog: HWND) -> String {
    let Ok(label) = (unsafe { GetDlgItem(Some(dialog), TEXT_ID) }) else { return String::new() };
    let mut buffer = [0u16; 2048];
    let length = unsafe { GetWindowTextW(label, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

/// A button's label; panics when the dialog has no such button.
fn button_text(dialog: HWND, id: i32) -> String {
    let button = unsafe { GetDlgItem(Some(dialog), id) }.unwrap_or_else(|_| panic!("button {id} missing"));
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(button, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn window_id(hwnd: HWND) -> String {
    (hwnd.0 as isize).to_string()
}

/// The allow button's middle, relative to the dialog's window rectangle.
fn button_offset(dialog: HWND) -> (i32, i32) {
    let button = unsafe { GetDlgItem(Some(dialog), ALLOW_ID) }.expect("allow button");
    let (mut outer, mut inner) = (RECT::default(), RECT::default());
    unsafe {
        GetWindowRect(dialog, &mut outer).expect("dialog rect");
        GetWindowRect(button, &mut inner).expect("button rect");
    }
    ((inner.left + inner.right) / 2 - outer.left, (inner.top + inner.bottom) / 2 - outer.top)
}

/// Waits until the dialog's allow button is armed (or the dialog is gone), then a while longer,
/// so input the dialog still had queued at the arming has been handled.
fn wait_until_armed(dialog: HWND) {
    // Gone already (answered): nothing to wait for.
    let Ok(allow) = (unsafe { GetDlgItem(Some(dialog), ALLOW_ID) }) else { return };
    wait_until(
        || !unsafe { IsWindow(Some(dialog)) }.as_bool() || unsafe { IsWindowEnabled(allow) }.as_bool(),
        "the allow button was never armed",
    );
    std::thread::sleep(Duration::from_millis(500));
}

/// Fails, naming the answer, if the question was answered or its dialog closed.
fn assert_unanswered(helper: &mut Helper, asked: u64, dialog: HWND, what: &str) {
    if helper.answered(asked) {
        panic!("{what} answered the question: {}", helper.responses[&asked]);
    }
    assert!(unsafe { IsWindowVisible(dialog) }.as_bool(), "the dialog closed after {what}");
}

/// (Whether the dialog's thread holds the mouse capture, whether its deny button is pushed): what
/// a press whose release was lost leaves behind.
fn pressed_state(dialog: HWND) -> (bool, bool) {
    const BST_PUSHED: usize = 0x0004;
    unsafe {
        let thread = GetWindowThreadProcessId(dialog, None);
        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        let capture = GetGUIThreadInfo(thread, &mut info).is_ok() && !info.hwndCapture.0.is_null();
        let deny = GetDlgItem(Some(dialog), DENY_ID).expect("deny button");
        let pushed = SendMessageW(deny, BM_GETSTATE, None, None).0 as usize & BST_PUSHED != 0;
        (capture, pushed)
    }
}

/// The dialog's client area in screen coordinates.
fn client_rect_on_screen(dialog: HWND) -> RECT {
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(dialog, &mut rect).expect("client rect");
        assert!(ClientToScreen(dialog, &mut origin).as_bool(), "client origin");
    }
    RECT { left: origin.x, top: origin.y, right: origin.x + rect.right, bottom: origin.y + rect.bottom }
}

/// Copies a rectangle of the screen the way a GDI screenshot does; one RGB triple per pixel.
fn capture_screen(rect: RECT) -> Vec<[u8; 3]> {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
        BitBlt(memory, 0, 0, width, height, Some(screen), rect.left, rect.top, SRCCOPY).expect("BitBlt");
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(memory, bitmap, 0, height as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        assert_eq!(lines, height, "GetDIBits");
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    bgra.chunks_exact(4).map(|pixel| [pixel[2], pixel[1], pixel[0]]).collect()
}

/// A top-level window of the test's own, on a thread of its own that runs its messages, shown at
/// the top of the ordinary (not topmost) windows without activation. Closed when dropped.
struct TestWindow {
    hwnd: isize,
    thread: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestWindow {
    fn show(class: &'static str, ex_style: WINDOW_EX_STYLE, style: WINDOW_STYLE, rect: RECT, background: Option<COLORREF>, proc: WNDPROC) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(None).expect("module");
            let class: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = RegisterClassExW(&WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: instance.into(),
                hbrBackground: background.map(|color| CreateSolidBrush(color)).unwrap_or_default(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            });
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            let hwnd = CreateWindowExW(ex_style, PCWSTR(class.as_ptr()), w!("CUMCP test window"), style, rect.left, rect.top, width, height, None, None, Some(instance.into()), None)
                .expect("test window");
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), rect.left, rect.top, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            sender.send((hwnd.0 as isize, GetCurrentThreadId())).expect("test window shown");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                DispatchMessageW(&message);
            }
            let _ = DestroyWindow(hwnd);
        });
        let (hwnd, thread) = receiver.recv_timeout(Duration::from_secs(5)).expect("test window shown");
        Self { hwnd, thread, handle: Some(handle) }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn plain_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn wait_until(mut condition: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{message}");
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else { return false };
        let _ = CloseDesktop(desktop);
        true
    }
}

/// A helper whose responses may arrive out of order: requests refused while the dialog is open
/// are answered before the `ask_user` request that opened it.
struct Helper {
    child: Child,
    lines: Receiver<Value>,
    responses: HashMap<u64, Value>,
    next_id: u64,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--lang", "zh-TW", "--server-pid", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("helper");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let ready = lines.recv_timeout(Duration::from_secs(10)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        Self { child, lines, responses: HashMap::new(), next_id: 1 }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
        id
    }

    fn drain(&mut self) {
        while let Ok(value) = self.lines.try_recv() {
            self.keep(value);
        }
    }

    fn keep(&mut self, value: Value) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            self.responses.insert(id, value);
        }
    }

    fn answered(&mut self, id: u64) -> bool {
        self.drain();
        self.responses.contains_key(&id)
    }

    fn wait(&mut self, id: u64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = self.responses.remove(&id) {
                return value;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) => self.keep(value),
                Err(_) => panic!("no response to request {id} within {timeout:?}"),
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.wait(id, Duration::from_secs(15))
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

