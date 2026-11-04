//! `ask_user`: a native dialog in which the user answers an approval question the MCP client
//! could not ask.
//!
//! Nothing but the user can answer it:
//! - While it is open the helper runs no other request. The stdin reader refuses every request
//!   that arrives after an `ask_user` line with `prompt_open` without queueing it (see [`screen`];
//!   only `overlay_hide` and `session_end` wait instead; after a stop the refusal is `interrupted`,
//!   ticket 56), and the executing thread waits for the dialog, which runs on a thread of its own
//!   (ticket 82, [`show_on_own_thread`]), so no input method can run at all, let alone aim at the
//!   dialog. The server also holds its own requests back until the answer arrives.
//! - The dialog belongs to the helper process, whose windows are never listed, described or
//!   targeted (`windows_list::describe`, `input::hwnd_from`), and it is destroyed before the
//!   answer is written.
//! - There is no parameter or method that answers or skips it. Its only exits are the user's
//!   click or key, the timeout (answered `timeout`), and a stop (the physical Esc), which closes it
//!   without allowing. If its thread is held up past the timeout and a grace, the request stops
//!   waiting with `prompt_unavailable` and the dialog closes without allowing (ticket 86).
//!
//! The default button is the refusal and has the focus, so Enter or Space alone refuses, and the
//! allow buttons only take clicks after [`ARM_DELAY`], so a click already on its way when the dialog
//! appears cannot allow. Ticket 95: an `approve_app` question with `allow_always: true` has a third
//! button, "Always allow" (answer `allow_always`), armed with "Allow once" and held to the same rules. Ticket 68: that holds even when the dialog only takes such input from its
//! queue after arming (its thread can be held up showing itself, for example by a slow foreground
//! app): input stamped before the arming is dropped then (see [`stale_input`]). The dialog is also
//! excluded from screen capture, like the overlay, so the model's screenshots never show it.
//!
//! Ticket 114: `confirm_change` shows Deskpass's confirm window for a change the settings page
//! asks for that loosens a limit (restoring an app's default): which app, its current state, the
//! state it is restored to, and the reminder to choose "Cancel" if the user did not press this on the settings page. It is this same dialog with
//! other text, buttons "Allow" and "Cancel" (the refusal, default with the focus) and a class of its
//! own (`DESKPASS_CONFIRM_CHANGE`), held to every rule above: while it is open other requests are
//! refused `prompt_open` (an `ask_user` and another `confirm_change` too), and it answers `allow`,
//! `cancel` ("Cancel", Enter or Space on the default, closing it) or `timeout`; a stop closes it
//! without allowing, `interrupted` like the question.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, DrawTextW, GetDC, GetMonitorInfoW, GetSysColorBrush, COLOR_BTNFACE, MonitorFromPoint, ReleaseDC, SelectObject, DT_CALCRECT, DT_EDITCONTROL,
    DT_NOPREFIX, DT_WORDBREAK, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_ESCAPE, VK_F4};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetDlgItem, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, IsDialogMessageW, IsWindow, IsWindowVisible, KillTimer, LoadCursorW, RegisterClassExW,
    SendMessageW, SetForegroundWindow, SetTimer, SetWindowDisplayAffinity, SetWindowPos, SwitchToThisWindow, TranslateMessage, HMENU, HWND_TOPMOST,
    IDC_ARROW, MSG, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WDA_EXCLUDEFROMCAPTURE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_KEYDOWN, WM_KEYFIRST, WM_KEYLAST, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP,
    WM_MOUSELAST, WM_RBUTTONUP, WM_SETFONT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_XBUTTONUP, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_DISABLED, WS_EX_DLGMODALFRAME, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};

use crate::esc;
use crate::protocol::{err, Request, Response};
use crate::strings;

/// Bounds of the `timeout_ms` parameter.
pub const MIN_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMEOUT_MS: u64 = 120_000;
/// How long after the dialog appears the allow button starts taking clicks.
const ARM_DELAY: Duration = Duration::from_millis(700);
const POLL_MS: u32 = 50;
const POLL_TIMER: usize = 1;

/// The window classes and control ids; the tests' answering tool finds the dialog by them.
const CLASS: PCWSTR = w!("DESKPASS_ASK_USER");
/// Ticket 114: the `confirm_change` window, the same dialog under a class of its own.
const CONFIRM_CLASS: PCWSTR = w!("DESKPASS_CONFIRM_CHANGE");
const TEXT_ID: i32 = 100;
const ALLOW_ID: i32 = 101;
const DENY_ID: i32 = 102;
/// Ticket 95: "Always allow", only on an `approve_app` question asked with `allow_always: true`.
const ALWAYS_ID: i32 = 103;

const IDCANCEL: usize = 2;
const BN_CLICKED: usize = 0;
const BS_PUSHBUTTON: u32 = 0x0000;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const SS_NOPREFIX: u32 = 0x0080;
const SS_EDITCONTROL: u32 = 0x2000;
const DM_GETDEFID: u32 = 0x0400;
const DC_HASDEFID: usize = 0x534B;

/// The methods that show the dialog: from the moment their line is read until they are finished,
/// every other request is refused (ticket 114: `confirm_change` too).
const DIALOG_METHODS: [&str; 2] = ["ask_user", "confirm_change"];

/// Whether `method` shows the dialog; the executing thread then calls [`finished`] after it.
pub fn shows_dialog(method: &str) -> bool {
    DIALOG_METHODS.contains(&method)
}

/// Set by the stdin reader when it reads an `ask_user` or `confirm_change` request, cleared by
/// the executing thread once that request is finished.
static OPEN: AtomicBool = AtomicBool::new(false);

/// The stop count when the open question's `ask_user` line was read (ticket 56): a later stop,
/// even one before its dialog appears, ends the question without allowing.
static ASKED_AT_STOPS: AtomicU64 = AtomicU64::new(0);

/// Whether a stop came after the open question's line was read.
fn stopped_since_asked() -> bool {
    esc::stop_count() > ASKED_AT_STOPS.load(Ordering::SeqCst)
}

fn interrupted() -> (String, String) {
    ("interrupted".to_string(), esc::INTERRUPTED_MESSAGE.to_string())
}

/// Requests that only put away the helper's own things; they wait behind an open question
/// instead of being refused. The server sends them when a stop ends its wait for the answer,
/// which can be a moment before the dialog has closed.
const CLEANUP_METHODS: [&str; 2] = ["overlay_hide", "session_end"];

/// Screens one request line on the stdin reader, before it is queued. While a question is open
/// (from the moment its `ask_user` or `confirm_change` line was read) every other request is
/// refused here and never runs, except the cleanup requests above.
pub fn screen(line: &str) -> Option<Response> {
    let request = serde_json::from_str::<Request>(line).ok()?;
    if OPEN.load(Ordering::SeqCst) && !CLEANUP_METHODS.contains(&request.method.as_str()) {
        // After a stop the question is closing; what the request meets is the stop.
        if stopped_since_asked() {
            let (code, message) = interrupted();
            return Some(err(request.id, &code, message));
        }
        return Some(err(
            request.id,
            "prompt_open",
            "The helper is waiting for the user to answer a question and takes no requests until then; nothing was done.",
        ));
    }
    if shows_dialog(&request.method) {
        ASKED_AT_STOPS.store(esc::stop_count(), Ordering::SeqCst);
        OPEN.store(true, Ordering::SeqCst);
    }
    None
}

/// Called by the executing thread when the `ask_user` or `confirm_change` request is finished,
/// before its response.
pub fn finished() {
    OPEN.store(false, Ordering::SeqCst);
}

/// Test builds only, like `debug_stop` (ticket 82): how long the next dialog's thread is held up,
/// shown but before it takes the foreground (`take_foreground`), as a slow foreground app held the
/// request thread there before the dialog had a thread of its own. The ticket 68 tests use it to
/// have the dialog take early input from its queue only after arming. It neither answers nor skips
/// the dialog, and release builds have neither it nor the `debug_hold_dialog` method.
#[cfg(debug_assertions)]
static DEBUG_HOLD_MS: AtomicU64 = AtomicU64::new(0);

/// Ticket 86 review: whether the next dialog's hold comes later, once its thread has attached to
/// the foreground thread's input and before it brings itself in front, as a thread stuck in that
/// attach would resume.
#[cfg(debug_assertions)]
static DEBUG_HOLD_AFTER_ATTACH: AtomicBool = AtomicBool::new(false);

/// `debug_hold_dialog` `{ms, after_attach?}`: sets the hold for the next dialog.
#[cfg(debug_assertions)]
pub fn debug_hold(params: &Value) -> Result<Value, (String, String)> {
    let ms = params.get("ms").and_then(Value::as_u64).filter(|ms| *ms <= 10_000).ok_or_else(|| ("invalid_params".to_string(), "ms must be a whole number of milliseconds up to 10000.".to_string()))?;
    let after_attach = match params.get("after_attach") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| ("invalid_params".to_string(), "after_attach must be true or false.".to_string()))?,
    };
    DEBUG_HOLD_AFTER_ATTACH.store(after_attach, Ordering::SeqCst);
    DEBUG_HOLD_MS.store(ms, Ordering::SeqCst);
    Ok(json!({}))
}

/// Holds the dialog's thread here if the hold was set for this point (`after_attach` or not).
#[cfg(debug_assertions)]
fn debug_hold_at(after_attach: bool) {
    if DEBUG_HOLD_AFTER_ATTACH.load(Ordering::SeqCst) != after_attach {
        return;
    }
    DEBUG_HOLD_AFTER_ATTACH.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(DEBUG_HOLD_MS.swap(0, Ordering::SeqCst)));
}

enum Kind {
    /// `always`: whether "Always allow" is offered (ticket 95).
    ApproveApp { name: String, path: String, always: bool },
    Continue,
    /// Ticket 97: whether the AI may read the clipboard's text this session; "Deny / Allow once" only.
    ReadClipboard,
    /// Ticket 114: `confirm_change`, a loosening the settings page asks for: "Cancel / Allow".
    ConfirmChange { name: String, current: String, restored: String },
}

#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Allow,
    AllowAlways,
    Deny,
    Timeout,
}

struct Question {
    kind: Kind,
    timeout: Duration,
}

pub fn ask_user(params: &Value) -> Result<Value, (String, String)> {
    // Ticket 54: a `press_key` still waiting in the background for its target's release ends
    // before the dialog takes the foreground, so its attach never spans the question.
    crate::input::end_late_key_release();
    let question = parse(params).map_err(|message| ("invalid_params".to_string(), message))?;
    let answer = match ask(question)? {
        Answer::Allow => "allow",
        Answer::AllowAlways => "allow_always",
        Answer::Deny => "deny",
        Answer::Timeout => "timeout",
    };
    Ok(json!({ "answer": answer }))
}

/// Ticket 114: `confirm_change` `{app_name, current_state, restored_state, timeout_ms}` shows the
/// confirm window for restoring `app_name` from `current_state` to its default `restored_state`
/// (display texts, shown as given) and answers `{answer: "allow" | "cancel" | "timeout"}`.
pub fn confirm_change(params: &Value) -> Result<Value, (String, String)> {
    crate::input::end_late_key_release();
    let question = parse_confirm(params).map_err(|message| ("invalid_params".to_string(), message))?;
    let answer = match ask(question)? {
        Answer::Allow => "allow",
        Answer::Timeout => "timeout",
        // The window has no "Always allow", so nothing else than a refusal is left.
        Answer::Deny | Answer::AllowAlways => "cancel",
    };
    Ok(json!({ "answer": answer }))
}

/// Shows the dialog for `question` unless a stop, a locked screen or a missing desktop rules it
/// out, and waits for its answer.
fn ask(question: Question) -> Result<Answer, (String, String)> {
    // Ticket 56: a stop since the line was read (while it waited behind another request, or just
    // now) ends the question before any dialog appears.
    if stopped_since_asked() {
        return Err(interrupted());
    }
    if session_locked() {
        return Err(("prompt_unavailable".to_string(), "The screen is locked, so the user cannot see a dialog.".to_string()));
    }
    if !interactive_desktop() {
        return Err((
            "prompt_unavailable".to_string(),
            "There is no interactive desktop to show the dialog on (the session may be disconnected).".to_string(),
        ));
    }
    show_on_own_thread(question).map_err(|message| ("prompt_unavailable".to_string(), message))
}

fn parse(params: &Value) -> Result<Question, String> {
    let text = |name: &str| params.get(name).and_then(Value::as_str).filter(|value| !value.trim().is_empty()).map(str::to_string);
    let kind = match params.get("kind").and_then(Value::as_str) {
        Some("approve_app") => Kind::ApproveApp {
            name: text("app_name").ok_or("app_name is required for kind approve_app.")?,
            path: text("app_path").ok_or("app_path is required for kind approve_app.")?,
            always: match params.get("allow_always") {
                None => false,
                Some(value) => value.as_bool().ok_or("allow_always must be true or false.")?,
            },
        },
        Some("continue" | "read_clipboard") if params.get("allow_always").is_some() => return Err("allow_always is only for kind approve_app.".to_string()),
        Some("continue") => Kind::Continue,
        Some("read_clipboard") => Kind::ReadClipboard,
        _ => return Err("kind must be approve_app, continue or read_clipboard.".to_string()),
    };
    Ok(Question { kind, timeout: parse_timeout(params)? })
}

fn parse_confirm(params: &Value) -> Result<Question, String> {
    let text = |name: &str| {
        params.get(name).and_then(Value::as_str).filter(|value| !value.trim().is_empty()).map(str::to_string).ok_or(format!("{name} must be a non-empty string."))
    };
    let kind = Kind::ConfirmChange { name: text("app_name")?, current: text("current_state")?, restored: text("restored_state")? };
    Ok(Question { kind, timeout: parse_timeout(params)? })
}

fn parse_timeout(params: &Value) -> Result<Duration, String> {
    let timeout_ms = params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .filter(|ms| (MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(ms))
        .ok_or_else(|| format!("timeout_ms must be a whole number of milliseconds from {MIN_TIMEOUT_MS} to {MAX_TIMEOUT_MS}."))?;
    Ok(Duration::from_millis(timeout_ms))
}

/// Whether the session is locked. The lock screen runs on the same desktop (OpenInputDesktop
/// still opens it), so a dialog would sit behind it unseen and time out as if the user ignored it.
/// Unknown counts as unlocked: the dialog then times out, which refuses all the same.
fn session_locked() -> bool {
    unsafe {
        let mut buffer = PWSTR::null();
        let mut bytes = 0u32;
        if WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buffer, &mut bytes).is_err()
            || buffer.is_null()
        {
            return false;
        }
        let locked = bytes as usize >= size_of::<WTSINFOEXW>() && {
            let info = &*(buffer.0 as *const WTSINFOEXW);
            info.Level == 1 && info.Data.WTSInfoExLevel1.SessionFlags as u32 == WTS_SESSIONSTATE_LOCK
        };
        WTSFreeMemory(buffer.0 as *mut core::ffi::c_void);
        locked
    }
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else { return false };
        let _ = CloseDesktop(desktop);
        true
    }
}

struct Open {
    answer: Option<Answer>,
    deadline: Instant,
    arm_at: Instant,
    /// The tick count (`GetTickCount`, the clock of `MSG::time`) read right after the allow button
    /// was enabled; `None` until then.
    armed_tick: Option<u32>,
    stops_at_start: u64,
    /// Ticket 86: set once the request stopped waiting for this dialog; it then closes as a refusal.
    given_up: Arc<AtomicBool>,
}

thread_local! {
    static OPEN_DIALOG: RefCell<Option<Open>> = const { RefCell::new(None) };
}

fn settle(answer: Answer) {
    OPEN_DIALOG.with(|slot| {
        if let Some(open) = slot.borrow_mut().as_mut() {
            open.answer.get_or_insert(answer);
        }
    });
}

/// Ticket 82: shows the dialog on a thread started for this question, which ends once it is
/// answered; the calling thread waits for it. `take_foreground` attaches the dialog's thread to the
/// foreground thread's input while the dialog is that thread's active window, and a thread that did
/// so no longer sees, through a later attach to a target (`TargetQueue`), the key state follow the
/// target's reads: on the request thread, every `paste_text` after a question ended at its
/// 5-second limit with Ctrl+V unread. So the request thread, which attaches to targets for
/// `type_text` and `paste_text`, never owns a window.
///
/// Ticket 86: the wait is limited to the question's timeout plus [`DIALOG_GRACE`]. Past it the
/// question ends `prompt_unavailable` and the thread is left to finish alone: it is marked given up,
/// and whatever it answers afterwards goes nowhere. It cannot be closed from here (only a window's
/// own thread can destroy it, and a stuck thread runs nothing sent or posted to it), so the mark is
/// a flag the thread checks itself rather than a WM_CLOSE posted to its window: a flag cannot reach
/// a later dialog that happens to get the same window handle, and the dialog's 50 ms poll already
/// wakes its loop to see it. Once its thread runs again it does not take the foreground, does not
/// wait for input, and destroys its window. Its state is its thread's own (`OPEN_DIALOG`) and it
/// changes neither `OPEN` nor the stop bookkeeping, so the next question is unaffected.
fn show_on_own_thread(question: Question) -> Result<Answer, String> {
    let timeout = question.timeout;
    let given_up = Arc::new(AtomicBool::new(false));
    let (sender, answered) = mpsc::channel();
    let dialog_given_up = Arc::clone(&given_up);
    std::thread::Builder::new()
        .name("ask_user".to_string())
        .spawn(move || {
            let _ = sender.send(unsafe { show(&question, dialog_given_up) });
        })
        .map_err(|error| format!("The dialog could not be started: {error}"))?;
    match answered.recv_timeout(timeout + DIALOG_GRACE) {
        Ok(result) => result,
        Err(RecvTimeoutError::Disconnected) => Err("The dialog ended without an answer.".to_string()),
        Err(RecvTimeoutError::Timeout) => {
            given_up.store(true, Ordering::SeqCst);
            Err(format!(
                "The dialog did not finish within its {} s timeout and {} s of grace (its thread is held up, perhaps by the app in front), so the question was not answered; the dialog closes as soon as its thread runs again.",
                timeout.as_millis().div_ceil(1000),
                DIALOG_GRACE.as_secs()
            ))
        }
    }
}

/// Ticket 86: how long past the question's own timeout the request waits for the dialog's thread.
/// A dialog that runs closes itself at its timeout within one 50 ms poll and is gone a moment after,
/// and a slow foreground app has been seen to hold up taking the foreground for about a second; 5 s
/// covers both many times over. It stays well inside the 15 s the server waits beyond the dialog's
/// time (`ASK_USER_TIMEOUT_MS` in `src/config.ts`), so the helper always answers first.
const DIALOG_GRACE: Duration = Duration::from_secs(5);

unsafe fn show(question: &Question, given_up: Arc<AtomicBool>) -> Result<Answer, String> {
    let instance = GetModuleHandleW(None).map_err(|error| error.to_string())?;
    register_class(instance.into());
    let dpi = GetDpiForSystem().max(96) as i32;
    let scale = |value: i32| value * dpi / 96;
    let seconds = question.timeout.as_millis().div_ceil(1000).to_string();
    let (title, body, allow_label, deny_label, always) = match &question.kind {
        Kind::ApproveApp { name, path, always: false } => (
            strings::text(strings::APPROVE_TITLE),
            strings::filled(strings::APPROVE_BODY, &[("name", name), ("path", path), ("seconds", &seconds)]),
            strings::text(strings::BUTTON_ALLOW_ONCE),
            strings::text(strings::BUTTON_DENY),
            false,
        ),
        Kind::ApproveApp { name, path, always: true } => (
            strings::text(strings::APPROVE_TITLE),
            strings::filled(strings::APPROVE_ALWAYS_BODY, &[("name", name), ("path", path), ("seconds", &seconds)]),
            strings::text(strings::BUTTON_ALLOW_ONCE),
            strings::text(strings::BUTTON_DENY),
            true,
        ),
        Kind::ReadClipboard => (
            strings::text(strings::CLIPBOARD_TITLE),
            strings::filled(strings::CLIPBOARD_BODY, &[("seconds", &seconds)]),
            strings::text(strings::BUTTON_ALLOW_ONCE),
            strings::text(strings::BUTTON_DENY),
            false,
        ),
        Kind::Continue => (
            strings::text(strings::CONTINUE_TITLE),
            strings::filled(strings::CONTINUE_BODY, &[("question", strings::text(strings::CONTINUE_QUESTION)), ("seconds", &seconds)]),
            strings::text(strings::BUTTON_CONTINUE),
            strings::text(strings::BUTTON_DONT_CONTINUE),
            false,
        ),
        // Ticket 114: the wording of the confirm window's mockup (Confirm.dc.html).
        Kind::ConfirmChange { name, current, restored } => (
            strings::text(strings::CONFIRM_TITLE),
            strings::filled(strings::CONFIRM_BODY, &[("name", name), ("current", current), ("restored", restored), ("seconds", &seconds)]),
            strings::text(strings::BUTTON_CONFIRM_ALLOW),
            strings::text(strings::BUTTON_CONFIRM_CANCEL),
            false,
        ),
    };
    let class = if matches!(question.kind, Kind::ConfirmChange { .. }) { CONFIRM_CLASS } else { CLASS };

    let font = message_font(dpi as u32);
    let margin = scale(16);
    let text_width = scale(460);
    let text_height = measure(&body, font, text_width);
    let button_width = scale(120);
    let button_height = scale(30);
    let client_width = text_width + margin * 2;
    let client_height = margin + text_height + margin + button_height + margin;

    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let ex_style = WS_EX_TOPMOST | WS_EX_DLGMODALFRAME;
    let mut frame = RECT { left: 0, top: 0, right: client_width, bottom: client_height };
    let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, dpi as u32);
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    let (x, y) = centered(width, height);

    let title = wide(title);
    let hwnd = CreateWindowExW(ex_style, class, PCWSTR(title.as_ptr()), style, x, y, width, height, None, None, Some(instance.into()), None)
        .map_err(|error| format!("The dialog could not be created: {error}"))?;
    let child = |class: PCWSTR, text: &str, id: i32, extra: u32, rect: (i32, i32, i32, i32)| -> Result<HWND, String> {
        let text = wide(text);
        let control = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(text.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | extra),
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            Some(hwnd),
            Some(HMENU(id as isize as *mut core::ffi::c_void)),
            Some(instance.into()),
            None,
        )
        .map_err(|error| format!("The dialog could not be created: {error}"))?;
        if let Some(font) = font {
            SendMessageW(control, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
        Ok(control)
    };
    let buttons_top = margin + text_height + margin;
    let gap = scale(8);
    // Right to left: the refusal, "Allow once", then "Always allow" when offered, the allow buttons
    // created disabled until they are armed.
    let slot_left = |slot: i32| client_width - margin - button_width * (slot + 1) - gap * slot;
    let built = (|| {
        child(w!("STATIC"), &body, TEXT_ID, SS_NOPREFIX | SS_EDITCONTROL, (margin, margin, text_width, text_height))?;
        let always = if always {
            Some(child(w!("BUTTON"), strings::text(strings::BUTTON_ALLOW_ALWAYS), ALWAYS_ID, WS_TABSTOP.0 | WS_DISABLED.0 | BS_PUSHBUTTON, (slot_left(2), buttons_top, button_width, button_height))?)
        } else {
            None
        };
        let allow = child(w!("BUTTON"), allow_label, ALLOW_ID, WS_TABSTOP.0 | WS_DISABLED.0 | BS_PUSHBUTTON, (slot_left(1), buttons_top, button_width, button_height))?;
        let deny = child(w!("BUTTON"), deny_label, DENY_ID, WS_TABSTOP.0 | BS_DEFPUSHBUTTON, (slot_left(0), buttons_top, button_width, button_height))?;
        Ok::<_, String>((allow, always, deny))
    })();
    let (allow, always, deny) = match built {
        Ok(controls) => controls,
        Err(message) => {
            let _ = DestroyWindow(hwnd);
            delete_font(font);
            return Err(message);
        }
    };

    let now = Instant::now();
    OPEN_DIALOG.with(|slot| {
        *slot.borrow_mut() = Some(Open {
            answer: None,
            deadline: now + question.timeout,
            arm_at: now + ARM_DELAY,
            armed_tick: None,
            stops_at_start: ASKED_AT_STOPS.load(Ordering::SeqCst),
            given_up: Arc::clone(&given_up),
        });
    });
    // Ticket 68: never in a screenshot; if this fails the dialog still asks.
    let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, SWP_SHOWWINDOW);
    take_foreground(hwnd, &given_up);
    // Ticket 86: given up on while its thread was held up: no focus is taken, no input waited for.
    if given_up.load(Ordering::SeqCst) {
        settle(Answer::Deny);
    } else {
        let _ = SetFocus(Some(deny));
    }
    let shown = IsWindowVisible(hwnd).as_bool();
    if shown {
        SetTimer(Some(hwnd), POLL_TIMER, POLL_MS, None);
        let mut message = MSG::default();
        while current_answer().is_none() {
            // Before taking the next message (ticket 68 review): a thread that was held up past
            // the arming arms first, so a click from then is dropped whole instead of its press
            // being handled unarmed and its release dropped.
            arm_when_due(allow, always);
            if GetMessageW(&mut message, None, 0, 0).0 <= 0 {
                settle(Answer::Deny);
                break;
            }
            if stale_input(&message) {
                // Dropped: input from before the arming that reaches the dialog only now.
            } else if !IsDialogMessageW(hwnd, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let _ = KillTimer(Some(hwnd), POLL_TIMER);
    }
    let answer = OPEN_DIALOG.with(|slot| slot.borrow_mut().take()).and_then(|open| open.answer);
    if IsWindow(Some(hwnd)).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
    delete_font(font);
    if !shown {
        return Err("The dialog could not be shown.".to_string());
    }
    // Nobody reads it then, but a dialog given up on never reports allow.
    if given_up.load(Ordering::SeqCst) {
        return Ok(Answer::Deny);
    }
    Ok(answer.unwrap_or(Answer::Deny))
}

fn current_answer() -> Option<Answer> {
    OPEN_DIALOG.with(|slot| slot.borrow().as_ref().and_then(|open| open.answer))
}

/// Enables the allow buttons ("Always allow" too, when there is one) once [`ARM_DELAY`] has passed
/// and records when. Outside the state borrow, because enabling a window sends it messages.
unsafe fn arm_when_due(allow: HWND, always: Option<HWND>) {
    let due = OPEN_DIALOG.with(|slot| slot.borrow().as_ref().is_some_and(|open| open.armed_tick.is_none() && Instant::now() >= open.arm_at));
    if !due {
        return;
    }
    let _ = EnableWindow(allow, true);
    if let Some(always) = always {
        let _ = EnableWindow(always, true);
    }
    // Read after enabling: input that came before the buttons were enabled is stamped no later.
    let tick = GetTickCount();
    OPEN_DIALOG.with(|slot| {
        if let Some(open) = slot.borrow_mut().as_mut() {
            open.armed_tick = Some(tick);
        }
    });
}

/// Ticket 68: whether `message` is mouse-button or keyboard input that happened no later than the
/// arming but is only taken from the queue now that the button is armed. Windows decides which
/// control a click lands on when the thread takes it from the queue, so a click made while allow
/// was disabled would otherwise land on allow if the dialog's thread was held up past the arming;
/// and a key from then (Tab, Space) could move the focus to allow and press it. Such input is
/// dropped, so only input after the arming can allow: a click on allow needs its press after the
/// arming (a button only clicks on a release after its own press), and a press before it released
/// after it does nothing. Input taken before the arming is handled as always (allow is disabled
/// then, so it can only refuse), and so are Esc and Alt+F4 at any time, which only refuse; the
/// title bar (non-client input) is not touched either. `MSG::time` and `GetTickCount` are the same
/// millisecond clock, which moves in steps of about 16 ms: input stamped in the step the arming
/// happened in counts as before it (dropped, never allowing; a person cannot react that fast). The
/// difference is compared wrapping, so the count's wrap after 49.7 days does not matter.
///
/// Releases (mouse button up, key up) are never dropped (ticket 68 review), so a press handled
/// before the arming (on deny, which then holds the mouse capture or is pushed) is always finished
/// and answers deny instead of being stranded. Letting them through cannot allow: a button clicks
/// only on the release of a press it took itself (the mouse capture, or Space pushing it while it
/// has the focus), and allow can only have taken a press, or the focus, from input that is not
/// stale: before the arming it is disabled, and after it stale presses and keys are dropped.
fn stale_input(message: &MSG) -> bool {
    let Some(armed_tick) = OPEN_DIALOG.with(|slot| slot.borrow().as_ref().and_then(|open| open.armed_tick)) else { return false };
    let input = matches!(message.message, WM_KEYFIRST..=WM_KEYLAST | WM_LBUTTONDOWN..=WM_MOUSELAST);
    let release = matches!(message.message, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP | WM_KEYUP | WM_SYSKEYUP);
    let key = message.wParam.0;
    let refuses_only = (message.message == WM_KEYDOWN && key == VK_ESCAPE.0 as usize) || (message.message == WM_SYSKEYDOWN && key == VK_F4.0 as usize);
    input && !release && !refuses_only && (message.time.wrapping_sub(armed_tick) as i32) <= 0
}

/// Whether the allow button is armed.
fn armed() -> bool {
    OPEN_DIALOG.with(|slot| slot.borrow().as_ref().is_some_and(|open| open.armed_tick.is_some()))
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            let notification = (wparam.0 >> 16) & 0xFFFF;
            if id == ALLOW_ID as usize || id == ALWAYS_ID as usize {
                // Only once armed (ticket 68 review): a notification another program sends before
                // then (WM_COMMAND, BM_CLICK) does not allow. Ticket 95: "Always allow" alike, and
                // only on a dialog that has the button: its id sent to one that was not offered it
                // answers nothing.
                let always = id == ALWAYS_ID as usize;
                if notification == BN_CLICKED && armed() && (!always || GetDlgItem(Some(hwnd), ALWAYS_ID).is_ok()) {
                    settle(if always { Answer::AllowAlways } else { Answer::Allow });
                }
            } else if id == DENY_ID as usize || id == IDCANCEL {
                settle(Answer::Deny);
            }
            LRESULT(0)
        }
        // The title bar's close button and Alt+F4 refuse.
        WM_CLOSE => {
            settle(Answer::Deny);
            LRESULT(0)
        }
        // Enter with the focus off a button presses the refusal.
        DM_GETDEFID => LRESULT(((DC_HASDEFID << 16) | DENY_ID as usize) as isize),
        WM_TIMER => {
            let stops = esc::stop_count();
            let now = Instant::now();
            let due = OPEN_DIALOG.with(|slot| {
                slot.borrow().as_ref().map(|open| {
                    if stops > open.stops_at_start || open.given_up.load(Ordering::SeqCst) {
                        Some(Answer::Deny)
                    } else if now >= open.deadline {
                        Some(Answer::Timeout)
                    } else {
                        None
                    }
                })
            });
            if let Some(Some(answer)) = due {
                settle(answer);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Registers both dialog classes, alike but for their names.
unsafe fn register_class(instance: windows::Win32::Foundation::HINSTANCE) {
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    for name in [CLASS, CONFIRM_CLASS] {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassExW(&class);
    }
}

/// Brings the dialog in front of the window the user is working in. No keys are pressed for it.
/// Not once the request has given up on it (ticket 86), so a dialog whose thread was held up does
/// not take the foreground from the next question's: that is checked again before each call that
/// brings it in front, since the thread can be held up in any of them (ticket 86 review). Only a
/// thread held up inside one of those calls itself can still finish that one call.
unsafe fn take_foreground(hwnd: HWND, given_up: &AtomicBool) {
    let live = || !given_up.load(Ordering::SeqCst);
    let current = GetCurrentThreadId();
    #[cfg(debug_assertions)]
    debug_hold_at(false);
    if !live() {
        return;
    }
    let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
    let attached = foreground_thread != 0 && foreground_thread != current && AttachThreadInput(current, foreground_thread, true).as_bool();
    #[cfg(debug_assertions)]
    debug_hold_at(true);
    if live() {
        let _ = BringWindowToTop(hwnd);
    }
    if live() {
        let _ = SetForegroundWindow(hwnd);
    }
    if attached {
        let _ = AttachThreadInput(current, foreground_thread, false);
    }
    if live() && GetForegroundWindow() != hwnd {
        SwitchToThisWindow(hwnd, true);
    }
}

/// The dialog's top-left corner: centred in the work area of the monitor the pointer is on.
pub(crate) unsafe fn centered(width: i32, height: i32) -> (i32, i32) {
    let mut pointer = POINT::default();
    let _ = GetCursorPos(&mut pointer);
    let monitor = MonitorFromPoint(pointer, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
        return (100, 100);
    }
    let work = info.rcWork;
    (work.left + (work.right - work.left - width).max(0) / 2, work.top + (work.bottom - work.top - height).max(0) / 2)
}

pub(crate) unsafe fn message_font(dpi: u32) -> Option<HFONT> {
    let mut metrics = NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    SystemParametersInfoForDpi(
        SPI_GETNONCLIENTMETRICS.0,
        metrics.cbSize,
        Some(&mut metrics as *mut NONCLIENTMETRICSW as *mut core::ffi::c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0).0,
        dpi,
    )
    .ok()?;
    let font = CreateFontIndirectW(&metrics.lfMessageFont);
    (!font.is_invalid()).then_some(font)
}

pub(crate) unsafe fn delete_font(font: Option<HFONT>) {
    if let Some(font) = font {
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

/// The height the text needs when wrapped to `width`.
unsafe fn measure(text: &str, font: Option<HFONT>, width: i32) -> i32 {
    let dc = GetDC(None);
    let previous = font.map(|font| SelectObject(dc, HGDIOBJ(font.0)));
    let mut buffer: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT { left: 0, top: 0, right: width, bottom: 0 };
    DrawTextW(dc, &mut buffer, &mut rect, DT_CALCRECT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX);
    if let Some(previous) = previous {
        SelectObject(dc, previous);
    }
    ReleaseDC(None, dc);
    (rect.bottom - rect.top).max(40)
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
