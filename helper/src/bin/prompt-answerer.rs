//! Test tool: answers the helper's `ask_user` dialog the way a person would.
//!
//! Only tests run it (the helper itself has no way to answer or skip its dialog). It finds the
//! visible dialog of one helper process, waits until its buttons accept clicks, and then either
//! clicks a button with the real mouse or presses a key on the keyboard.
//!
//! ```text
//! prompt-answerer (--pid <helper pid> | --parent-pid <pid that started the helper>)
//!                 --press <allow|always|deny|enter|space|alt-f4|allow-early|always-early|allow-across-arm|deny-early|deny-across-arm>
//!                 [--wait-ms <ms>] [--after-ms <ms>] [--announce] [--class <window class>]
//! ```
//!
//! `--class` names the dialog's window class: `DESKPASS_ASK_USER` (the default) or, ticket 114,
//! `DESKPASS_CONFIRM_CHANGE`, the confirm window of `confirm_change`, which has the same control
//! ids (its "Allow" is the allow button and "Cancel" the deny button).
//!
//! `--after-ms` waits that long after finding the dialog before doing anything else.
//!
//! `allow-early` clicks the allow button at once, without waiting for it to be enabled, to show
//! that a click that early does not allow. `allow-across-arm` presses the mouse button on allow at
//! once, waits until allow is enabled, and only then releases it (ticket 68). `deny-early` clicks
//! deny at once; `deny-across-arm` presses on deny at once and releases the moment allow is
//! enabled, so the release lands right at the arming. The `-across-arm` outputs also have
//! `allow_enabled_at_press`: whether allow was enabled right after the press was sent.
//! `always` and `always-early` (ticket 95) are `allow` and `allow-early` on "Always allow"; when the
//! dialog has that button the output also has `always_enabled`, read like `allow_enabled`.
//!
//! With `--announce` it first prints `{"waiting":true}` once it is looking for the dialog, so a
//! test can start it before the dialog appears and know it will act as soon as it does, however
//! long the process took to start.
//!
//! Prints one JSON line (`dialog`, `text`, `foreground`, and `allow_enabled`: whether the allow
//! button was enabled once the input was sent) and exits 0 when the input was sent.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    IsWindowEnabled, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, VK_F4, VK_MENU, VK_RETURN, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetDlgItem, GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, WindowFromPoint, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// The dialog's default window class and its control ids (`helper/src/ask.rs`).
const DIALOG_CLASS: &str = "DESKPASS_ASK_USER";
const TEXT_ID: i32 = 100;
const ALLOW_ID: i32 = 101;
const DENY_ID: i32 = 102;
const ALWAYS_ID: i32 = 103;

enum Owner {
    Pid(u32),
    Parent(u32),
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let (owner, press, wait, after, announce, class) = match parse_args() {
        Ok(parsed) => parsed,
        Err(message) => fail(&message),
    };
    let deadline = Instant::now() + wait;
    if announce {
        use std::io::Write;
        println!("{}", serde_json::json!({ "waiting": true }));
        let _ = std::io::stdout().flush();
    }
    let dialog = loop {
        if let Some(dialog) = find_dialog(&owner, &class) {
            break dialog;
        }
        if Instant::now() > deadline {
            fail("no approval dialog appeared");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    std::thread::sleep(after);
    let allow = unsafe { GetDlgItem(Some(dialog), ALLOW_ID) }.unwrap_or_default();
    let early = matches!(press.as_str(), "allow-early" | "always-early" | "allow-across-arm" | "deny-early" | "deny-across-arm");
    while !early && !unsafe { IsWindowEnabled(allow) }.as_bool() {
        if Instant::now() > deadline {
            fail("the dialog's buttons never became enabled");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !early {
        // Ticket 68: the dialog drops input stamped no later than the tick count (steps of about
        // 16 ms) at which it armed, so act a moment after the arming, as a person would.
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = unsafe { GetDlgItem(Some(dialog), TEXT_ID) }.map(window_text).unwrap_or_default();
    // Keys need the dialog in front; a click reaches it anyway, so only report whether it is.
    let needs_foreground = matches!(press.as_str(), "enter" | "space" | "alt-f4");
    let foreground = if needs_foreground { wait_for_foreground(dialog, deadline) } else { (unsafe { GetForegroundWindow() }) == dialog };
    if needs_foreground && !foreground {
        fail("the dialog is not the foreground window, so the key would go elsewhere");
    }
    let mut enabled_at_press = None;
    let pause = || std::thread::sleep(Duration::from_millis(50));
    match press.as_str() {
        "allow" => click_button(dialog, ALLOW_ID, false, pause),
        "allow-early" => click_button(dialog, ALLOW_ID, true, pause),
        "always" => click_button(dialog, ALWAYS_ID, false, pause),
        "always-early" => click_button(dialog, ALWAYS_ID, true, pause),
        "allow-across-arm" => click_button(dialog, ALLOW_ID, true, || {
            enabled_at_press = Some(hold_until_armed(allow, deadline, false));
            std::thread::sleep(Duration::from_millis(100));
        }),
        "deny" | "deny-early" => click_button(dialog, DENY_ID, false, pause),
        "deny-across-arm" => click_button(dialog, DENY_ID, false, || enabled_at_press = Some(hold_until_armed(allow, deadline, true))),
        "enter" => send_keys(&[key(VK_RETURN.0, false), key(VK_RETURN.0, true)]),
        "space" => send_keys(&[key(VK_SPACE.0, false), key(VK_SPACE.0, true)]),
        "alt-f4" => send_keys(&[key(VK_MENU.0, false), key(VK_F4.0, false), key(VK_F4.0, true), key(VK_MENU.0, true)]),
        other => fail(&format!("unknown --press {other}")),
    }
    // Read after the input, so `false` means the allow button was disabled for all of it.
    let allow_enabled = unsafe { IsWindowEnabled(allow) }.as_bool();
    let mut output =
        serde_json::json!({ "dialog": (dialog.0 as isize).to_string(), "text": text, "foreground": foreground, "allow_enabled": allow_enabled });
    if let Some(enabled) = enabled_at_press {
        output["allow_enabled_at_press"] = enabled.into();
    }
    if let Ok(always) = unsafe { GetDlgItem(Some(dialog), ALWAYS_ID) } {
        output["always_enabled"] = unsafe { IsWindowEnabled(always) }.as_bool().into();
    }
    println!("{output}");
}

fn parse_args() -> Result<(Owner, String, Duration, Duration, bool, String), String> {
    let mut owner = None;
    let mut class = DIALOG_CLASS.to_string();
    let mut press = None;
    let mut wait = Duration::from_secs(10);
    let mut after = Duration::ZERO;
    let mut announce = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--announce" {
            announce = true;
            continue;
        }
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--pid" => owner = Some(Owner::Pid(value.parse().map_err(|_| "bad --pid")?)),
            "--parent-pid" => owner = Some(Owner::Parent(value.parse().map_err(|_| "bad --parent-pid")?)),
            "--press" => press = Some(value),
            "--wait-ms" => wait = Duration::from_millis(value.parse().map_err(|_| "bad --wait-ms")?),
            "--after-ms" => after = Duration::from_millis(value.parse().map_err(|_| "bad --after-ms")?),
            "--class" => class = value,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok((owner.ok_or("--pid or --parent-pid is required")?, press.ok_or("--press is required")?, wait, after, announce, class))
}

fn fail(message: &str) -> ! {
    eprintln!("prompt-answerer: {message}");
    std::process::exit(1);
}

fn find_dialog(owner: &Owner, class: &str) -> Option<HWND> {
    let pids = match owner {
        Owner::Pid(pid) => vec![*pid],
        Owner::Parent(parent) => children_of(*parent),
    };
    let mut found: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut Vec<HWND> as isize));
    }
    found.into_iter().find(|hwnd| {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(*hwnd, Some(&mut pid));
        }
        pids.contains(&pid) && class_name(*hwnd) == class && unsafe { IsWindowVisible(*hwnd) }.as_bool()
    })
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
    windows::Win32::Foundation::TRUE
}

fn children_of(parent: u32) -> Vec<u32> {
    let mut children = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return children };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32ParentProcessID == parent {
                children.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    children
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 2048];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn wait_for_foreground(dialog: HWND, deadline: Instant) -> bool {
    loop {
        if unsafe { GetForegroundWindow() } == dialog {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// While the mouse button is down: waits until allow is enabled and says whether it already was.
/// `at_once` watches without sleeping, so the release that follows comes within a moment of the
/// arming. On the deadline it returns anyway, so the button is never left held; the output's
/// `allow_enabled` then says the release was not after the arming.
fn hold_until_armed(allow: HWND, deadline: Instant, at_once: bool) -> bool {
    let enabled_at_press = unsafe { IsWindowEnabled(allow) }.as_bool();
    while !unsafe { IsWindowEnabled(allow) }.as_bool() && Instant::now() < deadline {
        if at_once {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    enabled_at_press
}

/// Clicks the middle of a button with the mouse, after checking the point is on that button
/// (or, with `disabled_ok`, on the dialog where the disabled button is). `held` runs between the
/// press and the release.
fn click_button(dialog: HWND, id: i32, disabled_ok: bool, held: impl FnOnce()) {
    let Ok(button) = (unsafe { GetDlgItem(Some(dialog), id) }) else { fail("the button is missing") };
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(button, &mut rect) }.is_err() {
        fail("could not read the button's position");
    }
    let point = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
    // A disabled button is skipped by hit testing: the point then belongs to the dialog itself.
    let hit = unsafe { WindowFromPoint(point) };
    if hit != button && !(disabled_ok && hit == dialog && !unsafe { IsWindowEnabled(button) }.as_bool()) {
        fail("something covers the button");
    }
    let (x, y) = absolute(point);
    let mouse = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx: x, dy: y, mouseData: 0, dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, time: 0, dwExtraInfo: 0 },
        },
    };
    send(&[mouse(MOUSEEVENTF_MOVE)]);
    std::thread::sleep(Duration::from_millis(50));
    send(&[mouse(MOUSEEVENTF_LEFTDOWN)]);
    held();
    send(&[mouse(MOUSEEVENTF_LEFTUP)]);
}

fn absolute(point: POINT) -> (i32, i32) {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2);
        (((point.x - left) * 65535) / (width - 1), ((point.y - top) * 65535) / (height - 1))
    }
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_keys(inputs: &[INPUT]) {
    for input in inputs {
        send(std::slice::from_ref(input));
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn send(inputs: &[INPUT]) {
    if unsafe { SendInput(inputs, size_of::<INPUT>() as i32) } != inputs.len() as u32 {
        fail("SendInput failed");
    }
}
