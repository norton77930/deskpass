use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde_json::Value;

static INPUT_BUSY: AtomicBool = AtomicBool::new(false);

fn stop_requested() -> bool {
    crate::esc::stopped()
}

/// Checked before every SendInput that presses or moves something. Releases are always sent.
fn check_stop() -> Result<(), Failure> {
    if stop_requested() {
        return Err(interrupted().into());
    }
    Ok(())
}

/// Ticket 73: sends the button press that is a click's or a drag's action (`send`) unless input is
/// stopped (`interrupted`, nothing sent), and marks the request as acting once it is sent, so a
/// later stop is answered with `esc::STOPPED_AFTER_ACTION_MESSAGE` (`esc::act`, which checks the
/// stop, sends and marks as one step). Moving the pointer there first is not the action.
fn act(send: impl FnOnce() -> Result<(), String>) -> Result<Result<(), String>, Failure> {
    crate::esc::act(crate::esc::current_request(), send).ok_or_else(|| interrupted().into())
}

pub fn input_busy() -> bool {
    INPUT_BUSY.load(Ordering::SeqCst)
}

struct InputBusy;

impl InputBusy {
    /// Every input method starts here, so a `press_key` still waiting in the background for its
    /// target to read the last release (ticket 54) is ended before any new input is sent.
    fn enter() -> Self {
        INPUT_BUSY.store(true, Ordering::SeqCst);
        end_late_key_release();
        Self
    }
}

impl Drop for InputBusy {
    fn drop(&mut self) {
        INPUT_BUSY.store(false, Ordering::SeqCst);
    }
}

fn interrupted() -> (String, String) {
    ("interrupted".to_string(), "Input stopped before it finished.".to_string())
}

/// A failed foreground attempt is `interrupted` when a stop cut it short.
fn foreground_failure(message: String) -> Failure {
    if stop_requested() { interrupted().into() } else { ("input_failed".to_string(), message).into() }
}

/// Ticket 87: a check made before any of the request's input is sent; its failure says so
/// (`Failure::with_input_sent`). Failures after input was sent stay unmarked, which the server
/// takes as possibly sent.
fn before_input<T, E: Into<Failure>>(result: Result<T, E>) -> Result<T, Failure> {
    result.map_err(|error| error.into().with_input_sent(false))
}

/// Ticket 92: refuses the content of `type_text`, `paste_text` and a content key of `press_key`
/// (`sends_content`) while the keyboard focus is in a password field (`password_field`, nothing
/// sent). Checked once the target holds the foreground, right before its first input. When UI
/// Automation cannot tell, input goes ahead (`uia::focus_is_password`).
fn ensure_focus_not_password() -> Result<(), Failure> {
    if crate::uia::focus_is_password(std::time::Instant::now() + crate::uia::PASSWORD_CHECK_LIMIT) {
        return Err(crate::uia::password_field("The keyboard focus"));
    }
    Ok(())
}

/// Ticket 92 review F1: a character of `type_text` after which the focus may be elsewhere (Tab,
/// and Enter for LF; a CR is never sent on its own), so typing checks the focus again
/// (`ensure_focus_stays_off_password`) before the next one.
fn may_move_focus(character: char) -> bool {
    character == '\t' || character == '\n'
}

/// Ticket 92 review F1: `type_text`'s check after a character that may move the focus, once the
/// target read it: when the focus is now in a password field, typing stops there with
/// `password_field`, marked as sent (`typed` of `total` characters were). The text is not quoted.
fn ensure_focus_stays_off_password(typed: usize, total: usize) -> Result<(), Failure> {
    if !crate::uia::focus_is_password(std::time::Instant::now() + crate::uia::PASSWORD_CHECK_LIMIT) {
        return Ok(());
    }
    let message = format!(
        "The keyboard focus moved into a password field after character {typed} of {total}, so typing stopped: {typed} of {total} characters were typed and the remaining {} were not sent. Deskpass never enters passwords. Ask the user to type it themselves.",
        total - typed
    );
    Err(Failure::from(("password_field".to_string(), message)).with_input_sent(true))
}

/// Ticket 92: whether the key combination `keys` (pressed together, as `press_key` takes them)
/// can put content into a field, so `press_key` refuses it in a password field:
/// - a paste: Ctrl with V, Win with V (the clipboard history, whose choice is pasted), or Shift
///   with Insert, whatever else is held;
/// - a character key (a letter, a digit, Space) alone, with Shift, or with Ctrl+Alt (AltGr on
///   many layouts types a character), but not with Win, nor with Ctrl or Alt alone (shortcuts).
///
/// Everything else goes, so the focus can leave the field and edits that add nothing still work:
/// Tab, Enter, Esc, the arrows, Home, End, Page Up and Down, Backspace, Delete, Insert alone,
/// F1 to F24, and shortcuts such as Ctrl+A.
fn sends_content(keys: &[VIRTUAL_KEY]) -> bool {
    let held = |key: VIRTUAL_KEY| keys.contains(&key);
    let (ctrl, alt, shift, win) = (held(VK_CONTROL), held(VK_MENU), held(VK_SHIFT), held(VK_LWIN));
    if ((ctrl || win) && held(VIRTUAL_KEY(b'V' as u16))) || (shift && held(VK_INSERT)) {
        return true;
    }
    let character = keys.iter().any(|key| key.0 == VK_SPACE.0 || (b'A' as u16..=b'Z' as u16).contains(&key.0) || (b'0' as u16..=b'9' as u16).contains(&key.0));
    character && !win && ctrl == alt
}
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, GetKeyboardLayoutList, LoadKeyboardLayoutW, SendInput, UnloadKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE,
    KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, KLF_NOTELLSHELL, MOUSEINPUT, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN,
    VK_END, VK_ESCAPE, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PACKET, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE,
    VK_TAB, VK_UP, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumThreadWindows, GetAncestor, GetForegroundWindow, GetGUIThreadInfo, GUITHREADINFO, GetSystemMetrics, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    PeekMessageW, PostMessageW, MSG, PM_NOREMOVE,
    IsIconic, IsWindow, IsWindowVisible, SendMessageTimeoutW, SetForegroundWindow, SetWindowPos, ShowWindow, SwitchToThisWindow, GA_ROOTOWNER, GW_HWNDPREV,
    GW_OWNER, GWL_EXSTYLE,
    HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, SMTO_ABORTIFHUNG, SMTO_BLOCK, SM_CXVIRTUALSCREEN, SM_SWAPBUTTON, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE, WM_IME_CONTROL, WM_INPUTLANGCHANGEREQUEST, WM_NULL,
    WS_EX_NOACTIVATE, WS_EX_TOPMOST, WindowFromPoint, GA_PARENT, GA_ROOT,
};

use crate::popups::{popup_above, popup_kind, target_popups};
use crate::protocol::Failure;
use crate::windows_list::{app_unresolved, check_expected_app, identify, listed, parse_hwnd, top_level_windows, visible_frame, window_app, window_text, Identity};

/// Whether `owner` is in `window`'s owner chain: `window` is one of its dialogs (or a dialog of
/// one of those).
unsafe fn owned_by(window: HWND, owner: HWND) -> bool {
    if window.0.is_null() || window == owner {
        return false;
    }
    if GetAncestor(window, GA_ROOTOWNER) == owner {
        return true;
    }
    let mut current = window;
    for _ in 0..32 {
        match GetWindow(current, GW_OWNER) {
            Ok(next) if next == owner => return true,
            Ok(next) => current = next,
            Err(_) => return false,
        }
    }
    false
}

/// Ticket 154: whether `window`, which `hwnd` owns, can be the dialog that takes input aimed at
/// `hwnd`: visible, enabled, listed (named, as `list_windows` describes it) and activatable. A
/// window with `WS_EX_NOACTIVATE` can never be activated, so it cannot be a modal dialog; it is
/// an owned popup host such as the `Xaml_WindowedPopupClass` window (its title is localized, e.g. in Chinese Windows) a XAML menu
/// leaves visible and enabled after it closed. `listed` is unchanged: `list_windows` still lists it.
unsafe fn is_dialog_of(window: HWND, hwnd: HWND) -> bool {
    owned_by(window, hwnd)
        && IsWindowVisible(window).as_bool()
        && IsWindowEnabled(window).as_bool()
        && GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0 == 0
        && listed(window)
}

/// The target counts as in front when it, or a window it owns (its dialog), holds the foreground.
/// Bringing a window with an open modal dialog forward hands the foreground to the dialog.
unsafe fn in_front(hwnd: HWND) -> bool {
    let foreground = GetForegroundWindow();
    foreground == hwnd || owned_by(foreground, hwnd)
}

/// The dialog that would take input aimed at `hwnd`: the foreground window when `hwnd` owns it,
/// or, when `hwnd` itself is in front but disabled by a modal dialog, the frontmost visible and
/// enabled window it owns (`is_dialog_of`: not one that cannot be activated, ticket 154).
/// (GetLastActivePopup is no help there: activating `hwnd` makes it its own last active popup.)
/// Only a window `get_window` describes is named, so the dialog it reports can always be
/// targeted.
unsafe fn open_dialog(hwnd: HWND) -> Option<HWND> {
    let foreground = GetForegroundWindow();
    if owned_by(foreground, hwnd) && listed(foreground) {
        return Some(foreground);
    }
    if IsWindowEnabled(hwnd).as_bool() {
        return None;
    }
    top_level_windows()
        .ok()?
        .into_iter()
        .find(|window| is_dialog_of(*window, hwnd))
}

/// Ticket 64 (M1): the target made ready for an action, as `click` and the element actions
/// (`uia::set_value`, `uia::element_action`) do first: input is marked busy (ending a key press still waiting in the
/// background), the target is brought to the foreground and must hold it without an open dialog
/// (`ready_for_input`), and a stop ends the action here. Input stays marked busy while this lives.
pub(crate) struct Prepared {
    _busy: InputBusy,
}

pub(crate) unsafe fn prepare_target(hwnd: HWND) -> Result<Prepared, Failure> {
    let busy = InputBusy::enter();
    before_input(foreground(hwnd).map_err(foreground_failure))?;
    before_input(ready_for_input(hwnd))?;
    before_input(check_stop())?;
    Ok(Prepared { _busy: busy })
}

/// Ticket 137: `prepare_target`'s checks without bringing the target to the foreground, for
/// background delivery. Input is marked busy as there, then the target must be open, visible and
/// answering (`ensure_target_responds`, the first steps of `foreground`), have no open dialog
/// whoever holds the foreground (`ensure_no_dialog_anywhere`), and a stop ends the action here.
/// Nothing is raised, restored or sent, and the foreground window is not read or changed. Like
/// `prepare_target`, the `expected_app` check is the caller's, in `hwnd_from`. Input stays marked
/// busy while the result lives.
/// Ticket 138: a minimized target is `background_unavailable` (`minimized`): the user cannot see
/// it and a screenshot cannot check the result, and restoring it would change the screen.
pub(crate) unsafe fn prepare_background(hwnd: HWND) -> Result<Prepared, Failure> {
    let busy = InputBusy::enter();
    before_input(ensure_target_responds(hwnd).map_err(foreground_failure))?;
    if IsIconic(hwnd).as_bool() {
        return Err(background_unavailable(BackgroundReason::Minimized));
    }
    before_input(ensure_no_dialog_anywhere(hwnd))?;
    before_input(check_stop())?;
    Ok(Prepared { _busy: busy })
}

/// Test builds only (ticket 137): runs `prepare_background` on `params`' window and releases it,
/// so a test can see what it reports for a window that is not in front, and that it leaves the
/// foreground alone.
#[cfg(debug_assertions)]
pub fn debug_prepare_background(params: &Value) -> Result<Value, Failure> {
    let hwnd = hwnd_from(params)?;
    let _prepared = unsafe { prepare_background(hwnd) }?;
    Ok(serde_json::json!({}))
}

/// Ticket 138 (spec-background "tool interface"): how a request's action reaches the target, from its
/// `delivery_mode`. `Background` makes the target ready with `prepare_background` (nothing raised,
/// no cursor move, no SendInput); `Foreground` with `prepare_target`, as before background
/// delivery. A request without `delivery_mode` (from a server that does not send it) is foreground.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Delivery {
    Background,
    Foreground,
}

impl Delivery {
    pub(crate) fn from_params(params: &Value) -> Result<Delivery, Failure> {
        match params.get("delivery_mode") {
            None | Some(Value::Null) => Ok(Delivery::Foreground),
            Some(Value::String(mode)) if mode == "background" => Ok(Delivery::Background),
            Some(Value::String(mode)) if mode == "foreground" => Ok(Delivery::Foreground),
            Some(_) => Err(("invalid_params".to_string(), "delivery_mode must be \"background\" or \"foreground\".".to_string()).into()),
        }
    }

    /// The target made ready for this delivery (`prepare_background` or `prepare_target`).
    pub(crate) unsafe fn prepare(self, hwnd: HWND) -> Result<Prepared, Failure> {
        match self {
            Delivery::Background => prepare_background(hwnd),
            Delivery::Foreground => prepare_target(hwnd),
        }
    }
}

/// spec-background "`background_unavailable`": why an action cannot be delivered in the background.
/// The server words the same reasons (`src/delivery.ts`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum BackgroundReason {
    // `ToolNeedsForeground` is the foreground-only tool ticket's (142).
    NeedsPointer,
    NoClickPattern,
    NoElementAtPoint,
    FocusNeedsForeground,
    Minimized,
    #[allow(dead_code)]
    ToolNeedsForeground,
}

impl BackgroundReason {
    fn name(self) -> &'static str {
        match self {
            BackgroundReason::NeedsPointer => "needs_pointer",
            BackgroundReason::NoClickPattern => "no_click_pattern",
            BackgroundReason::NoElementAtPoint => "no_element_at_point",
            BackgroundReason::FocusNeedsForeground => "focus_needs_foreground",
            BackgroundReason::Minimized => "minimized",
            BackgroundReason::ToolNeedsForeground => "tool_needs_foreground",
        }
    }

    fn why(self) -> &'static str {
        match self {
            BackgroundReason::NeedsPointer => "This click needs the real pointer (a right click, a double click or held modifiers), so it cannot be delivered in the background",
            BackgroundReason::NoClickPattern => "The element supports none of Invoke, Toggle, SelectionItem or ExpandCollapse, so a click on it cannot be delivered in the background",
            BackgroundReason::NoElementAtPoint => "No element that can be acted on was found at that point of the target window, so the click cannot be delivered in the background",
            BackgroundReason::FocusNeedsForeground => "Moving the keyboard focus brings the window forward, so focus cannot be delivered in the background",
            BackgroundReason::Minimized => "The target window is minimized, so the action cannot be delivered in the background",
            BackgroundReason::ToolNeedsForeground => "This tool needs real keyboard or pointer input, so it cannot be delivered in the background",
        }
    }
}

/// `background_unavailable` for `reason`, with `data` `{ reason, recommended: "foreground" }`:
/// checked before any input, so nothing was sent (`input_sent: false`) and nothing was raised.
pub(crate) fn background_unavailable(reason: BackgroundReason) -> Failure {
    Failure {
        code: "background_unavailable".to_string(),
        message: format!("{}; nothing was sent. Retry this action with delivery_mode: \"foreground\".", reason.why()),
        data: Some(serde_json::json!({ "reason": reason.name(), "recommended": "foreground" })),
    }
    .with_input_sent(false)
}

/// Checked after `foreground` and before any input: input goes to the target itself only. When
/// the target's dialog holds the foreground, the error names the dialog so it can be targeted.
unsafe fn ready_for_input(hwnd: HWND) -> Result<(), Failure> {
    ensure_no_dialog(hwnd)?;
    if GetForegroundWindow() != hwnd {
        return Err(("input_failed".to_string(), "The target window is not in the foreground, so no input was sent.".to_string()).into());
    }
    Ok(())
}

/// `dialog_open`, naming the dialog, when the target has one open (`open_dialog`): the input, or
/// the activation, would go to the dialog rather than to the target.
unsafe fn ensure_no_dialog(hwnd: HWND) -> Result<(), Failure> {
    let Some(dialog) = open_dialog(hwnd) else { return Ok(()) };
    Err(dialog_open(Some(dialog)))
}

/// Ticket 137: whether the target has an open dialog, judged without the foreground window (which
/// is not the target's when the action is delivered in the background): the frontmost window the
/// target owns that is visible, enabled, listed (named, as `list_windows` describes it) and
/// activatable (`is_dialog_of`; ticket 154: a `WS_EX_NOACTIVATE` popup host is no dialog), else
/// the target being disabled, which a modal dialog the helper cannot name also causes
/// (`Some(None)`). `None` when neither holds.
unsafe fn blocking_dialog(hwnd: HWND) -> Option<Option<HWND>> {
    let named = top_level_windows()
        .ok()
        .and_then(|windows| windows.into_iter().find(|window| is_dialog_of(*window, hwnd)));
    match named {
        Some(dialog) => Some(Some(dialog)),
        None if !IsWindowEnabled(hwnd).as_bool() => Some(None),
        None => None,
    }
}

/// `ensure_no_dialog` for background delivery (`blocking_dialog`): `dialog_open`, naming the
/// dialog when there is one to name.
unsafe fn ensure_no_dialog_anywhere(hwnd: HWND) -> Result<(), Failure> {
    match blocking_dialog(hwnd) {
        Some(dialog) => Err(dialog_open(dialog)),
        None => Ok(()),
    }
}

/// The `dialog_open` failure: names `dialog` (`window_id`, `title`) when there is one; without
/// one it says the target is disabled and carries no `window_id`.
unsafe fn dialog_open(dialog: Option<HWND>) -> Failure {
    let Some(dialog) = dialog else {
        return Failure {
            code: "dialog_open".to_string(),
            message: "The target window is disabled, so it has a dialog open that could not be named, and no input was sent. Ask the user to close the dialog.".to_string(),
            data: None,
        };
    };
    let window_id = (dialog.0 as isize).to_string();
    let title = window_text(dialog);
    Failure {
        code: "dialog_open".to_string(),
        message: format!("The target window has an open dialog (window_id {window_id}, title \"{title}\"), so no input was sent. Target the dialog instead."),
        data: Some(serde_json::json!({ "window_id": window_id, "title": title })),
    }
}

/// Checked after `ready_for_input` and before any pointer input: each screen point must be on a
/// monitor. A window can stick out past every screen (above the topmost monitor, beyond an outer
/// edge); Windows clamps absolute pointer input aimed there to the nearest screen edge, where it
/// would land on another part of the window or on another window. `WindowFromPoint` still names
/// the target there, so `ensure_unobscured` does not catch it.
unsafe fn ensure_on_screen(points: &[(i32, i32)]) -> Result<(), Failure> {
    for &(x, y) in points {
        if MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL).is_invalid() {
            return Err((
                "offscreen".to_string(),
                format!(
                    "Screen point ({x}, {y}) is outside every screen: that part of the window is off screen, so no input was sent. Use a point in the part of the window that is on a screen, or ask the user to move the window back onto a screen."
                ),
            )
                .into());
        }
    }
    Ok(())
}

/// Checked after `ensure_on_screen` and before any pointer input: each screen point must reach the
/// target. The window under the point (`WindowFromPoint`, which skips click-through windows such
/// as our overlay, like real pointer input does) must have the target as its root, a root the
/// target owns (its dialog, menu, or tooltip), or a root that is one of the target's own popups
/// above it, as the screenshot shows them (`popup_above`: a menu, tooltip, or dropdown of the
/// target's UI thread, often unowned, like Explorer's context menu). Otherwise another window,
/// typically an always-on-top one from another app, covers the point and would receive the input.
unsafe fn ensure_unobscured(hwnd: HWND, points: &[(i32, i32)]) -> Result<(), Failure> {
    for &(x, y) in points {
        let hit = WindowFromPoint(POINT { x, y });
        let root = if hit.0.is_null() { hit } else { GetAncestor(hit, GA_ROOT) };
        if !root.0.is_null() && (root == hwnd || owned_by(root, hwnd) || popup_above(root, hwnd).is_some()) {
            continue;
        }
        if root.0.is_null() {
            return Err(("target_obscured".to_string(), format!("No window is at screen point ({x}, {y}), so no input was sent.")).into());
        }
        let window_id = (root.0 as isize).to_string();
        let title = window_text(root);
        let (app, app_name) = window_app(root).unwrap_or_else(|| (String::new(), "an unknown app".to_string()));
        return Err(Failure {
            code: "target_obscured".to_string(),
            message: format!("The point is covered by a window of {app_name} (window_id {window_id}, title \"{title}\"), so no input was sent."),
            data: Some(serde_json::json!({ "window_id": window_id, "title": title, "app": app, "app_name": app_name, "topmost": is_topmost(root) })),
        });
    }
    Ok(())
}

/// Checked before every press or move of click, drag and scroll: the target, or its dialog,
/// still holds the foreground and the target is still open. Once that fails nothing more is
/// sent (releases excepted) and the call fails, saying what was not sent.
unsafe fn still_target(hwnd: HWND) -> bool {
    !target_closed(hwnd) && in_front(hwnd)
}

/// `input_failed` for a target that closed or lost the foreground before the last `unit` of the
/// `action` was sent: `done` of `total` were sent, the rest were not.
unsafe fn lost_partway(hwnd: HWND, action: &str, unit: &str, done: u64, total: u64) -> Failure {
    let what = if target_closed(hwnd) { "closed" } else { "lost the foreground" };
    let message = if done == 0 {
        format!("The target window {what} before the {action}, so no {unit} was sent.")
    } else {
        format!("The target window {what} after {unit} {done} of {total}; the remaining {} were not sent.", total - done)
    };
    ("input_failed".to_string(), message).into()
}

/// The success result of an input whose every event was sent while the target held the
/// foreground, and which the target then read. When the foreground is no longer the target or
/// its dialog, or the target closed, `foreground_changed` says so: `target_closed` (closed or
/// hidden) and `foreground`, the window now in front as `list_windows` describes it
/// (`window_id`, `title`, `app`, `app_name`), or null when no window `list_windows` lists is in
/// front. The input may have caused the change (a button that opens a window, Enter closing a
/// dialog) or not (another app taking the foreground just then); either way it reached the
/// target, so the call succeeds and the caller looks at the new foreground before going on.
/// Element actions (`uia::set_value`, `uia::element_action`) end with it too (M1).
pub(crate) unsafe fn action_result(hwnd: HWND) -> Value {
    let closed = target_closed(hwnd);
    if !closed && in_front(hwnd) {
        return serde_json::json!({});
    }
    foreground_changed(closed)
}

/// `action_result`'s `foreground_changed`, naming the window in front now.
unsafe fn foreground_changed(closed: bool) -> Value {
    let window = GetForegroundWindow();
    let foreground = if window.0.is_null() || !listed(window) {
        Value::Null
    } else {
        let (app, app_name) = window_app(window).unwrap_or_else(|| (String::new(), "an unknown app".to_string()));
        serde_json::json!({ "window_id": (window.0 as isize).to_string(), "title": window_text(window), "app": app, "app_name": app_name })
    };
    serde_json::json!({ "foreground_changed": { "foreground": foreground, "target_closed": closed } })
}

/// Ticket 138: `action_result` for an action delivered in the background, where the target was
/// never in front. The foreground is compared with `before`, the window in front when the action
/// began: when another window holds it now (another app took it), or the target closed,
/// `foreground_changed` says so in `action_result`'s shape. `restore` is what the foreground
/// restore reports (`restore::report`): `foreground_restored: true` when the target took the
/// foreground and it was given back; `foreground_changed` with `restore_failed: true` when it
/// could not be (the action itself was done).
pub(crate) unsafe fn background_action_result(hwnd: HWND, before: HWND, restore: crate::restore::Report) -> Value {
    use crate::restore::Report;
    let closed = target_closed(hwnd);
    // By root, not by handle: a control inside the user's window (the Win32 proxy's child, a UWP
    // CoreWindow) in front is still the user's window in front.
    let user_in_front = crate::restore::foreground_inside(before);
    let mut result = if restore != Report::RestoreFailed && !closed && user_in_front {
        serde_json::json!({})
    } else {
        foreground_changed(closed)
    };
    match restore {
        Report::Restored => result["foreground_restored"] = Value::Bool(true),
        Report::RestoreFailed => result["restore_failed"] = Value::Bool(true),
        Report::Unchanged => {}
    }
    result
}

/// Succeeds once the target is in front and raised. When other windows still cover part of it
/// (always-on-top ones, which raising cannot pass), the result names them in `covered_by`: input
/// at the points they cover is refused with `target_obscured`, the rest of the window works. A
/// target with an open dialog fails with `dialog_open` naming the dialog, as input to it does
/// (F20): bringing it forward hands the foreground to the dialog, which is the window to target.
pub fn activate_window(params: &Value) -> Result<Value, Failure> {
    let hwnd = hwnd_from(params)?;
    let _busy = InputBusy::enter();
    unsafe { foreground(hwnd) }.map_err(foreground_failure)?;
    unsafe { ensure_no_dialog(hwnd) }?;
    let covered_by: Vec<Value> = unsafe { covering_windows(hwnd) }.into_iter().map(|window| unsafe { describe_cover(window) }).collect();
    if covered_by.is_empty() {
        return Ok(serde_json::json!({}));
    }
    Ok(serde_json::json!({ "covered_by": covered_by }))
}

/// The windows above the target in the z-order that still cover part of it: not its own (its
/// dialogs, and its popups as `popup_kind` names them: menus, tooltips, dropdowns; only windows
/// above the target are walked, so these are the popups `popup_above` and the screenshot count),
/// overlapping its visible frame, and found by `WindowFromPoint` at
/// one of a grid of points in that overlap, as pointer input would find them. Hidden, cloaked
/// and click-through windows (our overlay) are never found there, so they never count.
unsafe fn covering_windows(hwnd: HWND) -> Vec<HWND> {
    const STEPS: i32 = 8;
    let (Ok(frame), Ok(windows)) = (visible_frame(hwnd), top_level_windows()) else { return Vec::new() };
    let mut covering = Vec::new();
    // EnumWindows lists top-level windows from the top of the z-order down.
    for window in windows {
        if window == hwnd {
            break;
        }
        if owned_by(window, hwnd) || popup_kind(window, hwnd).is_some() || !IsWindowVisible(window).as_bool() {
            continue;
        }
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_err() {
            continue;
        }
        let (left, top) = (rect.left.max(frame.left), rect.top.max(frame.top));
        let (right, bottom) = (rect.right.min(frame.right), rect.bottom.min(frame.bottom));
        if left >= right || top >= bottom {
            continue;
        }
        let found_at = |x: i32, y: i32| {
            let hit = WindowFromPoint(POINT { x, y });
            !hit.0.is_null() && GetAncestor(hit, GA_ROOT) == window
        };
        let found = (0..=STEPS).any(|i| (0..=STEPS).any(|j| found_at(left + (right - 1 - left) * i / STEPS, top + (bottom - 1 - top) * j / STEPS)));
        if found {
            covering.push(window);
        }
    }
    covering
}

/// A covering window as `target_obscured` describes one.
unsafe fn describe_cover(window: HWND) -> Value {
    let (app, app_name) = window_app(window).unwrap_or_else(|| (String::new(), "an unknown app".to_string()));
    serde_json::json!({ "window_id": (window.0 as isize).to_string(), "title": window_text(window), "app": app, "app_name": app_name, "topmost": is_topmost(window) })
}

/// Whether the window stays above ordinary ones (`WS_EX_TOPMOST`): raising the target cannot pass
/// it. A covering window that is not topmost is one the raise failed to pass (UIPI, foreground rights).
unsafe fn is_topmost(window: HWND) -> bool {
    GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0
}

/// One wheel notch (`WHEEL_DELTA`).
const WHEEL_NOTCH: i32 = 120;
/// Pause between notches, so the target takes them one by one instead of finding a full queue.
/// The most the server allows, 50 notches, spends 49 x 15 ms = 735 ms here, plus the 300 ms
/// `flush_target`: about 1.1 s of the 10 s request timeout (`DEFAULT_REQUEST_TIMEOUT_MS`).
const WHEEL_PACE: std::time::Duration = std::time::Duration::from_millis(15);

pub fn scroll(params: &Value) -> Result<Value, Failure> {
    let hwnd = before_input(hwnd_from(params))?;
    let x = required_i32(params, "x")?;
    let y = required_i32(params, "y")?;
    let direction = params.get("direction").and_then(|value| value.as_str()).unwrap_or("");
    // Absent means the default; anything else must be a positive whole number, never silently 3.
    let amount = match params.get("amount") {
        None => 3,
        Some(value) => value
            .as_i64()
            .filter(|amount| *amount > 0)
            .ok_or_else(|| ("invalid_params".to_string(), "amount must be a positive whole number of wheel notches.".to_string()))?,
    };
    let (flags, data) = match direction {
        "up" => (MOUSEEVENTF_WHEEL, WHEEL_NOTCH),
        "down" => (MOUSEEVENTF_WHEEL, -WHEEL_NOTCH),
        "right" => (MOUSEEVENTF_HWHEEL, WHEEL_NOTCH),
        "left" => (MOUSEEVENTF_HWHEEL, -WHEEL_NOTCH),
        _ => return Err(("invalid_params".to_string(), "direction must be up, down, left, or right.".to_string()).into()),
    };
    let (screen_x, screen_y) = point_in_window(hwnd, x, y)?;
    let _busy = InputBusy::enter();
    unsafe {
        before_input(foreground(hwnd).map_err(foreground_failure))?;
        before_input(ready_for_input(hwnd))?;
        ensure_on_screen(&[(screen_x, screen_y)])?;
        ensure_unobscured(hwnd, &[(screen_x, screen_y)])?;
        check_stop()?;
        send_mouse(screen_x, screen_y, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).map_err(|message| ("input_failed".to_string(), message))?;
        // One event per notch, like a physical wheel: an app caps what a single large event
        // scrolls (Chrome moved a page about 165 px for 50 notches in one event). A stop ends
        // the scroll between notches.
        for notch in 0..amount {
            if notch > 0 {
                std::thread::sleep(WHEEL_PACE);
            }
            check_stop()?;
            if !still_target(hwnd) {
                return Err(lost_partway(hwnd, "scroll", "wheel notch", notch as u64, amount as u64));
            }
            send_wheel(screen_x, screen_y, flags, data).map_err(|message| ("input_failed".to_string(), message))?;
        }
        flush_target(hwnd);
        Ok(action_result(hwnd))
    }
}

pub fn drag(params: &Value) -> Result<Value, Failure> {
    let hwnd = before_input(hwnd_from(params))?;
    let from_x = required_i32(params, "from_x")?;
    let from_y = required_i32(params, "from_y")?;
    let to_x = required_i32(params, "to_x")?;
    let to_y = required_i32(params, "to_y")?;
    let modifiers = modifiers(params)?;
    let start = point_in_window(hwnd, from_x, from_y)?;
    let end = point_in_window(hwnd, to_x, to_y)?;
    // Every point the pointer is sent to, in order: the press at the start, then five moves
    // ending at the end. All are checked before anything is sent.
    let path: Vec<(i32, i32)> = std::iter::once(start)
        .chain((1..=5).map(|step| (start.0 + (end.0 - start.0) * step / 5, start.1 + (end.1 - start.1) * step / 5)))
        .collect();
    let _busy = InputBusy::enter();
    unsafe {
        before_input(foreground(hwnd).map_err(foreground_failure))?;
        before_input(ready_for_input(hwnd))?;
        ensure_on_screen(&path)?;
        ensure_unobscured(hwnd, &[start, end])?;
        check_stop()?;
        // Attached before anything is pressed: attaching resets the key state it follows.
        let reads = PointerReads::follow(&[start, end], "left");
        // Held for the whole drag and released on every exit, after the button (locals drop in
        // reverse order), so the target sees the modifiers on both the press and the release.
        let held_modifiers = HeldKeys::press(&modifiers);
        check_stop()?;
        send_mouse(start.0, start.1, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).map_err(|message| ("input_failed".to_string(), message))?;
        check_stop()?;
        if !still_target(hwnd) {
            return Err(lost_during_drag(hwnd, None));
        }
        // Ticket 73: the press starts the drag's action; once it is sent, a stop says so (`esc::act`).
        act(|| send_mouse(start.0, start.1, MOUSEEVENTF_LEFTDOWN | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK))?.map_err(|message| ("input_failed".to_string(), message))?;
        // From here on every exit, including a stop, releases the button where the pointer is.
        let mut held = HeldLeftButton { at: start };
        let moves = path.len() - 1;
        for (done, &(x, y)) in path[1..].iter().enumerate() {
            check_stop()?;
            if !still_target(hwnd) {
                return Err(lost_during_drag(hwnd, Some((done, moves))));
            }
            send_mouse(x, y, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).map_err(|message| ("input_failed".to_string(), message))?;
            held.at = (x, y);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // The release is the drop, the drag's last event: the target must still be in front.
        if !still_target(hwnd) {
            return Err(lost_during_drag(hwnd, Some((moves, moves))));
        }
        drop(held);
        if let Some(reads) = &reads {
            reads.sent_click();
        }
        drop(held_modifiers);
        let read = confirm_read(hwnd, reads)?;
        Ok(pointer_result(hwnd, read))
    }
}

/// `input_failed` for a drag whose target closed or lost the foreground before the button went
/// down (`None`), or after `done` of `moves` moves; a pressed button is released where the
/// pointer is either way.
unsafe fn lost_during_drag(hwnd: HWND, progress: Option<(usize, usize)>) -> Failure {
    let what = if target_closed(hwnd) { "closed" } else { "lost the foreground" };
    let message = match progress {
        None => format!("The target window {what} before the drag, so the button was not pressed."),
        Some((done, moves)) if done < moves => format!(
            "The target window {what} during the drag, after {done} of {moves} moves; the remaining moves were not sent, and the button was released where the pointer was."
        ),
        Some(_) => format!("The target window {what} during the drag, before the button was released at its end; the button was released there anyway."),
    };
    ("input_failed".to_string(), message).into()
}

/// The left button pressed by a drag; released at the last pointer position when dropped.
struct HeldLeftButton {
    at: (i32, i32),
}

impl Drop for HeldLeftButton {
    fn drop(&mut self) {
        unsafe {
            let _ = send_mouse(self.at.0, self.at.1, MOUSEEVENTF_LEFTUP | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK);
        }
    }
}

pub fn move_mouse(params: &Value) -> Result<Value, Failure> {
    let hwnd = before_input(hwnd_from(params))?;
    let x = required_i32(params, "x")?;
    let y = required_i32(params, "y")?;
    let (screen_x, screen_y) = point_in_window(hwnd, x, y)?;
    let _busy = InputBusy::enter();
    unsafe {
        before_input(foreground(hwnd).map_err(foreground_failure))?;
        before_input(ready_for_input(hwnd))?;
        ensure_on_screen(&[(screen_x, screen_y)])?;
        ensure_unobscured(hwnd, &[(screen_x, screen_y)])?;
        check_stop()?;
        send_mouse(screen_x, screen_y, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).map_err(|message| ("input_failed".to_string(), message))?;
        // The move, its one event, was sent while the target held the foreground (checked just
        // before): a foreground change after it is no failure, as for the other inputs (ticket 49).
        flush_target(hwnd);
        Ok(action_result(hwnd))
    }
}

/// How long one `type_text` call keeps starting new characters, counted from the request. The
/// server kills the helper when `type_text` runs past its timeout (`TYPE_TEXT_TIMEOUT_MS`, 30 s,
/// in `src/config.ts`), and then nothing restores the keyboard layout. Worst case: the last
/// character starts just before this budget, and waits for the target end at `READ_LIMIT` after
/// it (`TargetQueue::deadline`), so typing ends by 25 s; confirming the target handled the last
/// key (`settle`, 0.4 s) and restoring the layout (0.5 s) take at most another second, which
/// leaves about 4 s of the server's timeout unused.
const TYPE_TEXT_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

pub fn type_text(params: &Value) -> Result<Value, Failure> {
    let started = std::time::Instant::now();
    let hwnd = before_input(hwnd_from(params))?;
    let text = params.get("text").and_then(|value| value.as_str()).ok_or_else(|| ("invalid_params".to_string(), "text is required.".to_string()))?;
    // Counted in characters of the text as sent, CR included, so "typed N" is where the rest
    // of the text starts.
    let total = text.chars().count();
    let _busy = InputBusy::enter();
    unsafe {
        before_input(foreground(hwnd).map_err(foreground_failure))?;
        before_input(ready_for_input(hwnd))?;
        let input = keyboard_window(hwnd)?;
        ensure_focus_not_password()?;
        if input != hwnd {
            return type_unconfirmed(hwnd, input, text, started);
        }
        let _layout = if text.chars().any(|character| !character.is_ascii()) {
            Some(EnglishLayout::switch(input).map_err(|message| {
                if stop_requested() { interrupted() } else { ("input_failed".to_string(), message) }
            })?)
        } else {
            None
        };
        // Detached before the layout is restored (locals drop in reverse order).
        let mut queue = TargetQueue::attach(input)?;
        queue.deadline = Some(started + TYPE_TEXT_BUDGET + READ_LIMIT);
        for (typed, character) in text.chars().enumerate() {
            if character == '\r' {
                continue;
            }
            check_stop()?;
            ensure_typing_target(hwnd, input, typed, total)?;
            if started.elapsed() >= TYPE_TEXT_BUDGET {
                return Err(typing_budget_spent(typed, total));
            }
            // The whole character, both halves of a surrogate pair included, goes in one
            // SendInput (`Key`), so the counts below are in characters as the text has them.
            let unread = match queue.type_key(hwnd, character_key(character)) {
                Ok(()) => {
                    if may_move_focus(character) {
                        // Handled, so a focus change it made has happened.
                        queue.settle();
                        ensure_focus_stays_off_password(typed + 1, total)?;
                    }
                    continue;
                }
                Err(unread) => unread,
            };
            let limit = match unread {
                Unread::Stalled => format!("within {} seconds", READ_LIMIT.as_secs()),
                Unread::OutOfTime => format!("before the {}-second limit for one call", TYPE_TEXT_BUDGET.as_secs()),
                Unread::LostForeground => "after it lost the foreground".to_string(),
                other => return Err(other.into()),
            };
            // Every character before this one was confirmed read.
            return Err((
                "input_failed".to_string(),
                format!(
                    "The target window did not read character {next} of {total} {limit}; {typed} of {total} characters were typed. Character {next} was sent but not confirmed read, and the remaining {} were not sent.",
                    total - typed - 1,
                    next = typed + 1,
                ),
            )
                .into());
        }
        queue.finish();
        drop(queue);
        // Every character was read: a target the text closed (Enter in a dialog) or that gave up
        // the foreground after the last one is no failure.
        Ok(action_result(hwnd))
    }
}

fn typing_budget_spent(typed: usize, total: usize) -> Failure {
    (
        "input_failed".to_string(),
        format!(
            "Typed {typed} of {total} characters before the {}-second limit for one call; the remaining {} were not sent. Send them with another type_text call.",
            TYPE_TEXT_BUDGET.as_secs(),
            total - typed
        ),
    )
        .into()
}

/// Ticket 44: `type_text` for a target whose keys another process's window reads (`input`, the
/// target's `keyboard_window` when it is not the target itself: a UWP app's CoreWindow inside its
/// ApplicationFrameHost.exe frame). Windows refuses `AttachThreadInput` to that thread (and to the
/// frame host's), so `TargetQueue` cannot follow it and nothing here confirms the app read a key.
/// Every other check stays: before each key the target holds the foreground, the app's window is
/// still inside it, input was not stopped and the call's time is not up, and once one of those
/// fails nothing more is sent. After each character is sent, the next one waits until the app
/// answers two sent messages in a row (`wait_for_answers`), so keys go no faster than it takes
/// them. The result says `read_confirmed: false`; normal windows never take this path, and their
/// result has no such field.
///
/// Text goes as `KEYEVENTF_UNICODE` (Enter as VK_RETURN), as for any window, and the app's thread
/// is switched to the English layout for any text, ASCII included (`EnglishLayout`, which needs no
/// attach). A `Windows.UI.Core.CoreWindow` with the zh-TW input method open hands every typed
/// character to the input method (VK_PROCESSKEY), digits too, and gets it back from there only
/// later, so the app would answer before it has the character and the pacing would run ahead of
/// it; with the English layout there is no input method in between. An app that does not take the
/// layout request within its 500 ms is typed into on its own layout rather than failed, and its
/// success result also says `layout_switched: false` (an input method there may have composed or
/// held the characters; failures do not carry it). The layout is restored on every path.
pub(crate) unsafe fn type_unconfirmed(hwnd: HWND, input: HWND, text: &str, started: std::time::Instant) -> Result<Value, Failure> {
    let total = text.chars().count();
    let mut layout_not_switched = false;
    let _layout = if text.chars().any(|character| character != '\r' && character != '\n') {
        match EnglishLayout::switch(input) {
            Ok(layout) => Some(layout),
            Err(_) if stop_requested() => return Err(interrupted().into()),
            Err(message) => {
                eprintln!("keyboard layout: typing into the app without the English layout ({message})");
                layout_not_switched = true;
                None
            }
        }
    } else {
        None
    };
    let deadline = started + TYPE_TEXT_BUDGET + READ_LIMIT;
    for (typed, character) in text.chars().enumerate() {
        if character == '\r' {
            continue;
        }
        check_stop()?;
        ensure_typing_target(hwnd, input, typed, total)?;
        if started.elapsed() >= TYPE_TEXT_BUDGET {
            return Err(typing_budget_spent(typed, total));
        }
        // The whole character, both halves of a surrogate pair included, goes in one SendInput
        // (`Key`), and the wait comes after it.
        if !character_key(character).send() {
            return Err(Unread::NotSent.into());
        }
        if let Err(unread) = wait_for_answers(hwnd, input, deadline) {
            let limit = match unread {
                Unread::Stalled => format!("within {} seconds", READ_LIMIT.as_secs()),
                Unread::OutOfTime => format!("before the {}-second limit for one call", TYPE_TEXT_BUDGET.as_secs()),
                other => return Err(other.into()),
            };
            let sent = typed + 1;
            return Err((
                "input_failed".to_string(),
                format!(
                    "The app in the target window did not respond {limit} after character {sent} of {total} was sent; {sent} of {total} characters were sent, without confirmation that the app read them, and the remaining {} were not sent.",
                    total - sent
                ),
            )
                .into());
        }
        if may_move_focus(character) {
            ensure_focus_stays_off_password(typed + 1, total)?;
        }
    }
    let mut result = unconfirmed_result(hwnd, input);
    if layout_not_switched {
        result["layout_switched"] = Value::Bool(false);
    }
    Ok(result)
}

/// Ticket 44: `press_key` for a target whose keys another process's window reads, as
/// `type_unconfirmed` does for text: each key is pressed only after the checks before it pass and
/// once the app answered after the one before; then all are released in reverse order (on every
/// path, stop and failure included), and the app gets until it answers again before the input
/// method is reopened. `ImeGuard` closes the app's input method as for any window: it sends to the
/// app's input method window and needs no attach (an app without one has nothing to close).
pub(crate) unsafe fn press_unconfirmed(hwnd: HWND, input: HWND, keys: &[VIRTUAL_KEY], started: std::time::Instant) -> Result<Value, Failure> {
    let _ime = ImeGuard::suspend(input)?;
    let deadline = started + PRESS_KEY_LIMIT;
    let total = keys.len();
    let mut held = HeldKeys::default();
    let mut pressed = Ok(());
    for (index, key) in keys.iter().enumerate() {
        let next = check_stop().and_then(|()| ensure_key_target(hwnd, input)).and_then(|()| {
            if std::time::Instant::now() >= deadline {
                Err(unanswered_key_press(Unread::OutOfTime, index, total))
            } else if !held.press_one(*key) {
                Err(key_press_failure(Unread::NotSent))
            } else {
                wait_for_answers(hwnd, input, deadline).map_err(|unread| unanswered_key_press(unread, index + 1, total))
            }
        });
        if next.is_err() {
            pressed = next;
            break;
        }
    }
    let any_pressed = !held.keys.is_empty();
    drop(held);
    if any_pressed && !stop_requested() {
        // The releases are the last events; the input method is reopened only after the app took
        // them. After a failure the app gets until the drain limit, and the failure stands.
        match pressed {
            Ok(()) => pressed = wait_for_answers(hwnd, input, deadline).map_err(|unread| unanswered_key_press(unread, total, total)),
            Err(_) => {
                let _ = wait_for_answers(hwnd, input, started + PRESS_KEY_DRAIN_LIMIT);
            }
        }
    }
    pressed?;
    Ok(unconfirmed_result(hwnd, input))
}

/// `press_unconfirmed`'s failure when the app did not answer after `pressed` of `total` keys.
fn unanswered_key_press(unread: Unread, pressed: usize, total: usize) -> Failure {
    let limit = match unread {
        Unread::Stalled => format!("within {} seconds", READ_LIMIT.as_secs()),
        Unread::OutOfTime => format!("within the {}-second limit for one call", PRESS_KEY_LIMIT.as_secs()),
        other => return key_press_failure(other),
    };
    let message = if pressed == total {
        format!("The app in the target window did not respond {limit} after the key press; every key was pressed and released, without confirmation that the app read them.")
    } else if pressed == 0 {
        format!("The app in the target window did not respond {limit}, so no key was sent.")
    } else {
        format!(
            "The app in the target window did not respond {limit} after key {pressed} of {total} was pressed; the keys pressed so far were released, and the remaining {} were not sent.",
            total - pressed
        )
    };
    ("input_failed".to_string(), message).into()
}

/// The success result of `type_unconfirmed` and `press_unconfirmed` (ticket 44): two more sent
/// messages first (`answers_twice`; not answering is no failure here), as `TargetQueue::settle`
/// does after the last key, so the app has most likely handled what its last answer may have come
/// before, and a window it opened over the last key shows in `foreground_changed`; then
/// `action_result`, marked `read_confirmed: false`.
unsafe fn unconfirmed_result(hwnd: HWND, input: HWND) -> Value {
    let _ = answers_twice(input);
    let mut result = action_result(hwnd);
    result["read_confirmed"] = Value::Bool(false);
    result
}

/// Ticket 44: after a key for an app whose input queue cannot be followed, waits until `input`'s
/// thread answers two sent messages in a row (`target_answers`, as `wait_typed`'s fallback judges a
/// key read): having come back to its message loop twice, it has most likely taken the key queued
/// before them. That is pacing, not confirmation. Ends with `Stalled` when it does not answer so
/// for `READ_LIMIT` (each ask can take up to 0.4 s, so that much later), `OutOfTime` at
/// `deadline`, and `Stopped` on a stop. Ends at once when the target or `input` is gone: the check
/// before the next key reports that, and after the last key it is no failure. A foreground change
/// does not end it: the keys already sent are in the app's queue either way, and the check before
/// the next key stops the rest.
unsafe fn wait_for_answers(hwnd: HWND, input: HWND, deadline: std::time::Instant) -> Result<(), Unread> {
    let started = std::time::Instant::now();
    loop {
        if stop_requested() {
            return Err(Unread::Stopped);
        }
        if target_closed(hwnd) || !IsWindow(Some(input)).as_bool() {
            return Ok(());
        }
        if answers_twice(input) {
            return Ok(());
        }
        if started.elapsed() >= READ_LIMIT {
            return Err(Unread::Stalled);
        }
        if std::time::Instant::now() >= deadline {
            return Err(Unread::OutOfTime);
        }
    }
}

/// How long `answers_twice` pauses before each ask.
const ANSWER_GAP: std::time::Duration = std::time::Duration::from_millis(1);

/// Whether `input`'s thread answers two sent messages in a row, each asked `ANSWER_GAP` after the
/// one before (and after the key). A thread handles a sent message ahead of anything posted to it
/// and of its input, and SendInput hands the key to it through the system's input thread, so asks
/// sent back to back can all be answered before it took the key or the character TranslateMessage
/// posted for it (measured with the fixture: typing ran two keys ahead of the app). The pause lets
/// it take those first.
unsafe fn answers_twice(input: HWND) -> bool {
    std::thread::sleep(ANSWER_GAP);
    if !target_answers(input) {
        return false;
    }
    std::thread::sleep(ANSWER_GAP);
    target_answers(input)
}

/// The target itself is gone or hidden: closed, as a dialog is by its Enter. Losing the
/// foreground to another window while the target stays open is not this.
unsafe fn target_closed(hwnd: HWND) -> bool {
    !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool()
}

/// Checked before every key of `type_text`: typing takes as long as the target
/// needs, and the text itself may close the target. Once it is closed, the rest would go to the
/// window that took the foreground; once it lost the foreground, to that window's app. `input` is
/// the target's `keyboard_window`.
unsafe fn ensure_typing_target(hwnd: HWND, input: HWND, typed: usize, total: usize) -> Result<(), Failure> {
    if target_closed(hwnd) {
        return Err((
            "input_failed".to_string(),
            format!("The target window closed after character {typed} of {total}; the remaining {} were not sent.", total - typed),
        )
            .into());
    }
    if !in_front(hwnd) {
        return Err((
            "input_failed".to_string(),
            format!("The target window lost the foreground after character {typed} of {total}; the remaining {} were not sent.", total - typed),
        )
            .into());
    }
    if input_left_target(hwnd, input) {
        return Err((
            "input_failed".to_string(),
            format!("The app inside the target window stopped taking its keyboard input after character {typed} of {total}; the remaining {} were not sent.", total - typed),
        )
            .into());
    }
    Ok(())
}

/// How long one `press_key` call waits for the target to read its keys, counted from the request.
/// The server kills the helper when `press_key` runs past its timeout (`DEFAULT_REQUEST_TIMEOUT_MS`,
/// 10 s, in `src/config.ts`), and then nothing reopens the input method `ImeGuard` closed. The
/// foreground (under 2 s), waiting out a reopen an earlier call on the same target left pending
/// (`IME_REQUEST_LIMIT_MS`, `ImeGuard::suspend`), and the two input method requests
/// (`IME_REQUEST_LIMIT_MS` each) come first, inside this limit: it is counted from the request, so
/// time any of them take is time the wait for keys below does not get, rather than added on top.
/// On success, confirming the target handled the keys (`settle`, 0.4 s) and reopening the input
/// method (`IME_REQUEST_LIMIT_MS`) follow, so the call ends by about 6 s.
const PRESS_KEY_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// When `press_key` fails after pressing keys, the keys are released and the target gets until
/// here, counted from the request, to read them (`press_key`), so it reads them while its key
/// state is still shared with the helper's thread and its input method is still closed. Then the
/// input method is reopened (`IME_REQUEST_LIMIT_MS`), so the call ends by about 7.5 s, 2.5 s before
/// the server's timeout. A target that has not read the last release by then gets the rest in the
/// background (`PRESS_KEY_BACKGROUND_LIMIT`, ticket 54): the call answers here at once.
const PRESS_KEY_DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_secs(7);

/// How long, counted from the request, the thread of a `press_key` that answered before its
/// target read the last release keeps waiting for it in the background (ticket 54, F27 b): still
/// attached to the target's input, so a main key the target reads late still has its modifiers
/// down in the target's key state (detaching resets that key state from the system's, where every
/// key is already released), and with the input method still closed, so the key is not composed.
/// Then it detaches and reopens the input method. The same as `IME_REOPEN_PENDING_MS`, how long a
/// reopen stays pending for a busy target.
const PRESS_KEY_BACKGROUND_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long the next input call (and the helper's exit) waits for such a background wait to end
/// on its own before cancelling it (`end_late_key_release`): it then detaches at once and reopens
/// the input method (up to `IME_REQUEST_LIMIT_MS` more, the rest pending in `ImeGuard::drop`'s
/// thread), so ending it adds at most about 1 s to that call.
const LATE_RELEASE_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Presses the keys in order, each once the target has read the one before, so every modifier is
/// down in the target's key state when the next key arrives; the system's key state keeps them
/// down as well until the last key was read, so a reset of the target's key state from the
/// system's (a foreground change, another thread attaching to its input) keeps them down too.
/// Then releases them in reverse order and waits until the target has read the first key's
/// release, the last event.
pub fn press_key(params: &Value) -> Result<Value, Failure> {
    let started = std::time::Instant::now();
    let hwnd = before_input(hwnd_from(params))?;
    let keys = params.get("keys").and_then(|value| value.as_array()).ok_or_else(|| ("invalid_params".to_string(), "keys are required.".to_string()))?;
    let virtual_keys = keys
        .iter()
        .map(|key| key_code(key.as_str().unwrap_or("")))
        .collect::<Result<Vec<_>, _>>()?;
    let _busy = InputBusy::enter();
    unsafe {
        before_input(foreground(hwnd).map_err(foreground_failure))?;
        before_input(ready_for_input(hwnd))?;
        let input = keyboard_window(hwnd)?;
        if sends_content(&virtual_keys) {
            ensure_focus_not_password()?;
        }
        if input != hwnd {
            return press_unconfirmed(hwnd, input, &virtual_keys, started);
        }
    }
    press_attached(hwnd, virtual_keys, started)
}

/// Runs the attached part of `press_key` on a thread of its own (ticket 54, F27 b) and answers
/// with its result. That thread, not the request thread, is the one attached to the target's
/// input, so when the target has not read the last release by `PRESS_KEY_DRAIN_LIMIT` the call
/// can answer while the thread stays attached, with the input method still closed, until the
/// target read it (`await_late_release`, at most `PRESS_KEY_BACKGROUND_LIMIT` from the request).
/// It is kept in `LATE_RELEASE` meanwhile, and the next input call, a stop and the helper's exit
/// end it first (`end_late_key_release`). Every other outcome answers only once the thread has
/// detached, reopened the input method and ended, as before.
fn press_attached(hwnd: HWND, keys: Vec<VIRTUAL_KEY>, started: std::time::Instant) -> Result<Value, Failure> {
    let (answer, answered) = std::sync::mpsc::channel::<(Result<Value, Failure>, bool)>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = cancel.clone();
    let target = hwnd.0 as isize;
    let worker = std::thread::spawn(move || unsafe {
        let hwnd = HWND(target as *mut core::ffi::c_void);
        // AttachThreadInput needs this thread to have a message queue, which a new thread gets
        // with its first call that needs one.
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        match press_on_this_thread(hwnd, &keys, started) {
            Pressed::Done(result) => {
                let _ = answer.send((result, false));
            }
            Pressed::Late { failure, queue, ime, first } => {
                let _ = answer.send((Err(failure), true));
                let ended = await_late_release(&queue, hwnd, first, started + PRESS_KEY_BACKGROUND_LIMIT, &cancelled);
                // Detached before the input method is reopened.
                drop(queue);
                drop(ime);
                log(format_args!(
                    "press_key: {ended} {:.1} s after the request; detached from the target's input and restored its input method",
                    started.elapsed().as_secs_f64()
                ));
            }
        }
    });
    match answered.recv() {
        Ok((result, true)) => {
            let earlier = LATE_RELEASE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).replace(LateRelease { cancel, worker });
            // `InputBusy::enter` ended any earlier one before this call sent anything.
            if let Some(earlier) = earlier {
                earlier.end(std::time::Duration::ZERO);
            }
            result
        }
        Ok((result, false)) => {
            let _ = worker.join();
            result
        }
        Err(_) => {
            let _ = worker.join();
            Err(("internal".to_string(), "The key press ended without a result; the keys pressed so far were released.".to_string()).into())
        }
    }
}

/// How `press_on_this_thread` ended: answered and done (detached, input method reopened), or
/// answered with `failure` while the target has still not read the release of `first`, the last
/// event, so `queue` and `ime` are handed back to stay as they are until it does.
enum Pressed {
    Done(Result<Value, Failure>),
    Late { failure: Failure, queue: TargetQueue, ime: ImeGuard, first: VIRTUAL_KEY },
}

/// `press_key` for a target whose input queue can be followed, on the thread that attaches to it.
unsafe fn press_on_this_thread(hwnd: HWND, keys: &[VIRTUAL_KEY], started: std::time::Instant) -> Pressed {
    let ime = match ImeGuard::suspend(hwnd) {
        Ok(ime) => ime,
        Err(failure) => return Pressed::Done(Err(failure)),
    };
    let mut queue = match TargetQueue::attach(hwnd) {
        Ok(queue) => queue,
        Err(failure) => return Pressed::Done(Err(failure)),
    };
    queue.deadline = Some(started + PRESS_KEY_LIMIT);
    // Released in reverse order on every exit, before the queue is detached.
    let mut held = HeldKeys::default();
    let pressed = press_in_order(&mut queue, &mut held, hwnd, keys);
    let any_pressed = !held.keys.is_empty();
    drop(held);
    // The release of the first key is the last event.
    let first = keys.first().copied().filter(|_| any_pressed);
    let released = |queue: &TargetQueue| match first {
        Some(first) => queue.wait(hwnd, first, |state| state >= 0),
        None => Ok(()),
    };
    // Every key press was sent while the target was in front and read by it. When it then
    // gives up the foreground (a key that opens a window), the releases go to the window that
    // took it: the target never reads them, which is no failure.
    let result = pressed.and_then(|()| match released(&queue) {
        Err(Unread::LostForeground) => Ok(()),
        other => other.map_err(key_press_failure),
    });
    if let Err(failure) = result {
        if let Some(first) = first.filter(|_| !stop_requested()) {
            queue.deadline = Some(started + PRESS_KEY_DRAIN_LIMIT);
            if let Err(Unread::OutOfTime | Unread::Stalled) = released(&queue) {
                return Pressed::Late { failure, queue, ime, first };
            }
        }
        // Detached before the input method is reopened.
        drop(queue);
        drop(ime);
        return Pressed::Done(Err(failure));
    }
    if first.is_some() {
        queue.settle();
    }
    drop(queue);
    let result = action_result(hwnd);
    drop(ime);
    Pressed::Done(Ok(result))
}

/// The background wait of a `press_key` that answered before its target read the release of
/// `first` (`press_attached`): polls the key state, still attached, until the target read it, then
/// returns why it ended. It also ends on a stop, when `cancel` is set (the next input call or the
/// helper's exit, `end_late_key_release`), when the target closed or is no longer in front (its key
/// state was reset from the system's then anyway, so staying attached keeps nothing), and at
/// `deadline`.
unsafe fn await_late_release(queue: &TargetQueue, hwnd: HWND, first: VIRTUAL_KEY, deadline: std::time::Instant, cancel: &AtomicBool) -> &'static str {
    loop {
        if queue.key_state(first) >= 0 {
            return "the target read the last release";
        }
        if cancel.load(Ordering::SeqCst) {
            return "ended by the next input call or the helper's exit before the target read the last release";
        }
        if stop_requested() {
            return "stopped before the target read the last release";
        }
        if target_closed(hwnd) {
            return "the target closed";
        }
        if !in_front(hwnd) {
            return "the target lost the foreground before it read the last release";
        }
        if std::time::Instant::now() >= deadline {
            return "the target had not read the last release within the background limit";
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// A `press_key` thread still waiting in the background (`press_attached`), with the flag that
/// cancels its wait.
struct LateRelease {
    cancel: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<()>,
}

impl LateRelease {
    /// Waits up to `wait` for the thread to end on its own, then cancels its wait and joins it.
    /// Once cancelled it detaches and reopens the input method at once, so the join takes at most
    /// about `IME_REQUEST_LIMIT_MS` more.
    fn end(self, wait: std::time::Duration) {
        let deadline = std::time::Instant::now() + wait;
        while !self.worker.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.worker.join();
    }
}

/// The `press_key` thread still waiting in the background, if any (at most one: every input call
/// ends it before it sends anything).
static LATE_RELEASE: Mutex<Option<LateRelease>> = Mutex::new(None);

/// Ends a `press_key` still waiting in the background for its target to read the last release
/// (`LATE_RELEASE`), waiting up to `LATE_RELEASE_WAIT` for it first: called before every input call
/// (`InputBusy::enter`) and on both paths the helper exits by, before the pending input method
/// reopens are waited for. A stop ends the wait on its own.
pub fn end_late_key_release() {
    let late = LATE_RELEASE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(late) = late {
        late.end(LATE_RELEASE_WAIT);
    }
}

/// Presses `keys` one at a time into `held`, each once the target has read the one before.
unsafe fn press_in_order(queue: &mut TargetQueue, held: &mut HeldKeys, hwnd: HWND, keys: &[VIRTUAL_KEY]) -> Result<(), Failure> {
    for (index, key) in keys.iter().enumerate() {
        check_stop()?;
        ensure_key_target(hwnd, queue.input)?;
        if queue.past_deadline() {
            return Err(key_press_failure(Unread::OutOfTime));
        }
        // Asked before the key is sent: the probe registers the combination for a moment, and
        // the key must not arrive while it does.
        let hotkey = registered_hotkey(&keys[..index], *key);
        if !held.press_one(*key) {
            return Err(key_press_failure(Unread::NotSent));
        }
        // Read once the key is down in the target's key state. A key that is already down (the
        // user holds the same modifier) counts at once: its press does not flip the toggle bit,
        // and it stays down for the target either way.
        if !hotkey {
            queue.wait(hwnd, *key, |state| state < 0).map_err(key_press_failure)?;
            continue;
        }
        // Windows hands a registered hotkey's key to the program that registered it, so the
        // target never reads it (ticket 52, F27): it gets `HOTKEY_READ_LIMIT`, not the call's.
        let deadline = queue.deadline;
        let limit = std::time::Instant::now() + HOTKEY_READ_LIMIT;
        queue.deadline = Some(deadline.map_or(limit, |deadline| deadline.min(limit)));
        let read = queue.wait(hwnd, *key, |state| state < 0);
        queue.deadline = deadline;
        match read {
            Ok(()) => {}
            Err(Unread::OutOfTime | Unread::Stalled) => return Err(taken_by_hotkey()),
            Err(other) => return Err(key_press_failure(other)),
        }
    }
    Ok(())
}

/// How long the target gets to read a key whose combination is registered as a global hotkey
/// (`registered_hotkey`) before `press_key` gives up on it.
const HOTKEY_READ_LIMIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether `key`, pressed while `before` (the keys of the combination pressed ahead of it) are
/// down, is a global hotkey some program registered (`RegisterHotKey`) on this desktop: this
/// thread's own registration of the same combination is refused with
/// `ERROR_HOTKEY_ALREADY_REGISTERED`. When it is not, the probe's registration succeeds and is
/// undone at once. A modifier is never the hotkey's key. Hotkeys a program takes with a keyboard
/// hook instead are not seen here; their key ends at the call's limit, as any unread key does.
unsafe fn registered_hotkey(before: &[VIRTUAL_KEY], key: VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
    const PROBE_ID: i32 = 0xB52;
    let modifier = |key: VIRTUAL_KEY| match key {
        VK_SHIFT => Some(MOD_SHIFT),
        VK_CONTROL => Some(MOD_CONTROL),
        VK_MENU => Some(MOD_ALT),
        VK_LWIN => Some(MOD_WIN),
        _ => None,
    };
    if modifier(key).is_some() {
        return false;
    }
    let modifiers = before.iter().filter_map(|key| modifier(*key)).fold(HOT_KEY_MODIFIERS(0), |all, one| all | one);
    // Without `MOD_NOREPEAT`, as most programs register theirs (measured: a hotkey registered
    // without it is refused to a probe with it too, so either way finds it; ticket 54).
    match RegisterHotKey(None, PROBE_ID, modifiers, u32::from(key.0)) {
        Ok(()) => {
            let _ = UnregisterHotKey(None, PROBE_ID);
            false
        }
        Err(error) => error.code() == windows::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult(),
    }
}

fn taken_by_hotkey() -> Failure {
    (
        "input_failed".to_string(),
        "The target window did not read the key press: that key combination is registered as a global hotkey, so Windows hands it to the program that registered it (which may have acted on it) instead of the target window. The keys pressed so far were released.".to_string(),
    )
        .into()
}

/// Checked before every key of `press_key`: no key goes to a closed target or, once the target
/// lost the foreground, to the window that took it. `input` is the target's `keyboard_window`.
unsafe fn ensure_key_target(hwnd: HWND, input: HWND) -> Result<(), Failure> {
    if target_closed(hwnd) {
        return Err(("input_failed".to_string(), "The target window closed during the key press; the remaining keys were not sent.".to_string()).into());
    }
    if !in_front(hwnd) {
        return Err(lost_foreground_during_key_press());
    }
    if input_left_target(hwnd, input) {
        return Err((
            "input_failed".to_string(),
            "The app inside the target window stopped taking its keyboard input during the key press; the remaining keys were not sent, and the keys pressed so far were released.".to_string(),
        )
            .into());
    }
    Ok(())
}

fn lost_foreground_during_key_press() -> Failure {
    (
        "input_failed".to_string(),
        "The target window lost the foreground during the key press; the remaining keys were not sent, and the keys pressed so far were released.".to_string(),
    )
        .into()
}

/// Why `press_key` ended early; the keys pressed so far are released either way.
fn key_press_failure(unread: Unread) -> Failure {
    let message = match unread {
        Unread::Stopped => return interrupted().into(),
        Unread::LostForeground => return lost_foreground_during_key_press(),
        Unread::Stalled => format!("The target window did not read the key press within {} seconds; the keys pressed so far were released.", READ_LIMIT.as_secs()),
        Unread::OutOfTime => format!(
            "The target window did not read the key press within the {}-second limit for one call; the keys pressed so far were released.",
            PRESS_KEY_LIMIT.as_secs()
        ),
        Unread::NotSent => "SendInput failed; the keys pressed so far were released.".to_string(),
    };
    ("input_failed".to_string(), message).into()
}

/// The most characters `paste_text` takes: the server's `PASTE_TEXT_MAX_CHARS`, checked here too.
const PASTE_TEXT_MAX_CHARS: usize = 20_000;

/// How long `paste_text` waits, once the target read Ctrl+V, for the text to show in the focused
/// element's value; and, where that value cannot be read, how long the text stays on the clipboard
/// after the key was read, for an app that reads the clipboard after handling the key rather than
/// while it does (spec step 6: "or a 300 ms cap").
const PASTE_CONFIRM_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// How long reading the focused element's value before the paste may take (`uia::focused_value`);
/// past it the paste goes on unconfirmed.
const PASTE_FOCUS_READ_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(b'V' as u16);

/// Ticket 77: pastes `text` into the target through the clipboard, in the order of spec-phase4a
/// "`computer_paste_text`": (1) the checks of `type_text` (`expected_app` in `hwnd_from`, the
/// foreground, no open dialog, a stop); (2, 3) the user's clipboard is saved and the text put there,
/// kept out of monitors, the history and the cloud clipboard (`clipboard::Pasting`; a clipboard
/// another program keeps open fails with nothing sent); (4) a stop is checked once more, then
/// Ctrl+V is sent as `press_key` sends keys (`PasteKeys`); (5) the paste is confirmed: when the
/// focused element has a Value pattern, its value must come to contain the text within
/// `PASTE_CONFIRM_WAIT` (`read_confirmed: true`), otherwise the key having been read is all there
/// is (`read_confirmed: false`); (6) the user's clipboard is put back (`clipboard_restored`), also
/// on every failure and stop after the text was put there. The text is in no answer, error or log
/// line.
pub fn paste_text(params: &Value) -> Result<Value, Failure> {
    // Ticket 87: every failure says whether Ctrl+V was sent (`Failure::with_input_sent`): before
    // `press_ctrl_v` it was not, after it it was; `press_ctrl_v` marks its own failures where it
    // knows, and one it cannot tell about (`None`) stays unmarked.
    let mut sent = Some(false);
    paste_text_marking(params, &mut sent).map_err(|failure| match sent {
        Some(sent) => failure.with_input_sent(sent),
        None => failure,
    })
}

fn paste_text_marking(params: &Value, sent: &mut Option<bool>) -> Result<Value, Failure> {
    let started = std::time::Instant::now();
    let hwnd = hwnd_from(params)?;
    let text = params
        .get("text")
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ("invalid_params".to_string(), "text is required.".to_string()))?;
    if text.chars().count() > PASTE_TEXT_MAX_CHARS {
        return Err(("invalid_params".to_string(), format!("text is longer than {PASTE_TEXT_MAX_CHARS} characters.")).into());
    }
    // The clipboard's text ends at the first NUL, so the target would get only what comes before.
    if text.contains('\0') {
        return Err(("invalid_params".to_string(), "text must not contain NUL characters.".to_string()).into());
    }
    let _busy = InputBusy::enter();
    unsafe {
        foreground(hwnd).map_err(foreground_failure)?;
        ready_for_input(hwnd)?;
        let input = keyboard_window(hwnd)?;
        ensure_focus_not_password()?;
        let before = crate::uia::focused_value(input, std::time::Instant::now() + PASTE_FOCUS_READ_LIMIT);
        check_stop()?;
        // Locals drop in reverse order: the user's clipboard is put back (`pasting`) before the
        // queue is detached (`keys`) and the input method reopened (`_ime`).
        let _ime = ImeGuard::suspend(input)?;
        let mut keys = PasteThread::start(hwnd, input, started + PRESS_KEY_LIMIT)?;
        let mut pasting = crate::clipboard::Pasting::begin(text).map_err(|not_put| -> Failure {
            let message = match not_put {
                crate::clipboard::NotPut::Busy => "Another program is using the clipboard, so nothing was pasted and no input was sent.".to_string(),
                crate::clipboard::NotPut::Failed(error) => {
                    format!("The text could not be put on the clipboard ({error}), so nothing was pasted and no input was sent.")
                }
            };
            ("input_failed".to_string(), message).into()
        })?;
        check_stop()?;
        *sent = None;
        keys.press_ctrl_v()?;
        *sent = Some(true);
        let read_at = std::time::Instant::now();
        let confirmed = before.is_some_and(|before| crate::uia::focused_value_shows(input, before, text, PASTE_CONFIRM_WAIT));
        while !confirmed && read_at.elapsed() < PASTE_CONFIRM_WAIT && !stop_requested() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Ctrl+V was sent: the stop notifier has answered that the action was taken.
        check_stop()?;
        let restored = pasting.finish();
        drop(keys);
        let mut result = action_result(hwnd);
        result["read_confirmed"] = Value::Bool(confirmed);
        result["clipboard_restored"] = Value::String(restored.as_str().to_string());
        Ok(result)
    }
}

/// Ticket 88: `PasteKeys` on a thread started for each `paste_text` call, as `press_key` attaches
/// on a thread of its own (ticket 54). A thread that attached to the foreground thread's input
/// while it had a window active (ticket 82's dialog, then on the request thread) no longer sees,
/// through a later attach, the key state follow the target's reads, so the Ctrl wait ran out at
/// `PRESS_KEY_LIMIT`; a new thread has never done either. Chosen over a fallback that asks the
/// target, as `TargetQueue::wait_typed` does: that only paces (ticket 49: it can count a key read
/// early), while here Ctrl must stay confirmed read before V is sent and the release of Ctrl
/// confirmed as the last event read (ticket 26, 52), which only the key state shows. The thread
/// attaches in `start` (a refused attach fails before the clipboard is touched), sends Ctrl+V on
/// `press_ctrl_v` and stays attached until this is dropped, so the request thread puts the
/// clipboard back first as before; the clipboard and UI Automation stay on the request thread.
/// Every key it pressed is released on every way out (`HeldKeys`, also on a panic, which is
/// answered as `internal`).
struct PasteThread {
    go: Option<std::sync::mpsc::Sender<()>>,
    done: std::sync::mpsc::Receiver<Result<(), Failure>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl PasteThread {
    fn start(hwnd: HWND, input: HWND, deadline: std::time::Instant) -> Result<Self, Failure> {
        let (go, wait_go) = std::sync::mpsc::channel::<()>();
        let (answer, done) = std::sync::mpsc::channel::<Result<(), Failure>>();
        let (target, reader) = (hwnd.0 as isize, input.0 as isize);
        let worker = std::thread::Builder::new()
            .name("paste_text".to_string())
            .spawn(move || unsafe {
                let hwnd = HWND(target as *mut core::ffi::c_void);
                let input = HWND(reader as *mut core::ffi::c_void);
                // AttachThreadInput needs this thread to have a message queue.
                let mut message = MSG::default();
                let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                let queue = if input == hwnd {
                    match TargetQueue::attach(input) {
                        Ok(queue) => Some(queue),
                        Err(failure) => {
                            let _ = answer.send(Err(failure));
                            return;
                        }
                    }
                } else {
                    None
                };
                let mut keys = PasteKeys { queue, hwnd, input, deadline };
                if answer.send(Ok(())).is_err() || wait_go.recv().is_err() {
                    return;
                }
                let _ = answer.send(keys.press_ctrl_v());
                // Detached once the request thread is done (it drops `go`).
                let _ = wait_go.recv();
            })
            .map_err(|error| -> Failure { ("internal".to_string(), format!("The paste could not be started ({error}), so no input was sent.")).into() })?;
        let thread = Self { go: Some(go), done, worker: Some(worker) };
        thread.answer()?;
        Ok(thread)
    }

    fn answer(&self) -> Result<(), Failure> {
        self.done.recv().unwrap_or_else(|_| {
            Err(("internal".to_string(), "The paste ended without a result; any key it had pressed was released.".to_string()).into())
        })
    }

    fn press_ctrl_v(&mut self) -> Result<(), Failure> {
        if let Some(go) = &self.go {
            let _ = go.send(());
        }
        self.answer()
    }
}

impl Drop for PasteThread {
    fn drop(&mut self) {
        self.go.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Ctrl+V for `paste_text`, sent as `press_key` sends keys (ticket 26, 52): Ctrl, once the target
/// read it V, then both released in reverse order (on every way out, `HeldKeys`), and the target
/// must read the release of Ctrl, the last event. With `queue` (attached to the target's input,
/// ticket 24) each read is seen in the key state; without one (a UWP app's frame, ticket 44) each
/// key waits until the app answered twice, which paces rather than confirms. The V press is the
/// action (`act`, ticket 73): a stop before it sends nothing more (Ctrl alone is released), a stop
/// after it is answered as an action taken.
struct PasteKeys {
    queue: Option<TargetQueue>,
    hwnd: HWND,
    input: HWND,
    /// When the waits for the target end: `PRESS_KEY_LIMIT` after the request.
    deadline: std::time::Instant,
}

impl PasteKeys {
    unsafe fn wait(&mut self, key: VIRTUAL_KEY, read: impl Fn(i16) -> bool) -> Result<(), Unread> {
        match &mut self.queue {
            Some(queue) => {
                queue.deadline = Some(self.deadline);
                queue.wait(self.hwnd, key, read)
            }
            None => wait_for_answers(self.hwnd, self.input, self.deadline),
        }
    }

    /// Checked before each key: not stopped, the target open and in front with its app's input
    /// inside it, and time left.
    unsafe fn ensure_target(&self) -> Result<(), Failure> {
        check_stop()?;
        let why = if target_closed(self.hwnd) {
            "The target window closed"
        } else if !in_front(self.hwnd) {
            "The target window lost the foreground"
        } else if input_left_target(self.hwnd, self.input) {
            "The app inside the target window stopped taking its keyboard input"
        } else if std::time::Instant::now() >= self.deadline {
            return Err(paste_not_sent(Unread::OutOfTime));
        } else {
            return Ok(());
        };
        Err(("input_failed".to_string(), format!("{why}, so Ctrl+V was not sent and nothing was pasted.")).into())
    }

    /// Ticket 87: each failure says whether V was sent (`Failure::with_input_sent`); Ctrl alone,
    /// released again, pastes nothing.
    unsafe fn press_ctrl_v(&mut self) -> Result<(), Failure> {
        let mut v_sent = false;
        self.press_ctrl_v_marking(&mut v_sent).map_err(|failure| failure.with_input_sent(v_sent))
    }

    unsafe fn press_ctrl_v_marking(&mut self, v_sent: &mut bool) -> Result<(), Failure> {
        let mut held = HeldKeys::default();
        self.ensure_target()?;
        if !held.press_one(VK_CONTROL) {
            return Err(paste_not_sent(Unread::NotSent));
        }
        self.wait(VK_CONTROL, |state| state < 0).map_err(paste_not_sent)?;
        self.ensure_target()?;
        // As `press_in_order`: a Ctrl+V some program registered as a global hotkey never reaches
        // the target, which then gets `HOTKEY_READ_LIMIT` rather than the call's limit.
        let hotkey = self.queue.is_some() && registered_hotkey(&[VK_CONTROL], VK_V);
        let toggle_before = self.queue.as_ref().map(|queue| queue.key_state(VK_V));
        act(|| if held.press_one(VK_V) { Ok(()) } else { Err("SendInput failed.".to_string()) })?.map_err(|_| paste_not_sent(Unread::NotSent))?;
        *v_sent = true;
        let deadline = self.deadline;
        if hotkey {
            self.deadline = deadline.min(std::time::Instant::now() + HOTKEY_READ_LIMIT);
        }
        let read = self.wait(VK_V, |state| state < 0);
        self.deadline = deadline;
        match read {
            Ok(()) => {}
            Err(Unread::OutOfTime | Unread::Stalled) if hotkey => {
                return Err((
                    "input_failed".to_string(),
                    "Ctrl+V is registered as a global hotkey, so Windows handed it to the program that registered it (which may have acted on it) instead of the target window; nothing was pasted there.".to_string(),
                )
                    .into());
            }
            Err(Unread::Stopped) => {
                drop(held);
                self.drain_stopped(toggle_before);
                return Err(interrupted().into());
            }
            Err(unread) => return Err(paste_unread(unread)),
        }
        // The releases, V's then Ctrl's.
        drop(held);
        match self.wait(VK_CONTROL, |state| state >= 0) {
            // As for `press_key`: a target that gave up the foreground over the key no longer
            // reads the releases, which is no failure.
            Ok(()) | Err(Unread::LostForeground) => {}
            // As on success: the target read V, so it finishes handling it before the clipboard is
            // put back, and pastes the text rather than the user's clipboard.
            Err(Unread::Stopped) => {
                self.settle();
                return Err(interrupted().into());
            }
            Err(Unread::Stalled | Unread::OutOfTime) => {
                return Err((
                    "input_failed".to_string(),
                    "The target window read Ctrl+V but did not finish handling it within the time limit; it may still be pasting. Take a screenshot before retrying.".to_string(),
                )
                    .into());
            }
            Err(unread) => return Err(unread.into()),
        }
        self.settle();
        Ok(())
    }

    /// Gives the target the time to finish handling the keys it read (`TargetQueue::settle`, or,
    /// without a queue, two answers).
    unsafe fn settle(&self) {
        match &self.queue {
            Some(queue) => queue.settle(),
            None => {
                let _ = answers_twice(self.input);
            }
        }
    }

    /// After a stop that came once V was sent but before the target read it: gives the target up
    /// to `POINTER_READ_LIMIT` to read the V press (its toggle bit flips from `toggle_before`)
    /// before the clipboard is put back, so that it pastes the text rather than the user's clipboard.
    unsafe fn drain_stopped(&self, toggle_before: Option<i16>) {
        let (Some(queue), Some(before)) = (&self.queue, toggle_before) else { return };
        let until = std::time::Instant::now() + POINTER_READ_LIMIT;
        while ((queue.key_state(VK_V) ^ before) & 1) == 0 && std::time::Instant::now() < until && !target_closed(self.hwnd) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        queue.settle();
    }
}

/// Test builds only, like `debug_stop` (ticket 88): leaves the request thread as the approval
/// dialog did before ticket 82 gave it a thread of its own (`ask::show_on_own_thread`): a window
/// of this thread, active on it, while the thread attaches to the foreground thread's input and
/// detaches again; the window is then destroyed. A `TargetQueue` this thread attaches later no
/// longer sees the key state follow the target's reads (ticket 82's measurements). The foreground
/// thread is the one in front before the window is shown, which may take the foreground itself.
/// Answers `{attached}`: whether the attach happened (not when no window of another thread was in
/// front). Release builds have neither it nor the `debug_attach_while_active` method.
#[cfg(debug_assertions)]
pub fn debug_attach_while_active() -> Result<Value, Failure> {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SW_SHOW, WS_EX_TOOLWINDOW, WS_POPUP};
    let attached;
    unsafe {
        let current = GetCurrentThreadId();
        let foreground = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let window = CreateWindowExW(WS_EX_TOOLWINDOW, windows::core::w!("STATIC"), windows::core::w!(""), WS_POPUP, 0, 0, 1, 1, None, None, None, None)
            .map_err(|error| ("internal".to_string(), format!("The window could not be created: {error}")))?;
        let _ = ShowWindow(window, SW_SHOW);
        attached = foreground != 0 && foreground != current && AttachThreadInput(current, foreground, true).as_bool();
        if attached {
            let _ = AttachThreadInput(current, foreground, false);
        }
        let _ = DestroyWindow(window);
    }
    Ok(serde_json::json!({ "attached": attached }))
}

/// `paste_text` ended before Ctrl+V was sent: nothing was pasted.
fn paste_not_sent(unread: Unread) -> Failure {
    let why = match unread {
        Unread::Stopped => return interrupted().into(),
        Unread::Stalled => format!("The target window did not read the input within {} seconds", READ_LIMIT.as_secs()),
        Unread::OutOfTime => format!("The target window did not read the input within the {}-second limit for one call", PRESS_KEY_LIMIT.as_secs()),
        Unread::LostForeground => "The target window lost the foreground".to_string(),
        Unread::NotSent => "SendInput failed".to_string(),
    };
    Failure::from(("input_failed".to_string(), format!("{why}, so Ctrl+V was not sent and nothing was pasted."))).with_input_sent(false)
}

/// Ctrl+V was sent, but the target did not read it: it may still, and would then paste whatever
/// the clipboard holds by then, the user's clipboard having been put back.
fn paste_unread(unread: Unread) -> Failure {
    let why = match unread {
        Unread::Stopped => return interrupted().into(),
        Unread::Stalled => format!("did not read it within {} seconds", READ_LIMIT.as_secs()),
        Unread::OutOfTime => format!("did not read it within the {}-second limit for one call", PRESS_KEY_LIMIT.as_secs()),
        Unread::LostForeground => "lost the foreground before it read it".to_string(),
        Unread::NotSent => return paste_not_sent(Unread::NotSent),
    };
    (
        "input_failed".to_string(),
        format!(
            "Ctrl+V was sent, but the target window {why}. It may still act on it later, and would then paste whatever the clipboard holds by then rather than this text (the user's clipboard was put back). Take a screenshot before retrying."
        ),
    )
        .into()
}

/// A click at a point of the window (`x`, `y`), or on an element of one of its UI trees
/// (`element`: `{tree_id, index}`, ticket 62), whose position is read now. Either way the click
/// then takes the same path.
pub fn click(params: &Value) -> Result<Value, Failure> {
    let hwnd = before_input(hwnd_from(params))?;
    let delivery = Delivery::from_params(params)?;
    let element = params.get("element");
    let point = match element {
        Some(_) if params.get("x").is_some() || params.get("y").is_some() => {
            return Err(("invalid_params".to_string(), "Give either element or x and y, not both.".to_string()).into());
        }
        Some(_) => None,
        None => Some((required_i32(params, "x")?, required_i32(params, "y")?)),
    };
    let button = params.get("button").and_then(|value| value.as_str()).unwrap_or("left");
    let count = params.get("click_count").and_then(|value| value.as_u64()).unwrap_or(1);
    if !(1..=3).contains(&count) {
        return Err(("invalid_params".to_string(), "click_count must be 1, 2, or 3.".to_string()).into());
    }
    let modifiers = modifiers(params)?;
    let (down, up) = match button {
        "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => return Err(("input_failed".to_string(), "button must be left, right, or middle.".to_string()).into()),
    };
    // Tickets 140 and 141: a click delivered in the background goes through the UI Automation
    // pattern of the element (`uia::click_element`) or, for a point, of the element of the target's
    // own tree at that point (`uia::click_point`); one that needs the real pointer is refused
    // before anything is done.
    if delivery == Delivery::Background {
        if button != "left" || count != 1 || !modifiers.is_empty() {
            return Err(background_unavailable(BackgroundReason::NeedsPointer));
        }
        return match point {
            Some((x, y)) => crate::uia::click_point(params, x, y),
            None => crate::uia::click_element(params),
        };
    }
    let (screen_x, screen_y) = match (point, element) {
        (Some((x, y)), _) => point_in_window(hwnd, x, y)?,
        (None, Some(element)) => crate::uia::element_point(hwnd, element)?,
        (None, None) => unreachable!("a click has a point or an element"),
    };
    let _prepared = unsafe { prepare_target(hwnd) }?;
    unsafe {
        ensure_on_screen(&[(screen_x, screen_y)])?;
        ensure_unobscured(hwnd, &[(screen_x, screen_y)])?;
        check_stop()?;
        // Attached before anything is pressed: attaching resets the key state it follows.
        let reads = PointerReads::follow(&[(screen_x, screen_y)], button);
        // Released on every exit, including a stop between clicks.
        let held = HeldKeys::press(&modifiers);
        for done in 0..count {
            check_stop()?;
            // A click the target answers by giving up the foreground (opening a window) ends a
            // double click there: the next click would go to whatever took its place.
            if !still_target(hwnd) {
                return Err(lost_partway(hwnd, "click", "click", done, count));
            }
            send_mouse(screen_x, screen_y, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).map_err(|message| ("input_failed".to_string(), message))?;
            // Ticket 73: the press is the click's action; once it is sent, a stop says so
            // (`esc::act`). A stop before it sends nothing.
            let pressed = act(|| send_mouse(screen_x, screen_y, down | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK))?;
            // The button goes up right after it went down, stop or not; a press that was never
            // sent gets no release.
            pressed.map_err(|message| ("input_failed".to_string(), message))?;
            let _ = send_mouse(screen_x, screen_y, up | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK);
            if let Some(reads) = &reads {
                reads.sent_click();
            }
            pause_following(reads.as_ref(), std::time::Duration::from_millis(40));
        }
        drop(held);
        let read = confirm_read(hwnd, reads)?;
        Ok(pointer_result(hwnd, read))
    }
}

pub(crate) fn hwnd_from(params: &Value) -> Result<HWND, (String, String)> {
    let id = params.get("window_id").and_then(|value| value.as_str()).unwrap_or("");
    let hwnd = parse_hwnd(id).ok_or_else(|| ("invalid_params".to_string(), "window_id is not a window handle.".to_string()))?;
    // The helper's own windows (the approval dialog, the overlay) never take input from it, nor
    // (ticket 96) those of another helper process, such as another server's helper.
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if crate::windows_list::is_helper_process(pid) {
        return Err(("window_not_found".to_string(), "That window belongs to Deskpass itself and cannot be operated.".to_string()));
    }
    // Ticket 34: a UWP app's frame whose app inside cannot be identified now is refused, also when
    // it was identified (and approved) as that app when the server asked `get_window`.
    if matches!(identify(hwnd, pid), Some(Identity::Unresolved { .. })) {
        return Err(app_unresolved());
    }
    check_expected_app(hwnd, params)?;
    Ok(hwnd)
}

/// The window whose thread takes the keyboard input aimed at the target (ticket 34): for a UWP
/// app's frame the app's CoreWindow inside it, which the input method (`ImeGuard`), the keyboard
/// layout (`EnglishLayout`) and the pacing of the keys then work on, rather than the frame host's;
/// for any other window the target itself. Windows refuses `AttachThreadInput` to the app's thread,
/// so such a target is typed into by `type_unconfirmed` / `press_unconfirmed`, without
/// `TargetQueue` (ticket 44). The foreground checks stay on the target. A frame whose app cannot be
/// identified now is refused with `app_unresolved` rather than typed into through the frame host.
unsafe fn keyboard_window(hwnd: HWND) -> Result<HWND, Failure> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    Ok(keyboard_window_of(hwnd, identify(hwnd, pid).as_ref())?)
}

fn keyboard_window_of(hwnd: HWND, identity: Option<&Identity>) -> Result<HWND, (String, String)> {
    match identity {
        Some(Identity::Hosted { input, .. }) => Ok(*input),
        Some(Identity::Unresolved { .. }) => Err(app_unresolved()),
        Some(Identity::Own { .. }) | None => Ok(hwnd),
    }
}

/// For a UWP app's frame: the app's CoreWindow, which takes the keys, is no longer inside it. Only
/// a direct child counts, as `identify` picks the CoreWindow among the frame's direct children.
unsafe fn input_left_target(hwnd: HWND, input: HWND) -> bool {
    input != hwnd && GetAncestor(input, GA_PARENT) != hwnd
}

fn required_i32(params: &Value, name: &str) -> Result<i32, (String, String)> {
    let Some(value) = params.get(name).and_then(|item| item.as_i64()) else {
        return Err(("invalid_params".to_string(), format!("{name} is required.")));
    };
    i32::try_from(value).map_err(|_| ("invalid_params".to_string(), format!("{name} is out of range.")))
}

fn modifiers(params: &Value) -> Result<Vec<VIRTUAL_KEY>, (String, String)> {
    let Some(list) = params.get("modifiers") else { return Ok(Vec::new()) };
    let Some(list) = list.as_array() else {
        return Err(("invalid_params".to_string(), "modifiers must be a list.".to_string()));
    };
    list.iter()
        .map(|item| match item.as_str().unwrap_or("").to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Ok(VK_CONTROL),
            "shift" => Ok(VK_SHIFT),
            "alt" => Ok(VK_MENU),
            "win" | "super" | "meta" => Ok(VK_LWIN),
            _ => Err(("invalid_params".to_string(), "Unknown modifier.".to_string())),
        })
        .collect()
}

/// The screen point of window pixel (x, y), which must be inside the target's visible frame or
/// inside one of its popups that the screenshot shows next to it (`target_popups`), such as a
/// context menu sticking out past its edge; there window pixels can be negative.
fn point_in_window(hwnd: HWND, x: i32, y: i32) -> Result<(i32, i32), (String, String)> {
    let rect = visible_frame(hwnd).map_err(|message| ("window_not_found".to_string(), message))?;
    let popups: Vec<RECT> = target_popups(hwnd, rect).iter().map(|popup| popup.frame).collect();
    screen_point(rect, &popups, x, y)
}

/// The screen point of window pixel `(x, y)` of a window whose visible frame is `frame`, when it
/// lies in the frame or in one of the window's `popups` (their visible frames): how a click at a
/// point is converted, in the foreground (`point_in_window`) and in the background (ticket 141).
pub(crate) fn screen_point(frame: RECT, popups: &[RECT], x: i32, y: i32) -> Result<(i32, i32), (String, String)> {
    let screen_x = frame.left.saturating_add(x);
    let screen_y = frame.top.saturating_add(y);
    let inside = |rect: &RECT| screen_x >= rect.left && screen_y >= rect.top && screen_x < rect.right && screen_y < rect.bottom;
    if !inside(&frame) && !popups.iter().any(inside) {
        return Err(("input_failed".to_string(), "The point is outside the target window, so no input was sent.".to_string()));
    }
    Ok((screen_x, screen_y))
}

/// What `foreground` and `prepare_background` check first: the target is open, visible and
/// answers a message (nothing is raised or sent).
unsafe fn ensure_target_responds(hwnd: HWND) -> Result<(), String> {
    if !IsWindow(Some(hwnd)).as_bool() {
        return Err("The window is no longer open.".to_string());
    }
    if !IsWindowVisible(hwnd).as_bool() {
        return Err("The target window is not visible, so no input was sent.".to_string());
    }
    let mut response = 0usize;
    if SendMessageTimeoutW(hwnd, WM_NULL, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 1000, Some(&mut response)).0 == 0 {
        return Err("The target window did not respond, so no input was sent.".to_string());
    }
    Ok(())
}

unsafe fn foreground(hwnd: HWND) -> Result<(), String> {
    ensure_target_responds(hwnd)?;
    // Already in front (itself or its dialog, as `ready_for_input` judges it): no key is sent, as
    // a stray ALT would put an app with a menu bar into menu mode. It may still lie under other
    // windows, so it is raised.
    if in_front(hwnd) && !IsIconic(hwnd).as_bool() {
        return raise(hwnd);
    }
    let stacking = Stacking::capture(hwnd);
    let previous = GetForegroundWindow();
    if IsIconic(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(700) && !stop_requested() {
        attempt_foreground(hwnd);
        if in_front(hwnd) {
            return raise(hwnd);
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    stacking.restore();
    if !previous.0.is_null() && previous != hwnd && GetForegroundWindow() != previous {
        let _ = SetForegroundWindow(previous);
        if GetForegroundWindow() != previous {
            tap_alt();
            let _ = SetForegroundWindow(previous);
        }
        if GetForegroundWindow() != previous {
            SwitchToThisWindow(previous, false);
        }
    }
    Err("Could not bring the target window to the foreground, so no input was sent.".to_string())
}

/// Raises the target, which holds the foreground (itself or through its dialog), to the top of its
/// z-order band without activating anything: no key is sent and the focus stays where it is. A
/// window can hold the foreground yet lie under other apps' windows (Paint under VS Code), and
/// then pointer input at every point they cover is refused. Its owned windows move up with it; a
/// dialog of it that holds the foreground is raised last, so it stays on top and keeps the input.
/// Its own popups above it that are not owned (`popup_kind`: an app's flyouts and menus, often
/// unowned) do not move with it, so the ones in its band are raised back over it, front one last
/// (ticket 51, F16): the screenshot showed them over the target, and input there is theirs.
///
/// SetWindowPos waits, without a limit, for the target's thread to answer WM_WINDOWPOSCHANGING,
/// so the raise runs on a thread of its own and fails once it took `RAISE_LIMIT` (F15). That
/// thread is left to finish when the target answers (the raise then still happens, late).
unsafe fn raise(hwnd: HWND) -> Result<(), String> {
    let target = hwnd.0 as isize;
    // EnumWindows lists top-level windows from the top of the z-order down. A popup always on
    // top above a target that is not stays above it anyway.
    let popups: Vec<isize> = top_level_windows()
        .unwrap_or_default()
        .into_iter()
        .take_while(|window| *window != hwnd)
        .filter(|window| popup_kind(*window, hwnd).is_some() && (!is_topmost(*window) || is_topmost(hwnd)))
        .map(|window| window.0 as isize)
        .collect();
    let (done, raised) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        unsafe {
            let hwnd = HWND(target as *mut core::ffi::c_void);
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
            for popup in popups.iter().rev() {
                let _ = SetWindowPos(HWND(*popup as *mut core::ffi::c_void), Some(HWND_TOP), 0, 0, 0, 0, flags);
            }
            let foreground = GetForegroundWindow();
            if owned_by(foreground, hwnd) {
                let _ = SetWindowPos(foreground, Some(HWND_TOP), 0, 0, 0, 0, flags);
            }
        }
        let _ = done.send(());
    });
    raised
        .recv_timeout(RAISE_LIMIT)
        .map_err(|_| format!("The target window did not respond to being raised within {} second, so no input was sent.", RAISE_LIMIT.as_secs()))
}

/// How long `raise` waits for the target to take the raise; the same as the `WM_NULL` probe
/// `foreground` starts with.
const RAISE_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

struct Stacking {
    hwnd: HWND,
    topmost: bool,
    above: HWND,
}

impl Stacking {
    unsafe fn capture(hwnd: HWND) -> Self {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let above = GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default();
        Self { hwnd, topmost: style & WS_EX_TOPMOST.0 != 0, above }
    }

    unsafe fn restore(self) {
        let insert_after = if self.topmost {
            HWND_TOPMOST
        } else if self.above.0.is_null() {
            HWND_NOTOPMOST
        } else {
            self.above
        };
        let _ = SetWindowPos(self.hwnd, Some(insert_after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

unsafe fn attempt_foreground(hwnd: HWND) {
    let current = GetCurrentThreadId();
    let foreground = GetForegroundWindow();
    let foreground_thread = GetWindowThreadProcessId(foreground, None);
    let target_thread = GetWindowThreadProcessId(hwnd, None);
    let attached_foreground = foreground_thread != 0 && foreground_thread != current && AttachThreadInput(current, foreground_thread, true).0 != 0;
    let attached_target = target_thread != 0 && target_thread != current && target_thread != foreground_thread && AttachThreadInput(current, target_thread, true).0 != 0;
    run_foreground_steps(
        |step| match step {
            ForegroundStep::Set => {
                let _ = SetForegroundWindow(hwnd);
            }
            ForegroundStep::AltThenSet => {
                tap_alt();
                let _ = SetForegroundWindow(hwnd);
            }
            ForegroundStep::SwitchTo => SwitchToThisWindow(hwnd, false),
            ForegroundStep::BringToTop => {
                let _ = BringWindowToTop(hwnd);
            }
        },
        || in_front(hwnd),
    );
    if attached_foreground {
        let _ = AttachThreadInput(current, foreground_thread, false);
    }
    if attached_target {
        let _ = AttachThreadInput(current, target_thread, false);
    }
}

/// One way `attempt_foreground` tries to bring the target forward.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ForegroundStep {
    Set,
    /// ALT (`tap_alt`), then `Set`: a tap of ALT counts as input to this process, which lifts the
    /// foreground lock that refused the step before.
    AltThenSet,
    SwitchTo,
    BringToTop,
}

/// The steps in order, each taken only while the target is still not in front.
const FOREGROUND_STEPS: [ForegroundStep; 5] =
    [ForegroundStep::Set, ForegroundStep::AltThenSet, ForegroundStep::SwitchTo, ForegroundStep::BringToTop, ForegroundStep::AltThenSet];

/// Runs `FOREGROUND_STEPS` until `in_front` holds, checked before every step: ALT goes only after
/// a step that left the target behind, never once `BringWindowToTop` alone brought it forward (F14).
fn run_foreground_steps(mut run: impl FnMut(ForegroundStep), in_front: impl Fn() -> bool) {
    for step in FOREGROUND_STEPS {
        if in_front() {
            return;
        }
        run(step);
    }
}

/// Presses and releases ALT for the foreground lock, masked (`alt_tap_events`), in one SendInput.
/// Never pressed once input is stopped; once pressed, always released (when SendInput took only
/// part of the events, the keys they left down are released, with `retry_release`), and nothing
/// is released that was not pressed.
unsafe fn tap_alt() {
    if stop_requested() {
        return;
    }
    let events = alt_tap_events();
    let inputs: Vec<INPUT> = events.iter().map(|(key, up)| key_input(*key, *up)).collect();
    let taken = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) as usize;
    let up: Vec<INPUT> = left_down(&events, taken).iter().map(|key| key_input(*key, true)).collect();
    if !up.is_empty() {
        let (released, attempts) = retry_release(|| SendInput(&up, std::mem::size_of::<INPUT>() as i32) as usize == up.len());
        if !released {
            eprintln!("alt tap: releasing {} key(s) did not take after {attempts} attempts; they may still be down", up.len());
        }
    }
}

/// An unassigned virtual key (AutoHotkey's MenuMaskKey): pressed while ALT is down, it makes the
/// system take the ALT for part of a combination rather than a lone ALT (ticket 82).
const VK_ALT_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

/// The key events `tap_alt` sends, in order, as (key, released): ALT with `VK_ALT_MASK` pressed
/// and released inside it, as a lone ALT released puts the window that handles it into menu or
/// keytip mode, which eats the keys after it (ticket 82).
fn alt_tap_events() -> Vec<(VIRTUAL_KEY, bool)> {
    vec![(VK_MENU, false), (VK_ALT_MASK, false), (VK_ALT_MASK, true), (VK_MENU, true)]
}

/// The keys `events[..taken]` pressed and did not release, last pressed first: SendInput inserts
/// its events in order and stops at the first it cannot, so these are the ones left down.
fn left_down(events: &[(VIRTUAL_KEY, bool)], taken: usize) -> Vec<VIRTUAL_KEY> {
    let mut down: Vec<VIRTUAL_KEY> = Vec::new();
    for (key, up) in &events[..taken.min(events.len())] {
        if *up {
            down.retain(|held| held != key);
        } else {
            down.push(*key);
        }
    }
    down.reverse();
    down
}

unsafe fn flush_target_once(hwnd: HWND) {
    let _ = target_answers(hwnd);
}

/// Whether `hwnd`'s thread answered a sent message within 200 ms.
unsafe fn target_answers(hwnd: HWND) -> bool {
    let mut response = 0usize;
    SendMessageTimeoutW(
        hwnd,
        WM_NULL,
        windows::Win32::Foundation::WPARAM(0),
        windows::Win32::Foundation::LPARAM(0),
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        200,
        Some(&mut response),
    )
    .0 != 0
}

unsafe fn flush_target(hwnd: HWND) {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(300) {
        flush_target_once(hwnd);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// How long click and drag wait, after their last event, for the target to read their last button
/// release: the 300 ms that `flush_target` waits, so a call takes no longer than it did with it.
const POINTER_READ_LIMIT: std::time::Duration = std::time::Duration::from_millis(300);

/// Follows whether the target read the presses and releases of the mouse button that click or
/// drag sends, in the key state of the thread that gets them (ticket 53, F35). The helper's thread
/// is attached to the input of the thread of the window under the pointer (`TargetQueue`), so its
/// key state for the button changes only as that thread takes the button's messages out of its
/// queue: the high bit is set while a press is read and its release is not, and the toggle bit
/// flips with every press read. Measured with the fixture busy after every press, also while it
/// answers sent messages: the key state never showed a release read before the target took it,
/// where `WM_NULL` replies did; `GetQueueStatus` and `GetInputState` on the attached thread never
/// showed the target's pending input. Wheel notches and moves change no key state, so scroll and
/// move_mouse keep `flush_target`.
struct PointerReads {
    queue: TargetQueue,
    /// The button's virtual key or, with swapped buttons, both left and right (which of them the
    /// target's key state shows is not relied on), each with its state before anything was sent
    /// and how many of its events (press, release, press, ...) the target has read.
    buttons: Vec<(VIRTUAL_KEY, i16, std::cell::Cell<u32>)>,
    /// How many events (presses and releases) were sent.
    sent: std::cell::Cell<u32>,
}

impl PointerReads {
    /// None, so the caller waits like `flush_target`, when the points are on windows of different
    /// threads or the thread cannot be attached to (another integrity level, a UWP app's
    /// CoreWindow, the helper's own thread).
    unsafe fn follow(points: &[(i32, i32)], button: &str) -> Option<Self> {
        let windows: Vec<HWND> = points.iter().map(|&(x, y)| WindowFromPoint(POINT { x, y })).collect();
        let thread = GetWindowThreadProcessId(*windows.first()?, None);
        if thread == 0 || windows.iter().any(|window| GetWindowThreadProcessId(*window, None) != thread) {
            return None;
        }
        let queue = TargetQueue::attach(windows[0]).ok()?;
        let swapped = GetSystemMetrics(SM_SWAPBUTTON) != 0;
        let keys = match button {
            "left" | "right" if swapped => vec![VK_LBUTTON, VK_RBUTTON],
            "right" => vec![VK_RBUTTON],
            "middle" => vec![VK_MBUTTON],
            _ => vec![VK_LBUTTON],
        };
        let buttons = keys.into_iter().map(|key| (key, GetKeyState(i32::from(key.0)), std::cell::Cell::new(0))).collect();
        Some(Self { queue, buttons, sent: std::cell::Cell::new(0) })
    }

    /// One press and its release were sent.
    fn sent_click(&self) {
        self.sent.set(self.sent.get() + 2);
    }

    /// Looks at the key state and returns whether every event sent so far was read. After `n`
    /// events read the button is down when `n` is odd, and its toggle bit has flipped `(n + 1) / 2`
    /// times; the count goes up to the fewest events that agree with what the key state shows, so
    /// several events read between two looks are never taken for more than they are (a whole
    /// double click read between two looks shows nothing new, and the wait runs into the limit
    /// rather than ending early).
    unsafe fn observe(&self) -> bool {
        let sent = self.sent.get();
        let mut all_read = false;
        for (key, before, read) in &self.buttons {
            let state = GetKeyState(i32::from(key.0));
            let (flipped, down) = (((state ^ before) & 1) == 1, state < 0);
            if let Some(count) = (read.get()..=sent).find(|n| (n % 2 == 1) == down && ((n + 1) / 2 % 2 == 1) == flipped) {
                read.set(count);
            }
            all_read |= read.get() == sent;
        }
        all_read
    }
}

/// The success result of click and drag: `action_result`, marked `read_confirmed: false` when the
/// target had not read the last release by `POINTER_READ_LIMIT`. A confirmed read (and a target
/// whose reads are not followed) carries no such field, so that JSON is unchanged.
unsafe fn pointer_result(hwnd: HWND, read: bool) -> Value {
    let mut result = action_result(hwnd);
    if !read {
        result["read_confirmed"] = Value::Bool(false);
    }
    result
}

/// Waits `pause`, following the target's reads meanwhile when they are followed.
unsafe fn pause_following(reads: Option<&PointerReads>, pause: std::time::Duration) {
    let Some(reads) = reads else {
        std::thread::sleep(pause);
        return;
    };
    let started = std::time::Instant::now();
    while started.elapsed() < pause {
        reads.observe();
        std::thread::sleep(std::time::Duration::from_micros(250));
    }
}

/// After the last event of a click or drag: waits until the target has read the last release and
/// handled it, for at most `POINTER_READ_LIMIT`; without `reads`, like before (`flush_target`).
/// Once the release is read, two sent messages give the target the rest of the limit to finish
/// handling it (as `TargetQueue::settle`), so a window it opens or closes over the click is in
/// place for the result. Returns whether the release was read: a target that has not read it by
/// the limit is busy, and the call still succeeds (`false`), as it did before the wait existed,
/// rather than failing, which would invite a retry that clicks twice; the caller marks the result
/// `read_confirmed: false` so the server asks the model to check a screenshot. A target that
/// closes ends the wait (`TargetQueue::wait_until`). Only a stop fails the wait: its
/// `LostForeground` and `Stalled` limits (`FOREGROUND_RETURN_LIMIT`, `READ_LIMIT`) are longer
/// than `POINTER_READ_LIMIT`, so a busy target that loses the foreground meanwhile ends at the
/// limit like any other busy target (`read_confirmed: false`, with `foreground_changed`).
unsafe fn confirm_read(hwnd: HWND, reads: Option<PointerReads>) -> Result<bool, Failure> {
    let Some(mut reads) = reads else {
        flush_target(hwnd);
        return Ok(true);
    };
    let deadline = std::time::Instant::now() + POINTER_READ_LIMIT;
    reads.queue.deadline = Some(deadline);
    let key = reads.buttons[0].0;
    match reads.queue.wait_until(hwnd, key, |_| reads.observe(), || false) {
        Ok(()) => {
            let window = if GetWindowThreadProcessId(hwnd, None) == reads.queue.target { hwnd } else { reads.queue.input };
            for _ in 0..2 {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                let mut response = 0usize;
                let _ = SendMessageTimeoutW(
                    window,
                    WM_NULL,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    (left.as_millis() as u32).clamp(1, 200),
                    Some(&mut response),
                );
            }
            Ok(true)
        }
        Err(Unread::OutOfTime) => Ok(false),
        Err(unread) => Err(unread.into()),
    }
}

unsafe fn send_wheel(x: i32, y: i32, flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, delta: i32) -> Result<(), String> {
    let (dx, dy) = absolute_point(x, y);
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: delta as u32,
                dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    if SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 0 { Err("SendInput failed.".to_string()) } else { Ok(()) }
}

fn absolute_point(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let origin_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let origin_y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        (normalized(x - origin_x, width), normalized(y - origin_y, height))
    }
}

/// The 0..=65535 absolute coordinate of pixel `offset` on a virtual-screen axis `size` pixels
/// long. Windows maps it back to `floor(normalized * size / 65536)`, so aiming at the pixel's
/// center lands on that pixel; scaling by 65535 / (size - 1) landed 1 pixel short near the origin.
fn normalized(offset: i32, size: i32) -> i32 {
    ((offset as i64 * 65536 + 32768) / size.max(1) as i64) as i32
}

unsafe fn send_mouse(x: i32, y: i32, flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> Result<(), String> {
    let (dx, dy) = absolute_point(x, y);
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    if sent == 0 { Err("SendInput failed.".to_string()) } else { Ok(()) }
}

/// How long the target may go without reading a typed key before typing gives up: the time after
/// which Windows itself calls a window that does not read its messages "not responding".
const READ_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// One character typed in one SendInput: a UTF-16 unit (KEYEVENTF_UNICODE, which reaches the
/// target as VK_PACKET) as a press and a release, a surrogate pair as the press and release of
/// its high half then of its low half, or Enter. A pair goes in one call so that both halves are
/// sent or neither is: SendInput inserts its events back to back, and no check between them can
/// end typing with the high half sent alone (F13).
#[derive(Clone, Copy)]
enum Key {
    Unit(u16),
    Pair(u16, u16),
    Return,
}

impl Key {
    fn virtual_key(self) -> VIRTUAL_KEY {
        match self {
            Key::Unit(_) | Key::Pair(..) => VK_PACKET,
            Key::Return => VK_RETURN,
        }
    }

    /// How many presses of `virtual_key` the key sends: two for a surrogate pair.
    fn presses(self) -> u32 {
        if matches!(self, Key::Pair(..)) { 2 } else { 1 }
    }

    fn inputs(self) -> Vec<INPUT> {
        match self {
            Key::Unit(unit) => vec![unicode_input(unit, false), unicode_input(unit, true)],
            Key::Pair(high, low) => vec![unicode_input(high, false), unicode_input(high, true), unicode_input(low, false), unicode_input(low, true)],
            Key::Return => vec![key_input(VK_RETURN, false), key_input(VK_RETURN, true)],
        }
    }

    /// Sends the key's events in one SendInput; false unless every one of them was taken.
    unsafe fn send(self) -> bool {
        let inputs = self.inputs();
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len()
    }
}

/// The key that types `character`: Enter for LF, otherwise its UTF-16 units (a pair for a
/// character outside the BMP).
fn character_key(character: char) -> Key {
    if character == '\n' {
        return Key::Return;
    }
    let mut units = [0u16; 2];
    match *character.encode_utf16(&mut units) {
        [high, low] => Key::Pair(high, low),
        [unit] => Key::Unit(unit),
        _ => unreachable!("a char is one or two UTF-16 units"),
    }
}

/// The helper's thread attached to the target thread's input (AttachThreadInput), so both share
/// one input queue and its key state. That key state changes only as the target takes key
/// messages out of the queue: GetKeyState here shows whether a key is down and, in its toggle
/// bit that every press flips, the parity of the presses read so far. Detached when dropped.
/// The target thread is the thread of `input`, the target's `keyboard_window`, which is the target
/// itself: a UWP app's frame, whose CoreWindow thread cannot be attached to, is typed into without
/// this (`type_unconfirmed`, ticket 44).
struct TargetQueue {
    current: u32,
    target: u32,
    /// The window whose thread reads the keys; the sent messages that confirm it handled them go to
    /// it too. The foreground checks of the waits stay on the target window passed to them.
    input: HWND,
    /// When set, waits end here even while the target is still making progress.
    deadline: Option<std::time::Instant>,
}

/// Why a wait for the target ended before it read the key.
enum Unread {
    /// Input was stopped (Esc, or the server went away).
    Stopped,
    /// The key's state did not change for `READ_LIMIT`: the target stopped reading its input.
    Stalled,
    /// The queue's deadline passed.
    OutOfTime,
    /// Another window took the foreground and kept it for `FOREGROUND_RETURN_LIMIT`.
    LostForeground,
    /// SendInput did not take the key.
    NotSent,
}

impl From<Unread> for Failure {
    fn from(unread: Unread) -> Self {
        let message = match unread {
            Unread::Stopped => return interrupted().into(),
            Unread::Stalled => format!("The target window did not read the input within {} seconds; nothing more was sent.", READ_LIMIT.as_secs()),
            Unread::OutOfTime => "The target window did not read the input within the time limit; nothing more was sent.".to_string(),
            Unread::LostForeground => "The target window lost the foreground before it read the input; nothing more was sent.".to_string(),
            Unread::NotSent => "SendInput failed.".to_string(),
        };
        ("input_failed".to_string(), message).into()
    }
}

/// How long a wait for the target to read a key lets another window hold the foreground before it
/// gives up: a brief foreground change (a notification, a window flashing up) can hand it back.
const FOREGROUND_RETURN_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a wait for a typed key sees no change in its state before it asks the target
/// whether it is back in its message loop (`TargetQueue::wait_typed`), and how often it asks
/// again while nothing changes.
const ASK_TARGET_AFTER: std::time::Duration = std::time::Duration::from_millis(100);

impl TargetQueue {
    unsafe fn attach(input: HWND) -> Result<Self, Failure> {
        let current = GetCurrentThreadId();
        let target = GetWindowThreadProcessId(input, None);
        if target == 0 || target == current || AttachThreadInput(current, target, true).0 == 0 {
            return Err(("input_failed".to_string(), "Could not follow the target window's input processing, so no input was sent.".to_string()).into());
        }
        Ok(Self { current, target, input, deadline: None })
    }

    unsafe fn key_state(&self, key: VIRTUAL_KEY) -> i16 {
        GetKeyState(i32::from(key.0))
    }

    /// Sends one key (press and release) and waits until the target has read its release. Every
    /// earlier key was read before this one was sent, so its press is the only unread one and a
    /// flipped toggle bit means exactly that this one was read. Messages posted while handling
    /// the press (the WM_CHAR from TranslateMessage) are taken before the next input, so once
    /// the release is read the target has also handled the character, including closing itself
    /// over it; nothing more is in its queue at that point. A stop lets through at most this key.
    /// Those guarantees hold when the toggle bit confirms the key; a character confirmed by
    /// `wait_typed`'s fallback instead (only while the key state shows no press of it read) is
    /// only likely read. Enter never takes the fallback: it is
    /// the key that closes dialogs, and the next key must not be sent before the target handled it.
    /// A surrogate pair presses VK_PACKET twice, so it counts as read once the toggle bit was seen
    /// flipping twice; when the target reads both halves between two looks at the key state, the
    /// bit is back where it was and `wait_typed`'s fallback confirms the pair instead.
    unsafe fn type_key(&mut self, hwnd: HWND, key: Key) -> Result<(), Unread> {
        let virtual_key = key.virtual_key();
        let before = self.key_state(virtual_key);
        if !key.send() {
            return Err(Unread::NotSent);
        }
        let (seen, flips) = (std::cell::Cell::new(before), std::cell::Cell::new(0));
        let read = |state: i16| {
            if ((state ^ seen.get()) & 1) == 1 {
                flips.set(flips.get() + 1);
                seen.set(state);
            }
            state >= 0 && flips.get() >= key.presses()
        };
        match key {
            Key::Return => self.wait(hwnd, virtual_key, read),
            // Asked only while no press of this key was seen read (`wait_typed`).
            Key::Unit(_) | Key::Pair(..) => self.wait_typed(hwnd, virtual_key, read, || flips.get() == 0),
        }
    }

    /// `wait` for a typed character, which also ends when the key state has not changed for
    /// `ASK_TARGET_AFTER` and the target then answers two sent messages in a row
    /// (`target_answers`). The key state this thread sees can stop following the target's reads
    /// (seen with Windows 11 Notepad right after a layout switch: every character arrived, yet the
    /// toggle bit here never flipped), which made typing stop with a false `input_failed`. This is
    /// a heuristic that assumes a plain GetMessage loop: such a target answers the second message
    /// only after it has come back for more messages, having taken the queued key on the way. The
    /// target is asked only while `may_ask` holds, which `type_key` keeps true only until the key
    /// state shows this key's press read: from then on the key state is following the target, and
    /// a target that answers sent messages while it is still busy with the character (it pumps
    /// them during its input handling) would otherwise be counted as having read it, and every key
    /// after it the same way (measured with the fixture: typing ran 4 keys ahead and the call
    /// succeeded before the target had read them, ticket 49). A target that has posted messages of
    /// its own queued ahead of the key, or that pumps sent messages while busy before it takes the
    /// press, can still be counted as having read it early (typing runs ahead by that key; each
    /// next key is still sent only while the target is in front). A target busy handling input
    /// without pumping does not answer, so it still ends as `Stalled`. Each ask can take up to
    /// 0.4 s, so `Stalled` and the call deadline can be reported that much later, and a target
    /// whose key state stays silent types at most about 190 characters per call.
    unsafe fn wait_typed(&self, hwnd: HWND, key: VIRTUAL_KEY, read: impl Fn(i16) -> bool, may_ask: impl Fn() -> bool) -> Result<(), Unread> {
        self.wait_until(hwnd, key, read, may_ask)
    }

    /// After the last key of the text was read: waits until the target has handled it.
    unsafe fn finish(&self) {
        self.settle();
    }

    fn past_deadline(&self) -> bool {
        self.deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }

    /// Polls the state of `key` until `read` accepts it. Losing the foreground does not count as
    /// read: once the target's thread has finished the message it was handling (so a dialog
    /// closing itself over the key is already hidden), a closed target ends the wait for the
    /// caller to judge; otherwise the wait goes on while the target gets the foreground back
    /// within `FOREGROUND_RETURN_LIMIT`, and fails with `LostForeground` when it does not. Ends
    /// on a stop, when the state of `key` has not changed for `READ_LIMIT`, and at the deadline.
    unsafe fn wait(&self, hwnd: HWND, key: VIRTUAL_KEY, read: impl Fn(i16) -> bool) -> Result<(), Unread> {
        self.wait_until(hwnd, key, read, || false)
    }

    unsafe fn wait_until(&self, hwnd: HWND, key: VIRTUAL_KEY, read: impl Fn(i16) -> bool, ask_target: impl Fn() -> bool) -> Result<(), Unread> {
        let mut state = self.key_state(key);
        let mut changed_at = std::time::Instant::now();
        let mut asked_at = changed_at;
        let mut away_since: Option<std::time::Instant> = None;
        while !read(state) {
            if stop_requested() {
                return Err(Unread::Stopped);
            }
            if in_front(hwnd) {
                away_since = None;
            } else {
                let since = *away_since.get_or_insert_with(|| {
                    flush_target_once(self.input);
                    std::time::Instant::now()
                });
                if target_closed(hwnd) {
                    return Ok(());
                }
                if since.elapsed() >= FOREGROUND_RETURN_LIMIT {
                    return Err(Unread::LostForeground);
                }
            }
            if changed_at.elapsed() >= READ_LIMIT {
                return Err(Unread::Stalled);
            }
            if self.past_deadline() {
                return Err(Unread::OutOfTime);
            }
            std::thread::sleep(std::time::Duration::from_micros(250));
            if changed_at.elapsed() >= ASK_TARGET_AFTER && ask_target() && asked_at.elapsed() >= ASK_TARGET_AFTER {
                if target_answers(self.input) && target_answers(self.input) {
                    return Ok(());
                }
                asked_at = std::time::Instant::now();
            }
            let now = self.key_state(key);
            if now != state {
                state = now;
                changed_at = std::time::Instant::now();
            }
        }
        Ok(())
    }

    /// Sent messages are handled only once the target thread is back in its message loop, so
    /// these return after it finished handling the last key and the characters TranslateMessage
    /// made of it.
    unsafe fn settle(&self) {
        flush_target_once(self.input);
        flush_target_once(self.input);
    }
}

impl Drop for TargetQueue {
    fn drop(&mut self) {
        unsafe {
            let _ = AttachThreadInput(self.current, self.target, false);
        }
    }
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn key_input(key: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_code(name: &str) -> Result<VIRTUAL_KEY, (String, String)> {
    let name = name.to_ascii_lowercase();
    if name.len() == 1 {
        let byte = name.as_bytes()[0];
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            return Ok(VIRTUAL_KEY(byte.to_ascii_uppercase() as u16));
        }
    }
    if let Some(rest) = name.strip_prefix('f') {
        if let Ok(number) = rest.parse::<u16>() {
            if (1..=24).contains(&number) {
                return Ok(VIRTUAL_KEY(0x70 + number - 1));
            }
        }
    }
    let key = match name.as_str() {
        "backspace" => VK_BACK,
        "tab" => VK_TAB,
        "return" => VK_RETURN,
        "escape" => VK_ESCAPE,
        "space" => VK_SPACE,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "end" => VK_END,
        "home" => VK_HOME,
        "left" => VK_LEFT,
        "up" => VK_UP,
        "right" => VK_RIGHT,
        "down" => VK_DOWN,
        "insert" => VK_INSERT,
        "delete" => VK_DELETE,
        "shift" => VK_SHIFT,
        "ctrl" => VK_CONTROL,
        "alt" => VK_MENU,
        "win" => VK_LWIN,
        _ => return Err(("invalid_params".to_string(), format!("Unknown key {name}."))),
    };
    Ok(key)
}

const IMC_GETOPENSTATUS: usize = 5;
const IMC_SETOPENSTATUS: usize = 6;

/// How long each request to the target's input method window may take. The target's thread
/// handles it, so a busy target would otherwise hold `press_key` past the server's timeout.
const IME_REQUEST_LIMIT_MS: u32 = 500;

/// Closes the target's input method for `press_key`, so keys reach the app instead of being
/// composed, and reopens it when dropped.
struct ImeGuard {
    ime: HWND,
    was_open: bool,
}

impl ImeGuard {
    /// Fails with `input_failed`, before any key is sent, when the target does not answer within
    /// `IME_REQUEST_LIMIT_MS`.
    unsafe fn suspend(hwnd: HWND) -> Result<Self, Failure> {
        let ime = ImmGetDefaultIMEWnd(hwnd);
        if ime.0.is_null() {
            return Ok(Self { ime, was_open: false });
        }
        let unanswered = || -> Failure {
            (
                "input_failed".to_string(),
                format!("The target window did not answer the input method request within {IME_REQUEST_LIMIT_MS} ms, so no key was sent."),
            )
                .into()
        };
        // An earlier press_key on this target may have left a reopen pending (its close, or the
        // reopen itself, took longer than IME_REQUEST_LIMIT_MS to land): that request still sits
        // ahead of ours in the target's queue. IMC_GETOPENSTATUS answers a busy target at once,
        // not queued behind it, so it could say "closed" before the target has caught up to the
        // pending reopen; closing now would then be overtaken by that reopen once the target
        // finally handles both, leaving the input method open. So wait for it first, bounded like
        // the requests below so this stays inside PRESS_KEY_LIMIT; not answering, this touches no
        // window other than `ime`.
        let overtaken_by_a_pending_reopen = PendingReopen::for_target(ime.0 as isize).is_some_and(|pending| !pending.wait(std::time::Duration::from_millis(IME_REQUEST_LIMIT_MS.into())));
        if overtaken_by_a_pending_reopen {
            // Do not trust a status query while that reopen might still be ahead of it: treat the
            // input method as open and close it again, ordered after the pending reopen in the
            // target's queue, same as the open path below.
            let guard = Self { ime, was_open: true };
            ime_request(ime, IMC_SETOPENSTATUS, 0).ok_or_else(unanswered)?;
            return Ok(guard);
        }
        let open = ime_request(ime, IMC_GETOPENSTATUS, 0).ok_or_else(unanswered)?;
        if open == 0 {
            return Ok(Self { ime, was_open: false });
        }
        // Reopens on every exit from here, including a close that timed out: the target may
        // still handle that request late, and the reopen comes after it.
        let guard = Self { ime, was_open: true };
        ime_request(ime, IMC_SETOPENSTATUS, 0).ok_or_else(unanswered)?;
        Ok(guard)
    }
}

/// Sends an input method request to `ime` and returns its answer, or None when the target did
/// not answer within `IME_REQUEST_LIMIT_MS`.
unsafe fn ime_request(ime: HWND, request: usize, value: isize) -> Option<usize> {
    let mut answer = 0usize;
    let sent = SendMessageTimeoutW(
        ime,
        WM_IME_CONTROL,
        windows::Win32::Foundation::WPARAM(request),
        windows::Win32::Foundation::LPARAM(value),
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        IME_REQUEST_LIMIT_MS,
        Some(&mut answer),
    );
    (sent.0 != 0).then_some(answer)
}

/// How long the reopen request stays pending for a target too busy to take it within
/// `IME_REQUEST_LIMIT_MS`.
const IME_REOPEN_PENDING_MS: u32 = 30_000;

impl Drop for ImeGuard {
    /// Reopens the input method. A target busy with earlier input may not take the request within
    /// `IME_REQUEST_LIMIT_MS`, and a request whose sender stopped waiting may be dropped instead of
    /// handled late (asynchronous sends are refused for this message). So a thread of its own
    /// keeps the request pending for up to `IME_REOPEN_PENDING_MS` while `press_key` answers; the
    /// target takes it the next time it gets a message, after any earlier request. Registered in
    /// `PENDING_IME_REOPENS` while it runs, so a `suspend` on the same target that follows can wait
    /// for it instead of racing it.
    fn drop(&mut self) {
        if !self.was_open || self.ime.0.is_null() {
            return;
        }
        unsafe {
            if ime_request(self.ime, IMC_SETOPENSTATUS, 1).is_some() {
                return;
            }
        }
        let ime = self.ime.0 as isize;
        let finish = FinishReopen(PendingReopen::register(ime));
        std::thread::spawn(move || {
            // Finishes the reopen however this thread ends.
            let _finish = finish;
            unsafe {
                let hwnd = HWND(ime as *mut core::ffi::c_void);
                let mut answer = 0usize;
                let sent = SendMessageTimeoutW(
                    hwnd,
                    WM_IME_CONTROL,
                    windows::Win32::Foundation::WPARAM(IMC_SETOPENSTATUS),
                    windows::Win32::Foundation::LPARAM(1),
                    SMTO_BLOCK,
                    IME_REOPEN_PENDING_MS,
                    Some(&mut answer),
                );
                if sent.0 == 0 {
                    log(format_args!("input method: the target did not take the reopen request within {IME_REOPEN_PENDING_MS} ms"));
                }
            }
        });
    }
}

struct EnglishLayout {
    hwnd: HWND,
    thread: u32,
    original: HKL,
    loaded: HKL,
    unload: bool,
}

impl EnglishLayout {
    unsafe fn switch(hwnd: HWND) -> Result<Self, String> {
        let thread = GetWindowThreadProcessId(hwnd, None);
        let original = GetKeyboardLayout(thread);
        let mut existing = [HKL::default(); 32];
        let count = GetKeyboardLayoutList(Some(&mut existing));
        let english_id: Vec<u16> = "00000409".encode_utf16().chain(std::iter::once(0)).collect();
        let loaded = LoadKeyboardLayoutW(windows::core::PCWSTR(english_id.as_ptr()), KLF_NOTELLSHELL).map_err(|error| error.to_string())?;
        let unload = count < 0 || !existing.iter().take(count.max(0) as usize).any(|layout| layout == &loaded);
        let guard = Self { hwnd, thread, original, loaded, unload };
        let window = layout_window(thread, hwnd).ok_or_else(|| "The target window is no longer open.".to_string())?;
        guard.request(window, loaded)?;
        Ok(guard)
    }

    /// Posts the layout request to `window` (a window of the target thread) and waits until the
    /// thread is on `layout`.
    unsafe fn request(&self, window: HWND, layout: HKL) -> Result<(), String> {
        if stop_requested() && layout != self.original {
            return Err("Input stopped before the keyboard layout finished switching.".to_string());
        }
        let _ = PostMessageW(
            Some(window),
            WM_INPUTLANGCHANGEREQUEST,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(layout.0 as isize),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            if stop_requested() && layout != self.original {
                return Err("Input stopped before the keyboard layout finished switching.".to_string());
            }
            flush_target_once(window);
            if GetKeyboardLayout(self.thread) == layout {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Err(format!("The keyboard layout request to window {:?} was not handled within 500 ms.", window.0))
    }
}

/// The window of `thread` that takes a layout request: its focus window, where the system itself
/// sends WM_INPUTLANGCHANGEREQUEST when the user switches layouts (a modal dialog ignores the
/// request; its edit control does not). Without one, `preferred` while it is still open on that
/// thread, then any other top-level window the thread still has, visible ones first: the input
/// may have closed the target. Never a window of another thread, so no other app gets the
/// request. None when the thread has no window left.
unsafe fn layout_window(thread: u32, preferred: HWND) -> Option<HWND> {
    let on_thread = |window: HWND| !window.0.is_null() && IsWindow(Some(window)).as_bool() && GetWindowThreadProcessId(window, None) == thread;
    let mut gui = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    if GetGUIThreadInfo(thread, &mut gui).is_ok() && on_thread(gui.hwndFocus) {
        return Some(gui.hwndFocus);
    }
    if on_thread(preferred) {
        return Some(preferred);
    }
    let mut windows: Vec<HWND> = Vec::new();
    let _ = EnumThreadWindows(thread, Some(collect_window), windows::Win32::Foundation::LPARAM(&mut windows as *mut Vec<HWND> as isize));
    windows.sort_by_key(|window| !IsWindowVisible(*window).as_bool());
    windows.into_iter().find(|window| on_thread(*window))
}

unsafe extern "system" fn collect_window(window: HWND, windows: windows::Win32::Foundation::LPARAM) -> windows::core::BOOL {
    (*(windows.0 as *mut Vec<HWND>)).push(window);
    true.into()
}

impl Drop for EnglishLayout {
    /// Restores the thread's layout on every path, including input that closed the target. The
    /// request is posted even when the thread already shows the original layout, so it also
    /// undoes a switch request still queued behind a stop. Failures go to stderr.
    fn drop(&mut self) {
        unsafe {
            match layout_window(self.thread, self.hwnd) {
                Some(window) => {
                    if let Err(error) = self.request(window, self.original) {
                        eprintln!(
                            "keyboard layout: could not restore {:#x} on thread {} ({error}); it is on {:#x}",
                            self.original.0 as usize,
                            self.thread,
                            GetKeyboardLayout(self.thread).0 as usize
                        );
                    }
                }
                None if GetKeyboardLayout(self.thread) != self.original => eprintln!(
                    "keyboard layout: thread {} has no window left to restore {:#x} through",
                    self.thread, self.original.0 as usize
                ),
                None => {}
            }
            if self.unload && !self.loaded.0.is_null() {
                if let Err(error) = UnloadKeyboardLayout(self.loaded) {
                    eprintln!("keyboard layout: unloading {:#x} failed ({error})", self.loaded.0 as usize);
                }
            }
        }
    }
}

/// Keys held down; released in reverse order when dropped, so a stop or an error can never leave
/// a modifier down.
#[derive(Default)]
struct HeldKeys {
    keys: Vec<VIRTUAL_KEY>,
}

impl HeldKeys {
    /// Presses all of `keys` in one SendInput.
    unsafe fn press(keys: &[VIRTUAL_KEY]) -> Self {
        let down: Vec<INPUT> = keys.iter().map(|key| key_input(*key, false)).collect();
        if !down.is_empty() {
            let _ = SendInput(&down, std::mem::size_of::<INPUT>() as i32);
        }
        Self { keys: keys.to_vec() }
    }

    /// Presses one more key; false when SendInput did not take it (then it is not released).
    unsafe fn press_one(&mut self, key: VIRTUAL_KEY) -> bool {
        if SendInput(&[key_input(key, false)], std::mem::size_of::<INPUT>() as i32) != 1 {
            return false;
        }
        self.keys.push(key);
        true
    }
}

impl Drop for HeldKeys {
    /// Presses and releases are now up to `PRESS_KEY_LIMIT` apart, so switching to a UAC secure
    /// desktop or a higher-integrity window in between can make the release's `SendInput` fail,
    /// and this is the last chance to take the modifiers back down: retries a few times, a short
    /// moment apart, in case the switch was brief, and reports a release that still did not take
    /// (only ever resending the releases already computed here, never anything else).
    fn drop(&mut self) {
        if self.keys.is_empty() {
            return;
        }
        let up: Vec<INPUT> = self.keys.iter().rev().map(|key| key_input(*key, true)).collect();
        let (released, attempts) = retry_release(|| unsafe { SendInput(&up, std::mem::size_of::<INPUT>() as i32) as usize == up.len() });
        if !released {
            eprintln!("held keys: releasing {:?} did not take after {attempts} attempts; they may still be down", self.keys);
        }
    }
}

/// How many times a failed release is retried, and how long it waits between attempts.
const RELEASE_RETRY_LIMIT: u32 = 5;
const RELEASE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

/// Calls `send` until it returns true or it has been tried `RELEASE_RETRY_LIMIT` times, waiting
/// `RELEASE_RETRY_DELAY` between attempts. Returns whether it succeeded and how many attempts it
/// took.
fn retry_release(mut send: impl FnMut() -> bool) -> (bool, u32) {
    for attempt in 1..=RELEASE_RETRY_LIMIT {
        if send() {
            return (true, attempt);
        }
        if attempt < RELEASE_RETRY_LIMIT {
            std::thread::sleep(RELEASE_RETRY_DELAY);
        }
    }
    (false, RELEASE_RETRY_LIMIT)
}

/// A reopen still pending for a target's input method window (`ImeGuard::drop`'s background
/// thread keeping a `SendMessageTimeoutW` waiting up to `IME_REOPEN_PENDING_MS`), so `suspend` on
/// the same target can wait for it instead of acting on a status query that predates it landing.
/// Entries remove themselves once their thread ends, successfully or not.
static PENDING_IME_REOPENS: Mutex<Vec<Arc<PendingReopen>>> = Mutex::new(Vec::new());

struct PendingReopen {
    /// The target's input method window, identifying which target this reopen is for. Only ever
    /// compared here, never sent anything: the message it stands for is still `ImeGuard::drop`'s.
    ime: isize,
    done: Mutex<bool>,
    finished: Condvar,
}

impl PendingReopen {
    fn register(ime: isize) -> Arc<Self> {
        let pending = Arc::new(Self { ime, done: Mutex::new(false), finished: Condvar::new() });
        PENDING_IME_REOPENS.lock().unwrap().push(pending.clone());
        pending
    }

    /// The reopen pending for `ime`, if any (the most recently registered one).
    fn for_target(ime: isize) -> Option<Arc<Self>> {
        PENDING_IME_REOPENS.lock().unwrap().iter().rev().find(|pending| pending.ime == ime).cloned()
    }

    /// Removes this reopen from the registry, then marks it finished, waking anyone waiting on it:
    /// a waiter that sees it finished never finds it still registered.
    /// Called from `FinishReopen::drop`, possibly while a panic unwinds, so a poisoned lock is
    /// used as it is rather than panicking again.
    fn finish(self: &Arc<Self>) {
        PENDING_IME_REOPENS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|pending| !Arc::ptr_eq(pending, self));
        *self.done.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.finished.notify_all();
    }

    /// Waits up to `limit` for this reopen to finish. True if it did. A wakeup before it finished
    /// (spurious) keeps waiting for the time left.
    fn wait(&self, limit: std::time::Duration) -> bool {
        let done = self.done.lock().unwrap();
        let (done, _) = self.finished.wait_timeout_while(done, limit, |done| !*done).unwrap();
        *done
    }
}

/// Finishes its reopen when dropped (F31): the reopen thread holds it, so the entry leaves
/// `PENDING_IME_REOPENS` and its waiters wake however the thread ends, a panic included.
struct FinishReopen(Arc<PendingReopen>);

impl Drop for FinishReopen {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// Writes one line to stderr for the reopen thread and the exit path, ignoring a failed write:
/// `eprintln!` panics when stderr is gone (the server closed the pipe), which would end the
/// thread or the exit early.
fn log(line: std::fmt::Arguments) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// How long the helper waits, in total, for every input method reopen still pending
/// (`ImeGuard::drop`'s background thread) before it exits: long enough that a target only briefly
/// busy still gets its input method back, short enough not to hold up an exit that is happening
/// anyway. Anything still pending after this is left to run on its own for up to
/// `IME_REOPEN_PENDING_MS`, unobserved by the exiting process.
pub const IME_REOPEN_SHUTDOWN_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Waits for every input method reopen still pending, up to `IME_REOPEN_SHUTDOWN_LIMIT` in total.
/// Called on both paths the helper exits by (stdin EOF, the server going away): a reopen still
/// pending after the limit is named on stderr, not silently dropped.
pub fn wait_for_pending_ime_reopens() {
    let deadline = std::time::Instant::now() + IME_REOPEN_SHUTDOWN_LIMIT;
    let pending: Vec<Arc<PendingReopen>> = PENDING_IME_REOPENS.lock().unwrap().clone();
    for reopen in pending {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if !reopen.wait(remaining) {
            log(format_args!("input method: exiting with a reopen still pending for input method window {:#x}", reopen.ime as usize));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows turns a `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` coordinate back into the
    /// pixel `floor(normalized * size / 65536)` from the virtual screen's origin (measured with
    /// SendInput and GetCursorPos on every row and column of a 4480x1535 virtual screen). Every
    /// pixel must come back as itself, whatever the virtual screen's size.
    #[test]
    fn absolute_pointer_input_lands_on_the_aimed_pixel_for_any_virtual_screen_size() {
        for size in (1..=3000).chain([1535, 3840, 4480, 5120, 7680, 11520, 15360]) {
            for offset in 0..size {
                let landed = (normalized(offset, size) as i64 * size as i64) >> 16;
                assert_eq!(landed, offset as i64, "pixel {offset} of a {size}-pixel virtual screen lands on {landed}");
            }
        }
    }

    #[test]
    fn in_a_password_field_character_and_paste_keys_are_content_and_navigation_keys_are_not() {
        let keys = |names: &[&str]| names.iter().map(|name| key_code(name).expect("key")).collect::<Vec<_>>();
        for content in [&["a"][..], &["shift", "z"], &["7"], &["space"], &["ctrl", "alt", "q"], &["ctrl", "alt", "shift", "2"], &["ctrl", "v"], &["ctrl", "shift", "v"], &["shift", "insert"], &["win", "v"]] {
            assert!(sends_content(&keys(content)), "{content:?}");
        }
        for navigation in [
            &["tab"][..], &["shift", "tab"], &["return"], &["escape"], &["left"], &["up"], &["home"], &["end"], &["pagedown"], &["backspace"], &["delete"], &["insert"],
            &["f5"], &["ctrl", "a"], &["ctrl", "c"], &["alt", "f"], &["win", "d"], &["ctrl", "insert"], &["shift"],
        ] {
            assert!(!sends_content(&keys(navigation)), "{navigation:?}");
        }
    }

    /// A background wait (as `press_attached` leaves) that ends only when it is cancelled, or on
    /// its own after `finishes_after`; returns it and whether it saw the cancel.
    fn late_release(finishes_after: std::time::Duration) -> (LateRelease, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (cancelled, saw_cancel) = (cancel.clone(), Arc::new(AtomicBool::new(false)));
        let saw = saw_cancel.clone();
        let worker = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < finishes_after {
                if cancelled.load(Ordering::SeqCst) {
                    saw.store(true, Ordering::SeqCst);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        (LateRelease { cancel, worker }, saw_cancel)
    }

    #[test]
    fn ending_a_late_release_that_finishes_within_the_wait_does_not_cancel_it() {
        let (late, saw_cancel) = late_release(std::time::Duration::from_millis(50));
        let started = std::time::Instant::now();
        late.end(std::time::Duration::from_secs(2));
        assert!(!saw_cancel.load(Ordering::SeqCst), "the wait was cancelled although it finished in time");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "ending took {:?}", started.elapsed());
    }

    #[test]
    fn ending_a_late_release_that_does_not_finish_within_the_wait_cancels_it_and_joins() {
        let (late, saw_cancel) = late_release(std::time::Duration::from_secs(30));
        let started = std::time::Instant::now();
        late.end(std::time::Duration::from_millis(100));
        let took = started.elapsed();
        assert!(saw_cancel.load(Ordering::SeqCst), "the wait was not cancelled, or ending returned before its thread ended");
        assert!(took >= std::time::Duration::from_millis(100) && took < std::time::Duration::from_secs(1), "ending took {took:?}");
    }

    #[test]
    fn retry_release_stops_once_send_succeeds() {
        let mut calls = 0;
        let (released, attempts) = retry_release(|| {
            calls += 1;
            calls == 3
        });
        assert!(released);
        assert_eq!(attempts, 3);
        assert_eq!(calls, 3);
    }

    #[test]
    fn retry_release_gives_up_after_the_limit() {
        let mut calls = 0;
        let (released, attempts) = retry_release(|| {
            calls += 1;
            false
        });
        assert!(!released);
        assert_eq!(attempts, RELEASE_RETRY_LIMIT);
        assert_eq!(calls, RELEASE_RETRY_LIMIT);
    }

    /// `suspend` must not query the target's status while an earlier reopen might still be ahead
    /// of it in the target's queue; this is the registration and waiting it uses to notice one,
    /// independent of any real window (`helper/tests/keyboard.rs`'s
    /// `press_key_after_a_delayed_ime_reopen_does_not_let_the_ime_compose_the_next_key` covers the
    /// real race, and could not reproduce the exact IMC_GETOPENSTATUS timing the review assumed;
    /// see its comment).
    #[test]
    fn suspend_waits_for_a_pending_reopen_on_the_same_target_and_times_out_when_it_does_not_finish() {
        let ime = 0x1234;
        let pending = PendingReopen::register(ime);
        let found = PendingReopen::for_target(ime).expect("registered reopen must be found by its target");
        assert!(Arc::ptr_eq(&found, &pending));
        let waited_in_time = pending.wait(std::time::Duration::from_millis(50));
        assert!(!waited_in_time, "must time out: nothing finished the reopen yet");
        // Still registered: a timed-out wait must not remove someone else's entry.
        assert!(PendingReopen::for_target(ime).is_some());
        pending.finish();
        assert!(PendingReopen::for_target(ime).is_none(), "a finished reopen must remove itself");
    }

    #[test]
    fn suspend_does_not_wait_for_a_pending_reopen_registered_for_a_different_target() {
        let ime = 0x5678;
        let other = 0x9999;
        let pending = PendingReopen::register(other);
        assert!(PendingReopen::for_target(ime).is_none(), "must not find a reopen registered for a different target");
        pending.finish();
    }

    #[test]
    fn a_pending_reopen_that_finishes_in_time_is_reported_as_finished() {
        let ime = 0xabcd;
        let pending = PendingReopen::register(ime);
        let waiter = pending.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            waiter.finish();
        });
        let waited_in_time = pending.wait(std::time::Duration::from_secs(1));
        assert!(waited_in_time);
        assert!(PendingReopen::for_target(ime).is_none());
    }

    /// A wakeup without the reopen finishing (a spurious one, here a bare notify) must not end
    /// the wait early: it keeps waiting, for the time left, until the reopen finishes.
    #[test]
    fn a_wakeup_before_the_reopen_finishes_does_not_end_the_wait() {
        let ime = 0xbcde;
        let pending = PendingReopen::register(ime);
        let waiter = pending.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            waiter.finished.notify_all();
            std::thread::sleep(std::time::Duration::from_millis(50));
            waiter.finish();
        });
        assert!(pending.wait(std::time::Duration::from_secs(2)));
        assert!(PendingReopen::for_target(ime).is_none());
    }

    /// F31: a reopen thread that ends in a panic (before the logging stopped panicking, a write
    /// to a closed stderr did) still finishes its reopen, so nothing waits on it or finds it
    /// registered afterwards.
    #[test]
    fn a_reopen_thread_that_panics_still_finishes_its_reopen() {
        let ime = 0xcdef;
        let pending = PendingReopen::register(ime);
        let finish = FinishReopen(pending.clone());
        let ended = std::thread::spawn(move || {
            let _finish = finish;
            panic!("the reopen thread panicked");
        })
        .join();
        assert!(ended.is_err(), "the thread was meant to panic");
        assert!(pending.wait(std::time::Duration::ZERO), "the reopen was not marked finished");
        assert!(PendingReopen::for_target(ime).is_none(), "the reopen is still registered");
    }

    /// F14: the foreground steps stop as soon as the target is in front, so ALT goes only with a
    /// step after one that left the target behind: never once `BringWindowToTop` brought it forward,
    /// and never before the first plain attempt.
    #[test]
    fn foreground_steps_stop_once_the_target_is_in_front() {
        use ForegroundStep::*;
        let ran = |in_front_after: Option<ForegroundStep>| {
            let ran = std::cell::RefCell::new(Vec::new());
            run_foreground_steps(|step| ran.borrow_mut().push(step), || in_front_after.is_some_and(|last| ran.borrow().last() == Some(&last)));
            ran.into_inner()
        };
        assert_eq!(ran(Some(BringToTop)), [Set, AltThenSet, SwitchTo, BringToTop]);
        assert_eq!(ran(Some(Set)), [Set]);
        assert_eq!(ran(None), [Set, AltThenSet, SwitchTo, BringToTop, AltThenSet]);
    }

    /// Ticket 82: a lone ALT released puts the target in menu or keytip mode, which eats the keys
    /// after it, so `tap_alt` presses an unassigned key while ALT is down (AutoHotkey's
    /// MenuMaskKey): the system then does not take it for a lone ALT.
    #[test]
    fn tap_alt_masks_alt_with_an_unassigned_key() {
        const MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
        assert_eq!(alt_tap_events(), [(VK_MENU, false), (MASK, false), (MASK, true), (VK_MENU, true)]);
    }

    /// Whatever part of `tap_alt`'s events SendInput took, what it pressed and did not release is
    /// released, last pressed first, and nothing else.
    #[test]
    fn left_down_is_what_the_taken_events_pressed_and_did_not_release() {
        const MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
        let events = [(VK_MENU, false), (MASK, false), (MASK, true), (VK_MENU, true)];
        assert_eq!(left_down(&events, 0), []);
        assert_eq!(left_down(&events, 1), [VK_MENU]);
        assert_eq!(left_down(&events, 2), [MASK, VK_MENU]);
        assert_eq!(left_down(&events, 3), [VK_MENU]);
        assert_eq!(left_down(&events, 4), []);
    }

    /// F13: both halves of a surrogate pair go in one SendInput, after the checks before the
    /// character, so no check between them can leave the high half alone; any other character is
    /// one press and release.
    #[test]
    fn a_surrogate_pair_is_one_key_that_sends_both_halves_together() {
        let scans = |character: char| -> Vec<(u16, bool)> {
            character_key(character).inputs().iter().map(|input| unsafe { (input.Anonymous.ki.wScan, input.Anonymous.ki.dwFlags.contains(KEYEVENTF_KEYUP)) }).collect()
        };
        assert_eq!(scans('😀'), [(0xD83D, false), (0xD83D, true), (0xDE00, false), (0xDE00, true)]);
        assert_eq!(scans('你'), [(0x4F60, false), (0x4F60, true)]);
    }

    /// A covering window says whether it stays on top (`WS_EX_TOPMOST`), in `covered_by`
    /// (`describe_cover`) and in `target_obscured`'s data (same `is_topmost`; the always-on-top
    /// case is end to end in `helper/tests/obscured.rs`). A raise never leaves an ordinary window
    /// over the fixture in the interactive tests, and a background process cannot place a window
    /// above the foreground app's, so the ordinary case uses plain windows and no hit test.
    /// Ticket 34: the keys for a UWP app's frame go through the app's CoreWindow inside it, an
    /// ordinary window takes its own, and a frame whose app is unknown is refused, never typed
    /// into through the frame host.
    #[test]
    fn the_keyboard_window_is_the_core_window_inside_a_frame_and_the_target_otherwise() {
        let frame = HWND(0x1000 as *mut core::ffi::c_void);
        let core = HWND(0x2000 as *mut core::ffi::c_void);
        let hosted = Identity::Hosted { path: r"C:\Apps\CalculatorApp.exe".to_string(), pid: 7, input: core };
        assert_eq!(keyboard_window_of(frame, Some(&hosted)), Ok(core));
        assert_eq!(keyboard_window_of(frame, Some(&Identity::Own { path: r"C:\Windows\notepad.exe".to_string() })), Ok(frame));
        assert_eq!(keyboard_window_of(frame, None), Ok(frame));
        let unresolved = Identity::Unresolved { frame: r"C:\Windows\System32\ApplicationFrameHost.exe".to_string() };
        assert_eq!(keyboard_window_of(frame, Some(&unresolved)).map_err(|(code, _)| code), Err("app_unresolved".to_string()));
    }

    /// Keys for a frame stop once the CoreWindow that takes them is no longer inside it.
    #[test]
    fn the_keyboard_window_counts_as_the_target_only_while_inside_it() {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_CHILD, WS_POPUP};
        unsafe {
            let make = |style, parent: Option<HWND>| {
                CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("cumcp-unit"), style, 0, 0, 50, 50, parent, None, None, None).expect("a window")
            };
            let frame = make(WS_POPUP, None);
            let inside = make(WS_CHILD, Some(frame));
            // Ticket 55 (F8): only a direct child, as `identify` picks the CoreWindow.
            let deeper = make(WS_CHILD, Some(inside));
            let elsewhere = make(WS_POPUP, None);
            assert!(!input_left_target(frame, frame));
            assert!(!input_left_target(frame, inside));
            assert!(input_left_target(frame, deeper));
            assert!(input_left_target(frame, elsewhere));
            for window in [elsewhere, frame] {
                let _ = DestroyWindow(window);
            }
        }
    }

    /// Ticket 137: a disabled target with no window to name still has a dialog open (`dialog_open`
    /// without a `window_id`), and an enabled one with none owned has none. (Naming a dialog needs
    /// a listed window, which the helper's own process never has: `helper/tests/background_checks.rs`.)
    #[test]
    fn a_disabled_target_without_a_nameable_dialog_is_dialog_open_without_a_window_id() {
        use windows::core::w;
        use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE};
        unsafe {
            let target = CreateWindowExW(WINDOW_EX_STYLE(0) | WS_EX_NOACTIVATE, w!("STATIC"), w!("cumcp-unit"), WS_POPUP | WS_VISIBLE, 0, 0, 50, 50, None, None, None, None)
                .expect("a window on the test desktop");
            assert_eq!(blocking_dialog(target), None);
            assert!(ensure_no_dialog_anywhere(target).is_ok());
            let _ = EnableWindow(target, false);
            assert_eq!(blocking_dialog(target), Some(None));
            let failure = ensure_no_dialog_anywhere(target).expect_err("a disabled target has a dialog open");
            assert_eq!(failure.code, "dialog_open");
            assert!(failure.data.is_none(), "{:?}", failure.data);
            let _ = DestroyWindow(target);
        }
    }

    #[test]
    fn a_covering_window_reports_whether_it_is_topmost() {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE};
        let make = |ex_style: WINDOW_EX_STYLE| unsafe {
            CreateWindowExW(ex_style | WS_EX_NOACTIVATE, w!("BUTTON"), w!("cumcp-unit"), WS_POPUP | WS_VISIBLE, 100, 100, 200, 200, None, None, None, None)
                .expect("a window on the test desktop")
        };
        let ordinary = make(WINDOW_EX_STYLE(0));
        let always_on_top = make(WS_EX_TOPMOST);
        assert_eq!(unsafe { describe_cover(ordinary) }["topmost"], serde_json::json!(false));
        assert_eq!(unsafe { describe_cover(always_on_top) }["topmost"], serde_json::json!(true));
        for window in [always_on_top, ordinary] {
            let _ = unsafe { DestroyWindow(window) };
        }
    }
}
