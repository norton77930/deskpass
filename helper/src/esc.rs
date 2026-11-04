//! The stop keys and the stop state.
//!
//! The stop keys (ticket 145, spec-background "Stop"): a physical Ctrl+Alt+Esc stops at any time;
//! a physical single Esc stops only while a foreground action is in progress ([`ForegroundAction`])
//! or the border of the most recent foreground action shows (`overlay::foreground_border_shown`),
//! and otherwise is the user's key ([`is_stop_key`]). The hook swallows one key: the Esc of a physical
//! Ctrl+Alt+Esc that counts as a stop (with its repeats and its release), because Windows would
//! take it as Alt+Esc and send the user's front window to the back. Every other key, a single Esc
//! included, passes on.
//!
//! Three threads touch the stop state:
//! - the keyboard hook thread (a stop key), the stdin reader (`debug_stop` in debug builds) and
//!   the server watchdog call [`stop`]: one atomic increment and an unpark, no lock and no I/O,
//!   so a slow stdout write can never stall the system-wide keyboard hook;
//! - the notifier thread wakes up, answers the in-flight request with `interrupted` and writes the
//!   `stopped` event (it takes the response lock, the hook never does);
//! - the executing (main) thread checks [`stopped`] before dispatching an input method and before
//!   every SendInput, and only it clears the state (`resume`, `session_start`);
//! - the UI Automation worker marks, under the response lock, that an element action is about to
//!   be taken ([`begin_action`]), and the executing thread sends a click's or a drag's button press
//!   and marks it there too ([`act`], ticket 73), so the notifier answers a later stop with
//!   [`STOPPED_AFTER_ACTION_MESSAGE`] instead (ticket 70).
//!
//! The state is a pair of monotonic counters instead of a bare bool so that `resume` can name the
//! stop it acknowledges: an Esc that arrives after the server decided to resume keeps input stopped.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::{JoinHandle, Thread};

use serde::Serialize;
use serde_json::{json, Value};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const VK_ESCAPE: u32 = 0x1B;
const VK_CONTROL_ANY: u32 = 0x11;
const VK_MENU_ANY: u32 = 0x12;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
/// Test builds only (seam B): an injected key event whose `dwExtraInfo` is this is taken as
/// physical, so a test's SendInput can stand for the user's keyboard through the real hook. The
/// helper's own input carries 0.
#[cfg(debug_assertions)]
pub(crate) const DEBUG_PHYSICAL_MARK: usize = 0x4445_534B_5048_5953;
pub(crate) const INTERRUPTED_MESSAGE: &str =
    "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";
/// `interrupted` for a stop between an element action and its read-back (ticket 65 review minor
/// 3): the action was taken, so it is not to be done again. The worker answers it when its
/// read-back sees the stop, and the notifier when the stop comes after [`begin_action`] (ticket 70)
/// or after a click's or a drag's button press was sent ([`act`], ticket 73).
pub(crate) const STOPPED_AFTER_ACTION_MESSAGE: &str = "The user pressed Esc and stopped Deskpass after the action was taken, so its result was not read back. Do not repeat the action; once the user lets you continue, take a screenshot to check its result. Do not call these tools again now: end this turn and tell the user.";

/// How many stops have been requested since the helper started.
static STOPS: AtomicU64 = AtomicU64::new(0);
/// The stop count last cleared by `resume` or `session_start`. Input is stopped while
/// `STOPS > CLEARED`. Only the executing thread writes it.
static CLEARED: AtomicU64 = AtomicU64::new(0);
/// A physical Esc is down, so its auto-repeat is not a new stop.
static ESC_HELD: AtomicBool = AtomicBool::new(false);
/// The physical Esc now down was swallowed as a Ctrl+Alt+Esc stop, so its repeats and its release
/// are swallowed too.
static ESC_SWALLOWED: AtomicBool = AtomicBool::new(false);
/// The physical Ctrl (`CTRL`) and Alt (`ALT`) keys held, as the hook saw them pressed and released.
static MODIFIERS: AtomicU8 = AtomicU8::new(0);
const CTRL: u8 = 1;
const ALT: u8 = 2;
/// A foreground input request is being executed (`ForegroundAction`).
static FOREGROUND_ACTION: AtomicBool = AtomicBool::new(false);
static NOTIFIER: OnceLock<Thread> = OnceLock::new();

struct InFlight {
    id: u64,
    /// The stop count when the request began; only a later stop interrupts it.
    stops_at_start: u64,
    responded: bool,
    /// The request's element action is taken (or being taken), or its button press was sent: set
    /// by [`begin_action`] and [`act`].
    acted: bool,
}

/// The request being executed. Shared by the executing thread and the notifier, both of which
/// write stdout while holding it; the keyboard hook never takes it.
static RESPONSES: Mutex<Option<InFlight>> = Mutex::new(None);

struct HookThread {
    id: u32,
    join: Option<JoinHandle<()>>,
}

static HOOK: Mutex<Option<HookThread>> = Mutex::new(None);

/// Stops all input: the one path shared by the physical Esc, `debug_stop` and a vanished server.
/// Lock-free and I/O-free, so it is safe inside the low-level keyboard hook.
pub fn stop() {
    STOPS.fetch_add(1, Ordering::SeqCst);
    if let Some(notifier) = NOTIFIER.get() {
        notifier.unpark();
    }
}

/// How many stops there have been; a later value than one read earlier means a new stop.
pub fn stop_count() -> u64 {
    STOPS.load(Ordering::SeqCst)
}

pub fn stopped() -> bool {
    STOPS.load(Ordering::SeqCst) > CLEARED.load(Ordering::SeqCst)
}

/// Clears stops up to `generation` (the latest `stopped` event the server saw), or all stops when
/// it is absent. A stop newer than `generation` stays in force and the call fails `interrupted`.
pub fn resume(params: &Value) -> Result<Value, (String, String)> {
    let current = STOPS.load(Ordering::SeqCst);
    let target = params.get("generation").and_then(Value::as_u64).map_or(current, |generation| generation.min(current));
    CLEARED.fetch_max(target, Ordering::SeqCst);
    if stopped() {
        return Err(("interrupted".to_string(), INTERRUPTED_MESSAGE.to_string()));
    }
    Ok(json!({}))
}

fn clear_all() {
    CLEARED.fetch_max(STOPS.load(Ordering::SeqCst), Ordering::SeqCst);
}

/// Starts the thread that reports stops. Called once, before any request is read.
pub fn start_notifier() {
    let thread = std::thread::spawn(notify_stops);
    let _ = NOTIFIER.set(thread.thread().clone());
}

fn notify_stops() {
    let mut reported = 0;
    loop {
        std::thread::park();
        let current = STOPS.load(Ordering::SeqCst);
        if current <= reported {
            continue;
        }
        reported = current;
        let mut flight = lock_responses();
        if let Some(flight) = flight.as_mut() {
            if !flight.responded && flight.stops_at_start < current {
                flight.responded = true;
                let message = if flight.acted { STOPPED_AFTER_ACTION_MESSAGE } else { INTERRUPTED_MESSAGE };
                write_stdout(&json!({
                    "id": flight.id,
                    "ok": false,
                    "error": { "code": "interrupted", "message": message },
                }));
            }
        }
        write_stdout(&json!({ "event": "stopped", "data": { "generation": current } }));
    }
}

pub fn begin_request(id: u64) {
    *lock_responses() = Some(InFlight { id, stops_at_start: STOPS.load(Ordering::SeqCst), responded: false, acted: false });
}

/// The id of the request being executed, if any.
pub fn current_request() -> Option<u64> {
    lock_responses().as_ref().map(|flight| flight.id)
}

/// Ticket 70: just before an element action's own call, on the worker. Refuses (false) when input
/// is stopped; otherwise marks request `id` (when it is still the one being executed) as acting, so
/// a stop from here on is answered with [`STOPPED_AFTER_ACTION_MESSAGE`]. The check and the mark
/// are one step under the response lock, which the notifier holds while it answers: a stop is
/// either seen here (and the action is not taken) or answered after the mark, never in between.
pub fn begin_action(id: Option<u64>) -> bool {
    act(id, || Ok::<(), ()>(())).is_some()
}

/// Ticket 73: [`begin_action`] for pointer input, with the input that takes the action (a click's
/// or a drag's button press). None, with `send` not called, when input is stopped; otherwise
/// `send` runs and request `id` is marked as acting only when it reports the input sent. The stop
/// check, `send` and the mark are one step under the response lock: a stop is either seen here
/// (nothing sent, the plain stop text) or answered by the notifier once this step is over, with
/// the after-action text exactly when the input was sent. `send` must not take the response lock;
/// a SendInput call holds the notifier's answer back only as long as it takes.
pub fn act<T, E>(id: Option<u64>, send: impl FnOnce() -> Result<T, E>) -> Option<Result<T, E>> {
    let mut flight = lock_responses();
    if stopped() {
        return None;
    }
    let sent = send();
    if sent.is_ok() {
        if let Some(flight) = flight.as_mut().filter(|flight| Some(flight.id) == id) {
            flight.acted = true;
        }
    }
    Some(sent)
}

pub fn finish_request(response: &impl Serialize) {
    let mut flight = lock_responses();
    let already = flight.as_ref().is_some_and(|flight| flight.responded);
    *flight = None;
    if !already {
        write_stdout(response);
    }
}

/// Ticket 145: while one lives, a foreground action is in progress and a physical single Esc stops.
/// Made by the executing thread around a foreground input request (`main`).
pub struct ForegroundAction(());

impl ForegroundAction {
    pub fn begin() -> ForegroundAction {
        FOREGROUND_ACTION.store(true, Ordering::SeqCst);
        ForegroundAction(())
    }
}

impl Drop for ForegroundAction {
    fn drop(&mut self) {
        FOREGROUND_ACTION.store(false, Ordering::SeqCst);
    }
}

/// Ticket 145: whether a physical Esc press is a stop. With Ctrl and Alt physically held it always
/// is (Ctrl+Alt+Esc); alone only during a foreground action or while its border shows.
pub(crate) fn is_stop_key(ctrl_alt_held: bool, foreground: bool) -> bool {
    ctrl_alt_held || foreground
}

pub fn session_start() -> Result<Value, (String, String)> {
    let mut slot = lock_hook();
    ESC_HELD.store(false, Ordering::SeqCst);
    ESC_SWALLOWED.store(false, Ordering::SeqCst);
    MODIFIERS.store(0, Ordering::SeqCst);
    clear_all();
    if slot.is_some() {
        return Ok(json!({}));
    }
    crate::idle::reset();
    let (tx, rx) = mpsc::channel();
    let join = std::thread::spawn(move || hook_main(tx));
    let thread_id = match rx.recv() {
        Ok(Ok(id)) => id,
        Ok(Err(message)) => {
            drop(slot);
            let _ = join.join();
            return Err(("internal".to_string(), message));
        }
        Err(error) => {
            drop(slot);
            let _ = join.join();
            return Err(("internal".to_string(), error.to_string()));
        }
    };
    *slot = Some(HookThread { id: thread_id, join: Some(join) });
    Ok(json!({}))
}

pub fn session_end() -> Result<Value, (String, String)> {
    let existing = {
        let mut slot = lock_hook();
        slot.take()
    };
    let Some(mut existing) = existing else {
        return Ok(json!({}));
    };
    unsafe {
        let _ = PostThreadMessageW(existing.id, WM_QUIT, WPARAM(0), LPARAM(0));
    }
    if let Some(join) = existing.join.take() {
        let _ = join.join();
    }
    ESC_HELD.store(false, Ordering::SeqCst);
    ESC_SWALLOWED.store(false, Ordering::SeqCst);
    MODIFIERS.store(0, Ordering::SeqCst);
    crate::idle::reset();
    Ok(json!({}))
}

pub(crate) fn is_physical_esc_down(message: u32, vk: u32, flags: u32) -> bool {
    let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    down && vk == VK_ESCAPE && flags & LLKHF_INJECTED.0 == 0
}

fn is_physical_esc_up(message: u32, vk: u32, flags: u32) -> bool {
    let up = message == WM_KEYUP || message == WM_SYSKEYUP;
    up && vk == VK_ESCAPE && flags & LLKHF_INJECTED.0 == 0
}

/// The modifier bit of a Ctrl or Alt key (either side), or 0 for any other key.
fn modifier_of(vk: u32) -> u8 {
    match vk {
        VK_CONTROL_ANY | VK_LCONTROL | VK_RCONTROL => CTRL,
        VK_MENU_ANY | VK_LMENU | VK_RMENU => ALT,
        _ => 0,
    }
}

/// The hook's flags for the event, with the injected flag cleared for a test's marked key
/// ([`DEBUG_PHYSICAL_MARK`], test builds only).
#[cfg_attr(not(debug_assertions), allow(unused_variables))]
fn effective_flags(flags: u32, extra: usize) -> u32 {
    #[cfg(debug_assertions)]
    if extra == DEBUG_PHYSICAL_MARK {
        return flags & !LLKHF_INJECTED.0;
    }
    flags
}

fn hook_main(ready: mpsc::Sender<Result<u32, String>>) {
    unsafe {
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        let module = match GetModuleHandleW(None) {
            Ok(module) => module,
            Err(error) => {
                let _ = ready.send(Err(error.to_string()));
                return;
            }
        };
        let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(HINSTANCE(module.0)), 0) {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready.send(Err(error.to_string()));
                return;
            }
        };
        // Ticket 149: physical mouse buttons for the background actions' wait (`idle`), on this
        // thread so it is set and removed with the keyboard hook. Without it the wait sees the
        // keyboard only; the stop key still works.
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(crate::idle::mouse_hook_proc), Some(HINSTANCE(module.0)), 0)
            .map_err(|error| eprintln!("idle wait: no mouse hook ({error}), so a held mouse button is not waited for"))
            .ok();
        let unhook = || {
            if let Some(mouse) = mouse {
                let _ = UnhookWindowsHookEx(mouse);
            }
            let _ = UnhookWindowsHookEx(hook);
        };
        if ready.send(Ok(GetCurrentThreadId())).is_err() {
            unhook();
            return;
        }
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {}
        unhook();
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let swallow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if code == HC_ACTION as i32 && lparam.0 != 0 {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            crate::idle::on_key(info.flags.0);
            return on_key(wparam.0 as u32, info.vkCode, effective_flags(info.flags.0, info.dwExtraInfo));
        }
        false
    }))
    .unwrap_or(false);
    if swallow {
        // A nonzero result keeps the key from Windows and from every later hook and app.
        return LRESULT(1);
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Runs inside the low-level keyboard hook: atomics, two `GetAsyncKeyState` reads and an unpark only.
/// Returns whether to swallow the key: the Esc of a Ctrl+Alt+Esc stop, its repeats and its release.
fn on_key(message: u32, vk: u32, flags: u32) -> bool {
    let modifier = modifier_of(vk);
    if modifier != 0 && flags & LLKHF_INJECTED.0 == 0 {
        if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
            MODIFIERS.fetch_or(modifier, Ordering::SeqCst);
        } else if message == WM_KEYUP || message == WM_SYSKEYUP {
            MODIFIERS.fetch_and(!modifier, Ordering::SeqCst);
        }
        return false;
    }
    if is_physical_esc_up(message, vk, flags) {
        ESC_HELD.store(false, Ordering::SeqCst);
        return ESC_SWALLOWED.swap(false, Ordering::SeqCst);
    }
    if !is_physical_esc_down(message, vk, flags) {
        return false;
    }
    let repeat = ESC_HELD.swap(true, Ordering::SeqCst);
    // Ticket 153: whether the press is swallowed is decided at its first down, and its repeats and
    // its release follow, whatever order Ctrl, Alt and Esc are let go in: the app sees a press with
    // its release or neither. A repeat with Ctrl and Alt held is still swallowed (Windows would take
    // it as Alt+Esc), but a press whose first down passed keeps its release.
    let swallowed_press = repeat && ESC_SWALLOWED.load(Ordering::SeqCst);
    let foreground = FOREGROUND_ACTION.load(Ordering::SeqCst) || crate::overlay::foreground_border_shown();
    let ctrl_alt = ctrl_alt_held();
    if !is_stop_key(ctrl_alt, foreground) {
        return swallowed_press;
    }
    if !repeat || !stopped() {
        stop();
    }
    if ctrl_alt && !repeat {
        ESC_SWALLOWED.store(true, Ordering::SeqCst);
    }
    ctrl_alt || swallowed_press
}

/// Ctrl and Alt are both physically held: the hook saw them pressed and not released, and
/// `GetAsyncKeyState` (which cannot tell injected from physical) agrees they are down, so a release
/// the hook missed (on the secure desktop after Ctrl+Alt+Del, say) does not leave them held.
fn ctrl_alt_held() -> bool {
    let down = |key: u16| unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000 != 0;
    MODIFIERS.load(Ordering::SeqCst) == CTRL | ALT && down(VK_CONTROL.0) && down(VK_MENU.0)
}

fn write_stdout(value: &impl Serialize) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{}", serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()));
    let _ = stdout.flush();
}

fn lock_responses() -> std::sync::MutexGuard<'static, Option<InFlight>> {
    RESPONSES.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn lock_hook() -> std::sync::MutexGuard<'static, Option<HookThread>> {
    HOOK.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests that stop and resume share the one stop count, so they take turns.
    static STOPPING: Mutex<()> = Mutex::new(());

    #[test]
    fn only_a_non_injected_escape_press_is_a_physical_stop() {
        assert!(is_physical_esc_down(WM_KEYDOWN, VK_ESCAPE, 0));
        assert!(is_physical_esc_down(WM_SYSKEYDOWN, VK_ESCAPE, 0));
        assert!(!is_physical_esc_down(WM_KEYDOWN, VK_ESCAPE, LLKHF_INJECTED.0));
        assert!(!is_physical_esc_down(WM_KEYUP, VK_ESCAPE, 0));
        assert!(!is_physical_esc_down(WM_KEYDOWN, b'A' as u32, 0));
    }

    #[test]
    fn ctrl_alt_esc_stops_at_any_time_and_a_single_esc_only_in_the_foreground() {
        assert!(is_stop_key(true, false), "Ctrl+Alt+Esc stops in the background");
        assert!(is_stop_key(true, true), "Ctrl+Alt+Esc stops in the foreground");
        assert!(is_stop_key(false, true), "a single Esc stops in the foreground");
        assert!(!is_stop_key(false, false), "a single Esc in the background is the user's");
    }

    #[test]
    fn ctrl_and_alt_of_either_side_are_modifiers() {
        for vk in [VK_CONTROL_ANY, VK_LCONTROL, VK_RCONTROL] {
            assert_eq!(modifier_of(vk), CTRL);
        }
        for vk in [VK_MENU_ANY, VK_LMENU, VK_RMENU] {
            assert_eq!(modifier_of(vk), ALT);
        }
        assert_eq!(modifier_of(VK_ESCAPE), 0);
        assert_eq!(modifier_of(0x10), 0, "Shift is not one");
    }

    #[test]
    fn only_a_marked_injected_key_is_taken_as_physical_in_test_builds() {
        assert_eq!(effective_flags(LLKHF_INJECTED.0, 0), LLKHF_INJECTED.0);
        assert_eq!(effective_flags(LLKHF_INJECTED.0, DEBUG_PHYSICAL_MARK), 0);
        assert_eq!(effective_flags(0, 0), 0);
    }

    #[test]
    fn resume_clears_only_the_stops_the_server_saw() {
        let _turn = STOPPING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = STOPS.load(Ordering::SeqCst);
        stop();
        assert!(stopped());
        stop();
        let resumed = resume(&json!({ "generation": before + 1 }));
        assert_eq!(resumed.map_err(|(code, _)| code), Err("interrupted".to_string()), "a newer Esc must stay in force");
        assert!(stopped());
        assert!(resume(&json!({ "generation": before + 2 })).is_ok());
        assert!(!stopped());
        stop();
        assert!(resume(&json!({})).is_ok(), "resume without a generation clears every stop");
        assert!(!stopped());
    }

    #[test]
    fn a_stopped_action_sends_nothing() {
        let _turn = STOPPING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        stop();
        let mut called = false;
        assert!(act(None, || {
            called = true;
            Ok::<(), ()>(())
        })
        .is_none());
        assert!(!called, "a stopped action must not send its input");
        assert!(resume(&json!({})).is_ok());
        let sent = act(None, || {
            called = true;
            Ok::<u8, ()>(7)
        });
        assert_eq!(sent, Some(Ok(7)));
        assert!(called);
    }
}
