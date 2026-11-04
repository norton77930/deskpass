//! Ticket 77: `paste_text` puts text on the clipboard, sends Ctrl+V to the target and puts the
//! user's clipboard back, on the fixture window. The tests keep the developer's clipboard around
//! themselves (best effort: what the helper's own saving keeps, `clipboard::Saved`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::CreateBitmap;
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetForegroundWindow, GetWindowTextW, SendMessageW, ShowWindow, SW_HIDE, WM_GETTEXT, WM_GETTEXTLENGTH};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, DispatchMessageW, IsHungAppWindow, PeekMessageW, MSG, PM_REMOVE, WS_POPUP};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[allow(dead_code)]
#[path = "../src/clipboard.rs"]
mod clipboard;

/// An edit box (UI Automation's Value pattern) gets the whole text, the helper reads it there
/// (`read_confirmed: true`), and the text is in no answer.
#[test]
fn an_edit_box_gets_the_whole_text_and_the_paste_is_confirmed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 24);

    let text = "Pasted 77: 你好,世界 ".repeat(20);
    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": text }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    assert_eq!(pasted["result"]["read_confirmed"], true, "{pasted}");
    assert_eq!(pasted["result"]["clipboard_restored"], "full", "{pasted}");
    assert!(!pasted.to_string().contains("Pasted 77"), "the answer quotes the text: {pasted}");
    assert_eq!(edit_text(&window_id), text);
}

/// A custom-drawn area (the fixture's scroll area with `--paste-area`: it takes the focus when
/// clicked and, on Ctrl+V, reads the clipboard's text into its title as `paste:<text>`) has no
/// Value pattern: it gets the text, but the helper cannot read it there (`read_confirmed: false`).
#[test]
fn a_custom_drawn_area_gets_ctrl_v_and_the_paste_is_not_confirmed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "area text 77" }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    assert_eq!(pasted["result"]["read_confirmed"], false, "{pasted}");
    assert_eq!(pasted["result"]["clipboard_restored"], "full", "{pasted}");
    let shown = window_title(hwnd_of(&window_id));
    assert!(shown.contains("paste:area text 77"), "the area did not get the text: {shown}");
    assert_eq!(edit_text(&window_id), "", "the text went to the edit box");
}

/// While the text is on the clipboard it carries the three formats that keep it out of clipboard
/// monitors, the clipboard history (`CanIncludeInClipboardHistory` = 0) and the cloud clipboard
/// (`CanUploadToCloudClipboard` = 0), as the fixture sees them when it pastes (`fmt:` in its title).
#[test]
fn the_pasted_text_carries_the_formats_that_keep_it_out_of_history_and_the_cloud() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "private 77" }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    let shown = window_title(hwnd_of(&window_id));
    assert!(shown.contains("fmt:ex1/hist0/cloud0,paste:private 77"), "{shown}");
    assert_eq!(clipboard_text().as_deref(), Some("the user's own clip"));
    assert!(!clipboard_has("ExcludeClipboardContentFromMonitorProcessing"), "the exclusion format stayed on the user's clipboard");
}

/// The user's clipboard comes back as it was, in content and formats (`clipboard_restored: full`):
/// text, a picture (PNG beside a DIB, from which Windows makes the bitmap and the other DIB), and
/// copied files (`CF_HDROP`).
#[test]
fn text_a_picture_and_copied_files_come_back_as_they_were() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let png = registered("PNG");
    let cases: [(&str, Vec<(u32, Vec<u8>)>); 3] = [
        ("text", vec![(clipboard::CF_UNICODETEXT, wide("the user's own clip, 文字"))]),
        ("picture", vec![(png, b"\x89PNG\r\n\x1a\nnot really a png 77".to_vec()), (clipboard::CF_DIB, small_dib())]),
        ("files", vec![(clipboard::CF_HDROP, drop_files(&["C:\\Windows\\win.ini", "C:\\Windows\\system.ini"]))]),
    ];
    for (case, formats) in cases {
        put_clipboard(&formats);
        let mut before = clipboard_formats();
        let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": format!("over the {case}") }));
        assert_eq!(pasted["ok"], true, "{case}: {pasted}");
        assert_eq!(pasted["result"]["clipboard_restored"], "full", "{case}: {pasted}");
        assert!(window_title(hwnd_of(&window_id)).contains(&format!("paste:over the {case}")), "{case}: {}", window_title(hwnd_of(&window_id)));
        let mut after = clipboard_formats();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(after, before, "{case}: the formats changed");
        for (format, bytes) in &formats {
            assert_eq!(clipboard_bytes(*format).as_ref(), Some(bytes), "{case}: format {format} changed");
        }
    }
}

/// A clipboard holding only a GDI bitmap (`CF_BITMAP`, from which Windows offers a DIB) cannot be
/// copied whole: the paste says `partial`, and what was copied (the DIB) comes back.
#[test]
fn a_bitmap_only_clipboard_comes_back_partial() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    {
        let _open = clipboard::Open::new(50).expect("open the clipboard");
        unsafe {
            EmptyClipboard().expect("empty the clipboard");
            let pixels = [0xFFu8; 8];
            let bitmap = CreateBitmap(4, 2, 1, 1, Some(pixels.as_ptr().cast()));
            assert!(!bitmap.is_invalid(), "CreateBitmap");
            SetClipboardData(clipboard::CF_BITMAP, Some(HANDLE(bitmap.0))).expect("SetClipboardData(CF_BITMAP)");
        }
    }
    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "over a bitmap" }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    assert_eq!(pasted["result"]["clipboard_restored"], "partial", "{pasted}");
    assert!(window_title(hwnd_of(&window_id)).contains("paste:over a bitmap"), "{}", window_title(hwnd_of(&window_id)));
    let after = clipboard_formats();
    assert!(after.contains(&clipboard::CF_DIB), "the bitmap's DIB did not come back: {after:?}");
    assert!(!after.contains(&clipboard::CF_UNICODETEXT), "the pasted text stayed: {after:?}");
}

/// When another program puts something on the clipboard during the paste (here the fixture, right
/// after it pastes, with `--paste-overwrite`), the helper leaves it there (`skipped_changed`)
/// rather than overwrite it with the user's earlier clipboard.
#[test]
fn a_clipboard_changed_during_the_paste_is_left_alone() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--paste-area", "--paste-overwrite"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "before the overwrite" }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    assert_eq!(pasted["result"]["clipboard_restored"], "skipped_changed", "{pasted}");
    assert!(window_title(hwnd_of(&window_id)).contains("paste:before the overwrite"), "{}", window_title(hwnd_of(&window_id)));
    assert_eq!(clipboard_text().as_deref(), Some("fixture overwrite"));
}

/// A stop that comes once the text is on the clipboard but before V is sent sends no Ctrl+V and
/// puts the user's clipboard back. The fixture (`--hang-on-clipboard-update`) stops reading input
/// when our text lands on the clipboard (`cliphang`), so the helper is still waiting for it to read
/// Ctrl when the stop comes.
#[test]
fn a_stop_before_ctrl_v_sends_nothing_and_puts_the_clipboard_back() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--paste-area", "--hang-on-clipboard-update", "2500"]).spawn().expect("fixture"),
    );
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let id = helper.send("paste_text", json!({ "window_id": window_id, "text": "never pasted 77" }));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !window_title(hwnd_of(&window_id)).contains("cliphang") {
        assert!(Instant::now() < deadline, "the fixture never saw the text on the clipboard: {}", window_title(hwnd_of(&window_id)));
        std::thread::sleep(Duration::from_millis(10));
    }
    helper.send("debug_stop", json!({}));
    let stopped = helper.response(id);
    assert_eq!(stopped["ok"], false, "{stopped}");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    assert_eq!(stopped["error"]["message"], INTERRUPTED_MESSAGE, "{stopped}");
    // The stop is answered at once (esc's notifier); the call itself ends, putting the clipboard
    // back, before the helper answers the next request.
    let pinged = helper.request("ping", json!({}));
    assert_eq!(pinged["ok"], true, "{pinged}");
    assert_eq!(clipboard_text().as_deref(), Some("the user's own clip"), "the clipboard was not put back");
    // The fixture reads what it was sent once it wakes up: Ctrl, but no V.
    std::thread::sleep(Duration::from_millis(3000));
    let shown = window_title(hwnd_of(&window_id));
    assert!(!shown.contains("paste:"), "Ctrl+V reached the fixture: {shown}");
    assert!(!shown.contains("key:v"), "V reached the fixture: {shown}");
}

/// Review L3: a stop that comes once the target read V but before it read the releases gives the
/// target time to finish handling the V press before the user's clipboard is put back, so it
/// pastes the text rather than the user's clipboard. The fixture (`--hang-on-paste`) reads the
/// clipboard only a while after it read the V press (`pastehang`).
#[test]
fn a_stop_after_v_was_read_lets_the_target_finish_before_the_clipboard_is_put_back() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--paste-area", "--hang-on-paste", "250"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let id = helper.send("paste_text", json!({ "window_id": window_id, "text": "pasted before the stop" }));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !window_title(hwnd_of(&window_id)).contains("pastehang") {
        assert!(Instant::now() < deadline, "the fixture never read Ctrl+V: {}", window_title(hwnd_of(&window_id)));
        std::thread::sleep(Duration::from_millis(5));
    }
    helper.send("debug_stop", json!({}));
    let stopped = helper.response(id);
    assert_eq!(stopped["ok"], false, "{stopped}");
    assert_eq!(stopped["error"]["code"], "interrupted", "{stopped}");
    let pinged = helper.request("ping", json!({}));
    assert_eq!(pinged["ok"], true, "{pinged}");
    assert_eq!(clipboard_text().as_deref(), Some("the user's own clip"), "the clipboard was not put back");
    let shown = window_title(hwnd_of(&window_id));
    assert!(shown.contains("paste:pasted before the stop"), "the target pasted something else: {shown}");
}

/// Review M2: when the clipboard cannot be opened to put the user's back (here the fixture keeps
/// it open after pasting, `--hold-clipboard-after-paste`), nothing was restored and the pasted
/// text stays (`not_restored`), which is not `partial`.
#[test]
fn a_clipboard_kept_open_at_restore_time_is_not_restored() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture =
        KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--paste-area", "--hold-clipboard-after-paste", "2500"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "left on the clipboard" }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    let shown = window_title(hwnd_of(&window_id));
    assert!(shown.contains("paste:left on the clipboard,cliphold"), "{shown}");
    assert_eq!(pasted["result"]["clipboard_restored"], "not_restored", "{pasted}");
    // Once the fixture lets the clipboard go, the pasted text is what it holds.
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(clipboard_text().as_deref(), Some("left on the clipboard"));
}

/// Review L6: the paste is confirmed only in the element that had the focus before it. Here the
/// fixture (`--paste-moves-focus`) moves the focus on Ctrl+V to another edit that then shows the
/// text, while the edit box that had the focus stays empty: `read_confirmed: false`.
#[test]
fn text_shown_in_another_element_that_took_the_focus_does_not_confirm_the_paste() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-moves-focus").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 24);

    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "shown elsewhere 77" }));
    assert_eq!(pasted["ok"], true, "{pasted}");
    assert!(window_title(hwnd_of(&window_id)).contains("moved"), "the fixture did not move the focus: {}", window_title(hwnd_of(&window_id)));
    assert_eq!(pasted["result"]["read_confirmed"], false, "{pasted}");
    assert_eq!(edit_text(&window_id), "");
}

/// Review L5: text with a NUL character is refused (the clipboard's text would end there), without
/// quoting the text.
#[test]
fn text_with_a_nul_character_is_refused() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);

    let refused = helper.request("paste_text", json!({ "window_id": window_id, "text": "before\0after 77" }));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_params", "{refused}");
    assert!(!refused.to_string().contains("after 77"), "the answer quotes the text: {refused}");
}

/// Review L4: when the program that owns the clipboard is hung (here a window of this test that
/// put text there rendered late and then stops reading messages), saving the clipboard would wait
/// for it; the paste fails as for a busy clipboard instead, with nothing sent.
#[test]
fn a_hung_clipboard_owner_is_treated_as_a_busy_clipboard() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 180);

    let (owner, owner_thread) = hung_clipboard_owner(Duration::from_secs(7));
    let deadline = Instant::now() + Duration::from_millis(6500);
    while !unsafe { IsHungAppWindow(owner) }.as_bool() {
        assert!(Instant::now() < deadline, "Windows never took the owner for hung");
        std::thread::sleep(Duration::from_millis(100));
    }
    let started = Instant::now();
    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "over a hung owner" }));
    let took = started.elapsed();
    assert_eq!(pasted["ok"], false, "{pasted}");
    assert_eq!(pasted["error"]["code"], "input_failed", "{pasted}");
    assert!(pasted["error"]["message"].as_str().unwrap_or("").contains("Another program is using the clipboard"), "{pasted}");
    // Ticket 87: the error says, as a field, that no input was sent.
    assert_eq!(pasted["error"]["data"]["input_sent"], false, "{pasted}");
    assert!(took < Duration::from_secs(2), "the paste waited for the hung owner: {took:?}");
    assert!(!window_title(hwnd_of(&window_id)).contains("paste:"), "Ctrl+V reached the fixture: {}", window_title(hwnd_of(&window_id)));
    owner_thread.join().expect("owner thread");
}

/// Ticket 87: a target that cannot be brought to the foreground (here hidden) fails the paste
/// before anything is sent, and the error says so as a field, without quoting the text.
#[test]
fn a_target_that_cannot_take_the_foreground_fails_with_no_input_sent() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--paste-area").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let _ = unsafe { ShowWindow(hwnd_of(&window_id), SW_HIDE) };

    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "hidden target 87" }));
    assert_eq!(pasted["ok"], false, "{pasted}");
    assert_eq!(pasted["error"]["code"], "input_failed", "{pasted}");
    assert!(pasted["error"]["message"].as_str().unwrap_or("").contains("not visible"), "{pasted}");
    assert_eq!(pasted["error"]["data"]["input_sent"], false, "{pasted}");
    assert!(!pasted.to_string().contains("hidden target 87"), "the answer quotes the text: {pasted}");
}

/// Ticket 82 (reopened): an `ask_user` dialog shown and closed earlier in the same helper process
/// must not leave the helper unable to see the target read its keys. Live, a question that timed
/// out made every later `paste_text` end `input_failed` at the 5-second limit with Ctrl+V unread.
/// Here a question times out and another is answered (deny, by a real click), and a paste after
/// each still lands in the edit box.
#[test]
fn a_paste_after_a_question_that_timed_out_or_was_answered_lands() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let question = json!({ "kind": "approve_app", "app_name": "Paste 82", "app_path": "C:\\Apps\\paste82.exe", "timeout_ms": 1_000 });

    let asked = helper.request("ask_user", question.clone());
    assert_eq!(asked["result"]["answer"], "timeout", "{asked}");
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 24);
    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "after a timeout 82 " }));
    assert_eq!(pasted["ok"], true, "the paste after a timed-out question failed: {pasted}");
    assert_eq!(edit_text(&window_id), "after a timeout 82 ");

    let mut long = question;
    long["timeout_ms"] = json!(10_000);
    let asked = helper.send("ask_user", long);
    let answerer = Command::new(env!("CARGO_BIN_EXE_prompt-answerer"))
        .args(["--pid", &helper.child.id().to_string(), "--press", "deny", "--wait-ms", "10000"])
        .output()
        .expect("prompt-answerer");
    assert!(answerer.status.success(), "prompt-answerer failed: {}", String::from_utf8_lossy(&answerer.stderr));
    let asked = helper.response(asked);
    assert_eq!(asked["result"]["answer"], "deny", "{asked}");
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 24);
    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "after an answer 82" }));
    assert_eq!(pasted["ok"], true, "the paste after an answered question failed: {pasted}");
    assert!(edit_text(&window_id).contains("after an answer 82"), "{}", edit_text(&window_id));
}

/// Ticket 88: the request thread attached to the foreground thread's input while it had a window
/// active (`debug_attach_while_active`, what ticket 82's dialog did), so a queue it attaches no
/// longer sees the target read its keys. `paste_text` still pastes, well before its 5-second limit.
#[test]
fn a_paste_lands_when_the_request_thread_no_longer_sees_the_targets_key_state() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    // The fixture in front is the foreground thread the knob attaches to.
    activate(&mut helper, &window_id);
    let spoiled = helper.request("debug_attach_while_active", json!({}));
    assert_eq!(spoiled["ok"], true, "{spoiled}");
    assert_eq!(spoiled["result"]["attached"], true, "the knob did not attach: {spoiled}");
    let size = activate(&mut helper, &window_id);
    click_at(&mut helper, &window_id, size, 240, 24);

    let started = Instant::now();
    let pasted = helper.request("paste_text", json!({ "window_id": window_id, "text": "spoiled key state 88" }));
    let took = started.elapsed();
    assert_eq!(pasted["ok"], true, "the paste failed after {took:?}: {pasted}");
    assert!(took < Duration::from_secs(3), "the paste took {took:?}");
    assert_eq!(pasted["result"]["read_confirmed"], true, "{pasted}");
    assert_eq!(edit_text(&window_id), "spoiled key state 88");
}

/// Ticket 82: when plain `SetForegroundWindow` does not bring the target forward, the helper
/// presses ALT to lift the foreground lock (`tap_alt`) and tries again. A lone ALT released puts
/// an app with a menu bar into menu mode, which eats the Ctrl+V after it (live: `ok` with
/// `read_confirmed: false` and nothing pasted). `LockSetForegroundWindow` cannot force that path
/// here: the helper attaches to the foreground thread's input, which gets past the lock. Instead
/// another window of this test process holds the foreground and refuses its first deactivation,
/// so the first `SetForegroundWindow` fails and the ALT step brings the target, a window with a
/// menu bar and an edit box, forward. The holder has a menu bar too and shares the target's
/// thread, so a lone ALT puts that thread in menu mode whichever of the two handles it. The text
/// must land and neither window may enter menu mode.
#[test]
fn a_paste_through_the_alt_step_leaves_a_menu_bar_window_out_of_menu_mode() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("the user's own clip"))]);
    let windows = menu_windows::open();
    let mut helper = Helper::spawn();
    let holder_id = (windows.holder.0 as isize).to_string();
    let target_id = (windows.target.0 as isize).to_string();
    let activated = helper.request("activate_window", json!({ "window_id": holder_id }));
    assert_eq!(activated["ok"], true, "could not foreground the holding window: {activated}");
    assert_eq!(unsafe { GetForegroundWindow() }, windows.holder, "the holding window is not in front");
    menu_windows::arm();

    let pasted = helper.request("paste_text", json!({ "window_id": target_id, "text": "past the alt 82" }));
    std::thread::sleep(Duration::from_millis(300));
    let events = menu_windows::events();
    let in_menu_mode = menu_windows::in_menu_mode(windows.target);
    let text = menu_windows::edit_text(windows.edit);
    drop(helper);
    assert!(events.iter().any(|event| event == "holder:refused"), "the holder was never asked to give up the foreground: {events:?} {pasted}");
    assert!(events.iter().any(|event| event.ends_with(":down:18")), "no ALT reached the windows, so the ALT step was not taken: {events:?} {pasted}");
    assert!(!in_menu_mode && !events.iter().any(|event| event.ends_with(":menuloop")), "a window entered menu mode: {events:?} {pasted}");
    assert_eq!(pasted["ok"], true, "{pasted} {events:?}");
    assert_eq!(text, "past the alt 82", "{pasted} {events:?}");
}

/// The windows of `a_paste_through_the_alt_step_leaves_a_menu_bar_window_out_of_menu_mode`, on
/// one thread of this process: a target with a menu bar ("&File") and an edit box that gets the
/// focus, and a holder, with a menu bar of its own, that holds the foreground and, once `arm`ed,
/// refuses its next deactivation. Every ALT and mask key (vkE8) and every menu loop are recorded
/// (`events`).
mod menu_windows {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW,
        RegisterClassExW, SendMessageW, TranslateMessage, GUITHREADINFO, GUI_INMENUMODE, HMENU, MF_POPUP, MF_STRING, MSG, WM_ENTERMENULOOP, WM_GETTEXT, WM_GETTEXTLENGTH, WM_KEYDOWN,
        WM_KEYUP, WM_NCACTIVATE, WM_QUIT, WM_SETFOCUS, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static TARGET: Mutex<isize> = Mutex::new(0);
    static EDIT: Mutex<isize> = Mutex::new(0);
    static HOLDER: Mutex<isize> = Mutex::new(0);
    static REFUSE: AtomicBool = AtomicBool::new(false);

    pub struct Windows {
        pub target: HWND,
        pub edit: HWND,
        pub holder: HWND,
        thread: u32,
    }

    impl Drop for Windows {
        fn drop(&mut self) {
            let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Clears the events, then makes the holder refuse its next deactivation.
    pub fn arm() {
        EVENTS.lock().unwrap().clear();
        REFUSE.store(true, Ordering::SeqCst);
    }

    /// A menu bar with one menu, "&File", holding "&Open".
    unsafe fn menu_bar() -> HMENU {
        let menu = CreateMenu().expect("menu bar");
        let file = CreatePopupMenu().expect("file menu");
        AppendMenuW(file, MF_STRING, 1, w!("&Open")).expect("menu item");
        AppendMenuW(menu, MF_POPUP, file.0 as usize, w!("&File")).expect("menu");
        menu
    }

    pub fn open() -> Windows {
        let (sender, opened) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let instance = GetModuleHandleW(None).expect("module");
            let class = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: w!("CUMCP_MENU_TARGET"), ..Default::default() };
            let _ = RegisterClassExW(&class);
            let target = CreateWindowExW(Default::default(), w!("CUMCP_MENU_TARGET"), w!("CUMCP_MENU_TARGET"), WS_OVERLAPPEDWINDOW | WS_VISIBLE, 120, 120, 480, 240, None, Some(menu_bar()), Some(instance.into()), None)
                .expect("target window");
            let edit = CreateWindowExW(Default::default(), w!("Edit"), w!(""), WS_CHILD | WS_VISIBLE | WS_BORDER, 10, 10, 440, 28, Some(target), None, Some(instance.into()), None).expect("edit box");
            *TARGET.lock().unwrap() = target.0 as isize;
            *EDIT.lock().unwrap() = edit.0 as isize;
            let holder = CreateWindowExW(Default::default(), w!("CUMCP_MENU_TARGET"), w!("CUMCP_HOLDER"), WS_OVERLAPPEDWINDOW | WS_VISIBLE, 640, 120, 320, 200, None, Some(menu_bar()), Some(instance.into()), None)
                .expect("holding window");
            *HOLDER.lock().unwrap() = holder.0 as isize;
            let thread = windows::Win32::System::Threading::GetCurrentThreadId();
            sender.send((target.0 as isize, edit.0 as isize, holder.0 as isize, thread)).expect("send the windows");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let name = if message.hwnd == holder { "holder" } else { "target" };
                match message.message {
                    WM_SYSKEYDOWN | WM_KEYDOWN | WM_SYSKEYUP | WM_KEYUP if message.wParam.0 == 18 || message.wParam.0 == 0xE8 => {
                        let down = message.message == WM_SYSKEYDOWN || message.message == WM_KEYDOWN;
                        record(format!("{name}:{}:{}", if down { "down" } else { "up" }, message.wParam.0));
                    }
                    _ => {}
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        });
        let (target, edit, holder, thread) = opened.recv_timeout(Duration::from_secs(5)).expect("the windows");
        let hwnd = |value: isize| HWND(value as *mut core::ffi::c_void);
        Windows { target: hwnd(target), edit: hwnd(edit), holder: hwnd(holder), thread }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let is_target = hwnd.0 as isize == *TARGET.lock().unwrap();
        let is_holder = hwnd.0 as isize == *HOLDER.lock().unwrap();
        if msg == WM_ENTERMENULOOP {
            record(format!("{}:menuloop", if is_holder { "holder" } else { "target" }));
        }
        if is_target && msg == WM_SETFOCUS {
            let _ = SetFocus(Some(HWND(*EDIT.lock().unwrap() as *mut core::ffi::c_void)));
            return LRESULT(0);
        }
        // FALSE to WM_NCACTIVATE with wParam FALSE keeps the window from being deactivated.
        if is_holder && msg == WM_NCACTIVATE && wparam.0 == 0 && REFUSE.swap(false, Ordering::SeqCst) {
            record("holder:refused".to_string());
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    fn record(event: String) {
        EVENTS.lock().unwrap().push(event);
    }

    pub fn events() -> Vec<String> {
        EVENTS.lock().unwrap().clone()
    }

    /// Whether the thread of `hwnd` is in a menu loop now.
    pub fn in_menu_mode(hwnd: HWND) -> bool {
        unsafe {
            let thread = GetWindowThreadProcessId(hwnd, None);
            let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread, &mut info).is_ok() && info.flags.contains(GUI_INMENUMODE)
        }
    }

    pub fn edit_text(edit: HWND) -> String {
        unsafe {
            let length = SendMessageW(edit, WM_GETTEXTLENGTH, None, None).0.max(0) as usize;
            let mut buffer = vec![0u16; length + 1];
            let copied = SendMessageW(edit, WM_GETTEXT, Some(WPARAM(buffer.len())), Some(LPARAM(buffer.as_mut_ptr() as isize))).0.max(0) as usize;
            String::from_utf16_lossy(&buffer[..copied])
        }
    }
}

/// A hidden window, on a thread of its own, that owns the clipboard with `CF_UNICODETEXT` rendered
/// late and then reads no messages for `hang` (a delayed-rendering program that hung); afterwards
/// it answers what came meanwhile without rendering anything and is destroyed.
fn hung_clipboard_owner(hang: Duration) -> (HWND, std::thread::JoinHandle<()>) {
    let (ready, owner) = mpsc::channel();
    let thread = std::thread::spawn(move || unsafe {
        let window = CreateWindowExW(Default::default(), w!("STATIC"), w!(""), WS_POPUP, 0, 0, 1, 1, None, None, None, None).expect("owner window");
        OpenClipboard(Some(window)).expect("open the clipboard");
        EmptyClipboard().expect("empty the clipboard");
        // Rendered late: no data now, the owner is asked for it (WM_RENDERFORMAT) when it is read.
        let _ = SetClipboardData(clipboard::CF_UNICODETEXT, None);
        CloseClipboard().expect("close the clipboard");
        ready.send(window.0 as isize).expect("send the owner");
        std::thread::sleep(hang);
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            DispatchMessageW(&message);
        }
        let _ = DestroyWindow(window);
    });
    let owner = owner.recv_timeout(Duration::from_secs(5)).expect("the owner window");
    (HWND(owner as *mut core::ffi::c_void), thread)
}

const INTERRUPTED_MESSAGE: &str = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";

fn registered(name: &str) -> u32 {
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { RegisterClipboardFormatW(PCWSTR(wide_name.as_ptr())) }
}

/// A 2 x 2, 32-bit `CF_DIB`: a BITMAPINFOHEADER, then the pixels.
fn small_dib() -> Vec<u8> {
    let mut dib = Vec::new();
    for value in [40u32, 2, 2] {
        dib.extend(value.to_le_bytes());
    }
    dib.extend(1u16.to_le_bytes());
    dib.extend(32u16.to_le_bytes());
    for value in [0u32, 16, 0, 0, 0, 0] {
        dib.extend(value.to_le_bytes());
    }
    dib.extend([0x10, 0x20, 0x30, 0, 0x40, 0x50, 0x60, 0, 0x70, 0x80, 0x90, 0, 0xA0, 0xB0, 0xC0, 0]);
    dib
}

/// `CF_HDROP` for `paths`: a DROPFILES header (wide names start after its 20 bytes), then the names,
/// each ending in NUL, and one more NUL.
fn drop_files(paths: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [20u32, 0, 0, 0, 1] {
        bytes.extend(value.to_le_bytes());
    }
    for path in paths {
        bytes.extend(wide(path));
    }
    bytes.extend([0, 0]);
    bytes
}

/// Whether the clipboard holds the registered format `name` now.
fn clipboard_has(name: &str) -> bool {
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let format = unsafe { RegisterClipboardFormatW(PCWSTR(wide_name.as_ptr())) };
    clipboard_formats().contains(&format)
}

/// The fixture's main edit box's text, read directly.
fn edit_text(window_id: &str) -> String {
    unsafe {
        let edit = FindWindowExW(Some(hwnd_of(window_id)), None, w!("Edit"), None).expect("the fixture's edit box");
        // WM_GETTEXT: GetWindowText does not read another process's edit box.
        let length = SendMessageW(edit, WM_GETTEXTLENGTH, None, None).0.max(0) as usize;
        let mut buffer = vec![0u16; length + 1];
        let copied = SendMessageW(edit, WM_GETTEXT, Some(WPARAM(buffer.len())), Some(LPARAM(buffer.as_mut_ptr() as isize))).0.max(0) as usize;
        String::from_utf16_lossy(&buffer[..copied])
    }
}

/// The developer's clipboard, saved as the helper saves one and put back when dropped.
struct KeepClipboard(Option<clipboard::Saved>);

impl KeepClipboard {
    fn new() -> Self {
        Self(clipboard::Open::new(50).map(|open| open.save()))
    }
}

impl Drop for KeepClipboard {
    fn drop(&mut self) {
        if let (Some(saved), Some(open)) = (self.0.as_ref(), clipboard::Open::new(50)) {
            open.put_back(saved);
        }
    }
}

/// `text` as `CF_UNICODETEXT` bytes, ending in NUL.
fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// Empties the clipboard and puts `formats` there, as global memory blocks.
fn put_clipboard(formats: &[(u32, Vec<u8>)]) {
    let _open = clipboard::Open::new(50).expect("open the clipboard");
    unsafe {
        EmptyClipboard().expect("empty the clipboard");
        for (format, bytes) in formats {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).expect("GlobalAlloc");
            let pointer = GlobalLock(memory) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            let _ = GlobalUnlock(memory);
            if SetClipboardData(*format, Some(HANDLE(memory.0))).is_err() {
                let _ = GlobalFree(Some(memory));
                panic!("SetClipboardData({format}) failed");
            }
        }
    }
}

/// The formats on the clipboard now, in the order Windows lists them.
#[allow(dead_code)]
fn clipboard_formats() -> Vec<u32> {
    let _open = clipboard::Open::new(50).expect("open the clipboard");
    let mut formats = Vec::new();
    let mut format = 0;
    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            return formats;
        }
        formats.push(format);
    }
}

/// The bytes of `format` on the clipboard now.
#[allow(dead_code)]
fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    let _open = clipboard::Open::new(50).expect("open the clipboard");
    unsafe {
        let memory = HGLOBAL(GetClipboardData(format).ok()?.0);
        let size = GlobalSize(memory);
        let pointer = GlobalLock(memory) as *const u8;
        if pointer.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
        let _ = GlobalUnlock(memory);
        Some(bytes)
    }
}

/// The clipboard's Unicode text now, up to its first NUL.
#[allow(dead_code)]
fn clipboard_text() -> Option<String> {
    let bytes = clipboard_bytes(clipboard::CF_UNICODETEXT)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).take_while(|unit| *unit != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

/// Brings the fixture forward; returns its width and height.
fn activate(helper: &mut Helper, window_id: &str) -> (i64, i64) {
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground.0 as isize, window_id.parse::<isize>().expect("hwnd"), "refusing to send input: the fixture is not in the foreground");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    (info["result"]["width"].as_i64().unwrap_or(480), info["result"]["height"].as_i64().unwrap_or(360))
}

/// Clicks the fixture at (`x`, `y`) of its 480 x 360 layout, which gives that child the focus:
/// (240, 24) is the edit box, (240, 180) the scroll area.
fn click_at(helper: &mut Helper, window_id: &str, (width, height): (i64, i64), x: i64, y: i64) {
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": width * x / 480, "y": height * y / 360 }));
    assert_eq!(clicked["ok"], true, "{clicked}");
}

/// The fixture's title, read directly so it works while the helper or the fixture is busy.
#[allow(dead_code)]
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
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
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
