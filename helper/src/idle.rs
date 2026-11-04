//! Ticket 149 (spec-background "wait for the user to stop", ADR 0002): before a background action's UI
//! Automation call, wait until the user has stopped typing and is not holding a mouse button, so
//! keys typed while the target briefly holds the foreground (2-9 ms before the restore) do not land
//! in it, and a drag the user is doing is not broken.
//!
//! Only physical input counts: the low-level hooks that feed this module skip events flagged as
//! injected (SendInput, including the helper's own foreground input), as the stop key does. Both
//! hooks live on the stop key's hook thread (`esc::session_start`, removed by `esc::session_end`):
//! the keyboard hook calls [`on_key`], the mouse hook is [`mouse_hook_proc`]. They only store
//! atomics. The executing thread reads them in [`wait_for_idle`], which checks the stop on every
//! look, so a stop during the wait ends it at once and nothing is done.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_XBUTTON1, VK_XBUTTON2};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, LLKHF_INJECTED, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use crate::esc;

/// How long the physical keyboard must have been idle.
pub(crate) const KEYBOARD_IDLE: Duration = Duration::from_millis(300);
/// The longest wait; the action then goes ahead anyway.
pub(crate) const WAIT_CAP: Duration = Duration::from_secs(3);
/// How often the wait looks again (and checks the stop).
const LOOK_EVERY: Duration = Duration::from_millis(5);

/// The user's physical input as the wait sees it at one moment.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct UserInput {
    /// How long ago the last physical key event was; `None` when none was seen.
    pub since_key: Option<Duration>,
    /// A physical mouse button is held.
    pub mouse_held: bool,
}

/// What the wait does after having waited `waited`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Verdict {
    /// The user is idle: act now.
    Go,
    /// The user is typing or holds a button: look again.
    Wait,
    /// The user is still busy after `WAIT_CAP`: act anyway.
    Capped,
}

/// The wait's rule: a physical key less than `KEYBOARD_IDLE` ago, or a held physical mouse button,
/// waits, up to `WAIT_CAP`.
pub(crate) fn verdict(input: UserInput, waited: Duration) -> Verdict {
    let busy = input.mouse_held || input.since_key.is_some_and(|since| since < KEYBOARD_IDLE);
    if !busy {
        Verdict::Go
    } else if waited >= WAIT_CAP {
        Verdict::Capped
    } else {
        Verdict::Wait
    }
}

/// A low-level keyboard event the user made, not one injected (the stop key's rule, `esc`).
pub(crate) fn is_physical_key(flags: u32) -> bool {
    flags & LLKHF_INJECTED.0 == 0
}

/// `MSLLHOOKSTRUCT.flags`: the event was injected (also set for one injected from a lower
/// integrity level).
const LLMHF_INJECTED: u32 = 0x1;

const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const MIDDLE: u8 = 4;
const X1: u8 = 8;
const X2: u8 = 16;

/// The button a physical low-level mouse event presses (`true`) or releases (`false`), as a bit of
/// `BUTTONS`; `None` for an injected event or one that is no button change (a move, the wheel).
pub(crate) fn physical_button(message: u32, mouse_data: u32, flags: u32) -> Option<(u8, bool)> {
    if flags & LLMHF_INJECTED != 0 {
        return None;
    }
    let x_button = || match mouse_data >> 16 {
        1 => Some(X1),
        2 => Some(X2),
        _ => None,
    };
    match message {
        WM_LBUTTONDOWN => Some((LEFT, true)),
        WM_LBUTTONUP => Some((LEFT, false)),
        WM_RBUTTONDOWN => Some((RIGHT, true)),
        WM_RBUTTONUP => Some((RIGHT, false)),
        WM_MBUTTONDOWN => Some((MIDDLE, true)),
        WM_MBUTTONUP => Some((MIDDLE, false)),
        WM_XBUTTONDOWN => x_button().map(|button| (button, true)),
        WM_XBUTTONUP => x_button().map(|button| (button, false)),
        _ => None,
    }
}

/// What the times below count from.
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// When the last physical key event was, in ms since `EPOCH` plus 1; 0 when none was seen.
static LAST_KEY: AtomicU64 = AtomicU64::new(0);
/// The physical mouse buttons held, as the mouse hook saw them pressed and released.
static BUTTONS: AtomicU8 = AtomicU8::new(0);
/// Test builds only (`debug_user_input`): the user types until then (the clock of `LAST_KEY`).
#[cfg(debug_assertions)]
static DEBUG_TYPING_UNTIL: AtomicU64 = AtomicU64::new(0);
/// Test builds only (`debug_user_input`): the user holds a mouse button until then.
#[cfg(debug_assertions)]
static DEBUG_HELD_UNTIL: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// No button is known to be held: one held from before the hooks is not seen, and one released
/// while no hook was in place must not stay held. Called by `esc` before the hooks are set (which
/// also starts the clock, so the hooks never initialize it) and after they are removed.
pub(crate) fn reset() {
    let _ = now_ms();
    BUTTONS.store(0, Ordering::SeqCst);
}

/// From the keyboard hook (`esc`): atomics only.
pub(crate) fn on_key(flags: u32) {
    if is_physical_key(flags) {
        LAST_KEY.store(now_ms(), Ordering::SeqCst);
    }
}

fn on_mouse(message: u32, mouse_data: u32, flags: u32) {
    match physical_button(message, mouse_data, flags) {
        Some((button, true)) => {
            BUTTONS.fetch_or(button, Ordering::SeqCst);
        }
        Some((button, false)) => {
            BUTTONS.fetch_and(!button, Ordering::SeqCst);
        }
        None => {}
    }
}

/// The low-level mouse hook, set on the stop key's hook thread (`esc`): atomics only.
pub(crate) unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if code == HC_ACTION as i32 && lparam.0 != 0 {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            on_mouse(wparam.0 as u32, info.mouseData, info.flags);
        }
    }));
    CallNextHookEx(None, code, wparam, lparam)
}

/// Whether any mouse button is down now, by `GetAsyncKeyState` (which cannot tell injected input
/// from physical). A button the hook saw pressed counts as held only while this agrees, so a
/// release the hook missed (made on the secure desktop, say) does not hold every action up.
fn any_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2].iter().any(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } as u16 & 0x8000 != 0)
}

fn user_input() -> UserInput {
    let now = now_ms();
    #[allow(unused_mut)]
    let mut last_key = LAST_KEY.load(Ordering::SeqCst);
    #[allow(unused_mut)]
    let mut mouse_held = BUTTONS.load(Ordering::SeqCst) != 0 && any_button_down();
    #[cfg(debug_assertions)]
    {
        let typing_until = DEBUG_TYPING_UNTIL.load(Ordering::SeqCst);
        if typing_until != 0 {
            last_key = last_key.max(typing_until.min(now));
        }
        mouse_held |= now < DEBUG_HELD_UNTIL.load(Ordering::SeqCst);
    }
    UserInput { since_key: (last_key != 0).then(|| Duration::from_millis(now.saturating_sub(last_key))), mouse_held }
}

/// How long a background action waited for the user (`waited_for_idle_ms`), and whether it went
/// ahead only because `WAIT_CAP` was reached (`idle_wait_capped`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Waited {
    pub ms: u64,
    pub capped: bool,
}

/// The user stopped Deskpass during the wait: nothing is done.
#[derive(Debug)]
pub(crate) struct Stopped;

/// Waits until the user is idle (`verdict`), at most `WAIT_CAP`. `ms` is 0 when the user was idle
/// at the first look. A stop is `Stopped`, seen within `LOOK_EVERY`.
pub(crate) fn wait_for_idle() -> Result<Waited, Stopped> {
    let started = Instant::now();
    let mut first = true;
    loop {
        if esc::stopped() {
            return Err(Stopped);
        }
        let waited = started.elapsed();
        let ms = if first { 0 } else { waited.as_millis() as u64 };
        match verdict(user_input(), waited) {
            Verdict::Go => return Ok(Waited { ms, capped: false }),
            Verdict::Capped => return Ok(Waited { ms, capped: true }),
            Verdict::Wait => {}
        }
        first = false;
        std::thread::sleep(LOOK_EVERY);
    }
}

/// Test builds only (seam B, spec-background testing decisions hook 1): a test's SendInput is
/// injected and never counts, so this marks physical input instead. `typing_ms`: the user types
/// from now for that long (0: one key, now). `mouse_held_ms`: the user holds a mouse button from
/// now for that long. At least one is required.
#[cfg(debug_assertions)]
pub fn debug_user_input(params: &serde_json::Value) -> Result<serde_json::Value, (String, String)> {
    let read = |name: &str| -> Result<Option<u64>, (String, String)> {
        match params.get(name) {
            None => Ok(None),
            Some(value) => value.as_u64().map(Some).ok_or_else(|| ("invalid_params".to_string(), format!("{name} must be a whole number of ms."))),
        }
    };
    let (typing, held) = (read("typing_ms")?, read("mouse_held_ms")?);
    if typing.is_none() && held.is_none() {
        return Err(("invalid_params".to_string(), "typing_ms or mouse_held_ms is required.".to_string()));
    }
    let now = now_ms();
    if let Some(typing) = typing {
        DEBUG_TYPING_UNTIL.store(now + typing, Ordering::SeqCst);
    }
    if let Some(held) = held {
        DEBUG_HELD_UNTIL.store(now + held, Ordering::SeqCst);
    }
    Ok(serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{LLKHF_LOWER_IL_INJECTED, WM_MOUSEMOVE, WM_MOUSEWHEEL};

    const LLMHF_LOWER_IL_INJECTED: u32 = 0x2;

    fn input(since_key_ms: Option<u64>, mouse_held: bool) -> UserInput {
        UserInput { since_key: since_key_ms.map(Duration::from_millis), mouse_held }
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn a_key_less_than_300_ms_ago_waits_and_one_300_ms_ago_or_none_goes_ahead() {
        assert_eq!(verdict(input(Some(0), false), ms(0)), Verdict::Wait);
        assert_eq!(verdict(input(Some(299), false), ms(0)), Verdict::Wait);
        assert_eq!(verdict(input(Some(300), false), ms(0)), Verdict::Go);
        assert_eq!(verdict(input(Some(5_000), false), ms(0)), Verdict::Go);
        assert_eq!(verdict(input(None, false), ms(0)), Verdict::Go);
    }

    #[test]
    fn a_held_mouse_button_waits_however_long_ago_the_last_key_was() {
        assert_eq!(verdict(input(None, true), ms(0)), Verdict::Wait);
        assert_eq!(verdict(input(Some(10_000), true), ms(1_000)), Verdict::Wait);
    }

    #[test]
    fn the_wait_is_capped_at_3_s_and_an_idle_user_is_never_capped() {
        assert_eq!(verdict(input(Some(0), false), ms(2_999)), Verdict::Wait);
        assert_eq!(verdict(input(Some(0), false), ms(3_000)), Verdict::Capped);
        assert_eq!(verdict(input(None, true), ms(3_500)), Verdict::Capped);
        assert_eq!(verdict(input(None, false), ms(3_500)), Verdict::Go);
    }

    #[test]
    fn injected_keys_do_not_count_as_the_users_typing() {
        assert!(is_physical_key(0));
        assert!(!is_physical_key(LLKHF_INJECTED.0));
        assert!(!is_physical_key(LLKHF_INJECTED.0 | LLKHF_LOWER_IL_INJECTED.0));
    }

    #[test]
    fn physical_button_presses_and_releases_are_tracked_and_injected_ones_are_not() {
        assert_eq!(physical_button(WM_LBUTTONDOWN, 0, 0), Some((LEFT, true)));
        assert_eq!(physical_button(WM_LBUTTONUP, 0, 0), Some((LEFT, false)));
        assert_eq!(physical_button(WM_RBUTTONDOWN, 0, 0), Some((RIGHT, true)));
        assert_eq!(physical_button(WM_RBUTTONUP, 0, 0), Some((RIGHT, false)));
        assert_eq!(physical_button(WM_MBUTTONDOWN, 0, 0), Some((MIDDLE, true)));
        assert_eq!(physical_button(WM_XBUTTONDOWN, 1 << 16, 0), Some((X1, true)));
        assert_eq!(physical_button(WM_XBUTTONUP, 2 << 16, 0), Some((X2, false)));
        assert_eq!(physical_button(WM_LBUTTONDOWN, 0, LLMHF_INJECTED), None);
        assert_eq!(physical_button(WM_LBUTTONUP, 0, LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED), None);
        assert_eq!(physical_button(WM_MOUSEMOVE, 0, 0), None);
        assert_eq!(physical_button(WM_MOUSEWHEEL, 120 << 16, 0), None);
    }
}
