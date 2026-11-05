//! Ticket 138 (ADR 0002, spec-background "foreground restore"): foreground restore for background delivery.
//! A UI Automation action can bring its target forward (the Win32 proxy fronts a standard control
//! from the helper's own process; XAML and Chromium apps front themselves). From the action's
//! start until `WATCH_AFTER_ACTION` after it returns, every foreground change is judged by the
//! restore rule (`Watched::verdict`); one the target took is undone at once, inside the event
//! callback (or at the next look at the foreground, for an activation that lands after its event),
//! and the target is put back below the window that was above it; the result waits (at most
//! `RESTACK_GRACE`) for that asynchronous restack to land.

use std::cell::RefCell;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, GetCurrentThreadId, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, TIMER_ALL_ACCESS,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetAncestor, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindowVisible, MsgWaitForMultipleObjectsEx, PeekMessageW,
    PostThreadMessageW, SetForegroundWindow, SetWindowPos, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GWL_EXSTYLE, GW_HWNDPREV, MSG, MWMO_INPUTAVAILABLE, PM_NOREMOVE, PM_REMOVE,
    QS_ALLINPUT, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINEVENT_OUTOFCONTEXT, WM_APP, WS_EX_TOPMOST,
};

/// How long the foreground stays watched after the action returned: late activations (Calculator
/// fronts its CoreWindow, then its frame) are caught too.
pub(crate) const WATCH_AFTER_ACTION: Duration = Duration::from_millis(500);

/// A window that just took the foreground: its handle, its root window (`GetAncestor(GA_ROOT)`)
/// and its process. Handles are compared as numbers so the rule needs no window.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Activation {
    pub window: isize,
    pub root: isize,
    pub pid: u32,
}

impl Activation {
    /// Whether this is the user's window or a window inside it (a control the Win32 proxy fronts, a
    /// UWP app's CoreWindow): nothing was taken from the user when it holds the foreground. With no
    /// user window (0) only no window is the user's: a failed root lookup (0) is not inside it.
    pub(crate) fn inside(&self, user: isize) -> bool {
        self.window == user || (user != 0 && self.root == user)
    }
}

/// What one watch is about: the user's window (in front when the action began), the target window
/// and the target's process.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Watched {
    pub user: isize,
    pub target: isize,
    pub target_pid: u32,
}

/// The restore rule's answer to one foreground change.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Verdict {
    /// The target took the foreground: give it back to the user's window.
    Restore,
    /// Nothing to do, keep watching: no window, or the user's own window (a restore's own event).
    Ignore,
    /// Another app took the foreground (or the user switched): never fought, and the watch ends.
    StopWatching,
}

impl Watched {
    /// The restore rule: a window of the target's process, or one whose root is the target (a UWP
    /// app's CoreWindow inside its ApplicationFrameHost frame), is restored; the user's own window
    /// (or one inside it) is ignored; any other process ends the watch. No window in front is
    /// restored once a restore was made (`state.restored`: a restore was measured leaving no
    /// foreground window), and ignored before.
    pub(crate) fn verdict(&self, state: WatchState, activation: Activation) -> Verdict {
        if activation.window == 0 {
            return if state.restored { Verdict::Restore } else { Verdict::Ignore };
        }
        if activation.inside(self.user) {
            return Verdict::Ignore;
        }
        if self.is_target(activation) {
            return Verdict::Restore;
        }
        Verdict::StopWatching
    }

    fn is_target(&self, activation: Activation) -> bool {
        activation.pid == self.target_pid || activation.root == self.target
    }
}

/// Whether the watch is over at `now`: only once the action returned (`returned`) and
/// `WATCH_AFTER_ACTION` has passed since.
/// How long a watch goes on after its request was answered, for an action whose UI Automation call
/// was still running when the request timed out (`ForegroundWatch::finish_when`): the call cannot be
/// cancelled, so the app may still take the action, and the foreground, once it answers.
pub(crate) const LATE_ACTION_WATCH: Duration = Duration::from_secs(30);

/// Whether a late watch (`finish_when`) can stop waiting for its action at `now`: the action's call
/// returned, or `LATE_ACTION_WATCH` has passed since `since`.
pub(crate) fn late_wait_over(returned: bool, since: Instant, now: Instant) -> bool {
    returned || now >= since + LATE_ACTION_WATCH
}

pub(crate) fn watch_over(returned: Option<Instant>, now: Instant) -> bool {
    returned.is_some_and(|returned| now >= returned + WATCH_AFTER_ACTION)
}

/// How long, after the watch, the result still waits for the target to be back below its anchor:
/// the restack is asynchronous, so it lands only when the target's thread gets to it, and a thread
/// busy with the action (a list selection) was measured taking about a second.
pub(crate) const RESTACK_GRACE: Duration = Duration::from_millis(1000);

/// Whether the wait for the restack is over at `now`: once the target is back below its anchor
/// (`landed`), or `RESTACK_GRACE` after the watch ended.
pub(crate) fn restack_wait_over(returned: Instant, now: Instant, landed: bool) -> bool {
    landed || now >= returned + WATCH_AFTER_ACTION + RESTACK_GRACE
}

/// What happened during one watch.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct WatchState {
    /// A restore gave the foreground back.
    pub restored: bool,
    /// A restore was made to fail (the debug hook): the watch ends there.
    pub failed: bool,
    /// Another app took the foreground, so the watch ended there.
    pub stopped: bool,
}

/// What the action's result says about the foreground, from the watch and the window in front when
/// it ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Report {
    /// The target took the foreground and it is back with the user's window: `foreground_restored`.
    Restored,
    /// A restore failed, or the target still holds the foreground: `foreground_changed` and
    /// `restore_failed`.
    RestoreFailed,
    /// No restore to report: the result compares the foreground as before restores existed.
    Unchanged,
}

/// `last` is the window in front when the watch ended. A failed restore, the target holding the
/// foreground at the end of a watch no other app ended, or a restore that ended with no window in
/// front, is `RestoreFailed`; a restore with the user's window in front at the end is `Restored`;
/// anything else (another app took the foreground itself) `Unchanged`.
pub(crate) fn report(watched: &Watched, state: WatchState, last: Activation) -> Report {
    let target_kept = last.window != 0 && !last.inside(watched.user) && watched.is_target(last);
    let left_empty = state.restored && last.window == 0;
    if state.failed || (!state.stopped && (target_kept || left_empty)) {
        return Report::RestoreFailed;
    }
    if state.restored && last.inside(watched.user) {
        return Report::Restored;
    }
    Report::Unchanged
}

/// Test builds only: the next restore is not made (nothing is called) and the watch ends with the
/// target in front, as when a restore never takes, so a test can see `restore_failed` reported.
#[cfg(debug_assertions)]
static FAIL_NEXT_RESTORE: AtomicBool = AtomicBool::new(false);

/// Test builds only: makes the next foreground restore fail (`FAIL_NEXT_RESTORE`).
#[cfg(debug_assertions)]
pub fn debug_fail_next_restore() -> serde_json::Value {
    FAIL_NEXT_RESTORE.store(true, Ordering::SeqCst);
    serde_json::json!({})
}

/// Test builds only (ticket 150): in every later restore the plain `SetForegroundWindow` counts as
/// refused (it is not made), so a test can drive the refused path (`give_back`) every time instead
/// of the few runs a refusal happens on its own.
#[cfg(debug_assertions)]
static REFUSE_FIRST_CALL: AtomicBool = AtomicBool::new(false);

/// Test builds only: how many restores had their first call refused.
#[cfg(debug_assertions)]
static REFUSED_RESTORES: AtomicUsize = AtomicUsize::new(0);

/// Test builds only: makes the first call of every later restore count as refused
/// (`REFUSE_FIRST_CALL`); answers how many restores had it refused so far (`refused_restores`), so
/// a test can tell the path it means to show was taken.
#[cfg(debug_assertions)]
pub fn debug_refuse_first_restore_call() -> serde_json::Value {
    REFUSE_FIRST_CALL.store(true, Ordering::SeqCst);
    serde_json::json!({ "refused_restores": REFUSED_RESTORES.load(Ordering::SeqCst) })
}

/// Test builds only: how many restores had their first call refused so far (`refused_restores`),
/// without changing how later restores go.
#[cfg(debug_assertions)]
pub fn debug_refused_restores() -> serde_json::Value {
    serde_json::json!({ "refused_restores": REFUSED_RESTORES.load(Ordering::SeqCst) })
}

/// Posted to the watch's thread once the action returned, which starts `WATCH_AFTER_ACTION`.
const WM_ACTION_RETURNED: u32 = WM_APP + 1;

/// One action's watch, on a thread of its own that pumps messages, so the out-of-context
/// `EVENT_SYSTEM_FOREGROUND` hook calls back there as soon as a window takes the foreground
/// (`watch`).
pub(crate) struct ForegroundWatch {
    thread: u32,
    handle: std::thread::JoinHandle<Report>,
}

/// What the watch's thread keeps for its event callback.
struct Live {
    watched: Watched,
    /// The nearest visible window above the target when the action began (`anchor_above`).
    anchor: Option<isize>,
    state: WatchState,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
}

impl ForegroundWatch {
    /// Starts watching before the action on `target`, with `user` the window in front. `None`
    /// (nothing watched, nothing restored) without a window in front or when the hook cannot be set.
    pub(crate) unsafe fn start(target: HWND, user: HWND) -> Option<ForegroundWatch> {
        if user.0.is_null() {
            return None;
        }
        let mut target_pid = 0u32;
        GetWindowThreadProcessId(target, Some(&mut target_pid));
        let watched = Watched { user: user.0 as isize, target: target.0 as isize, target_pid };
        let anchor = anchor_above(target).map(|anchor| anchor.0 as isize);
        let (ready, started) = mpsc::channel();
        let handle = std::thread::spawn(move || watch(watched, anchor, ready));
        match started.recv() {
            Ok(Some(thread)) => Some(ForegroundWatch { thread, handle }),
            _ => {
                let _ = handle.join();
                eprintln!("foreground restore: could not watch the foreground, so a target that takes it keeps it");
                None
            }
        }
    }

    /// The action returned: waits out the rest of the watch (`WATCH_AFTER_ACTION`, or until another
    /// app took the foreground) and answers what the result reports.
    pub(crate) fn finish(self) -> Report {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_ACTION_RETURNED, WPARAM(0), LPARAM(0));
        }
        self.handle.join().unwrap_or(Report::Unchanged)
    }

    /// The request timed out while the action's call was still running, and was answered: the
    /// watch goes on, on a thread of its own, until `returned` is set (the call came back) or for
    /// `LATE_ACTION_WATCH`, and then ends as `finish` ends it, so a target that takes the foreground
    /// when it finally acts is still given back (ADR 0002). What it would report goes nowhere. It
    /// still ends early when another app takes the foreground, as every watch does.
    pub(crate) fn finish_when(self, returned: Arc<AtomicBool>) {
        let since = Instant::now();
        std::thread::spawn(move || {
            while !late_wait_over(returned.load(Ordering::SeqCst), since, Instant::now()) {
                std::thread::sleep(Duration::from_millis(10));
            }
            if matches!(self.finish(), Report::RestoreFailed) {
                eprintln!("foreground restore: a late action's target kept the foreground");
            }
        });
    }
}

/// How often the watch also looks at the foreground itself: a window another thread fronts (the
/// Win32 proxy calls `SetForegroundWindow` from the helper for the target's control) takes the
/// foreground only once that thread handles the activation, measured 3 to 112 ms after the
/// `EVENT_SYSTEM_FOREGROUND` for it, and no event comes when it does. In 100 ns units.
const LOOK_EVERY_100NS: i64 = 10_000;

/// The watch's thread: hooks foreground changes and, between them, looks at the foreground every
/// `LOOK_EVERY_100NS` on a high-resolution timer (a plain wait can take 15 ms), until the watch is
/// over (`watch_over`, once `WM_ACTION_RETURNED` came), another app took the foreground, or the
/// debug hook failed a restore; then judges the window in front (`report`).
fn watch(watched: Watched, anchor: Option<isize>, ready: mpsc::Sender<Option<u32>>) -> Report {
    unsafe {
        let mut message = MSG::default();
        // Creates the thread's message queue before `finish` can post to it.
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        LIVE.with(|live| *live.borrow_mut() = Some(Live { watched, anchor, state: WatchState::default() }));
        let timer = CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0));
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
        let timer = match timer {
            Ok(timer) if !hook.is_invalid() && SetWaitableTimer(timer, &-LOOK_EVERY_100NS, 1, None, None, false).is_ok() => timer,
            other => {
                if let Ok(timer) = other {
                    let _ = CloseHandle(timer);
                }
                if !hook.is_invalid() {
                    let _ = UnhookWinEvent(hook);
                }
                let _ = ready.send(None);
                return Report::Unchanged;
            }
        };
        let _ = ready.send(Some(GetCurrentThreadId()));
        let mut returned = None;
        loop {
            let _ = MsgWaitForMultipleObjectsEx(Some(&[timer]), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            // Delivers the hook's events too.
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_ACTION_RETURNED {
                    returned = Some(Instant::now());
                } else {
                    DispatchMessageW(&message);
                }
            }
            if ended() || watch_over(returned, Instant::now()) {
                break;
            }
            respond(activation(GetForegroundWindow()));
            if ended() {
                break;
            }
        }
        let _ = UnhookWinEvent(hook);
        let state = LIVE.with(|live| live.borrow_mut().take()).map(|live| live.state).unwrap_or_default();
        // The restack is asynchronous: the result waits (bounded) for the target to be back below
        // its anchor, so the z-order it leaves is the one the result reports.
        if let (Some(returned), Some(anchor), true) = (returned, anchor, state.restored) {
            let target = HWND(watched.target as *mut core::ffi::c_void);
            while !restack_wait_over(returned, Instant::now(), anchor_above(target).map(|above| above.0 as isize) == Some(anchor)) {
                let _ = MsgWaitForMultipleObjectsEx(Some(&[timer]), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&message);
                }
            }
        }
        let _ = CloseHandle(timer);
        report(&watched, state, activation(GetForegroundWindow()))
    }
}

/// `window` as the restore rule sees it.
unsafe fn activation(window: HWND) -> Activation {
    if window.0.is_null() {
        return Activation { window: 0, root: 0, pid: 0 };
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(window, Some(&mut pid));
    Activation { window: window.0 as isize, root: GetAncestor(window, GA_ROOT).0 as isize, pid }
}

/// Whether the window in front now is `user` or inside it (`Activation::inside`).
pub(crate) unsafe fn foreground_inside(user: HWND) -> bool {
    activation(GetForegroundWindow()).inside(user.0 as isize)
}

/// The `EVENT_SYSTEM_FOREGROUND` callback, on the watch's thread: the window that took the
/// foreground is answered at once (`respond`).
unsafe extern "system" fn on_foreground(_hook: HWINEVENTHOOK, _event: u32, hwnd: HWND, _object: i32, _child: i32, _thread: u32, _time: u32) {
    respond(activation(hwnd));
}

/// Whether another app took the foreground or a restore failed, which ends the watch.
fn ended() -> bool {
    LIVE.with(|live| live.borrow().as_ref().is_none_or(|live| live.state.stopped || live.state.failed))
}

/// The restore rule applied to `taken`, a window that took the foreground (from the hook) or holds
/// it (looked at). A window of the target is answered at once: the foreground goes back to the
/// user's window (`give_back`), then the target goes back below its anchor (`restack`). Another
/// app's window ends the watch. Whether the restore held is judged by the foreground, not by what
/// `SetForegroundWindow` answered: while the target still holds it, the next look restores again,
/// and a target holding it (or no window) when the watch ends is `RestoreFailed` (`report`). The
/// debug hook's forced failure ends the watch with the target in front.
unsafe fn respond(taken: Activation) {
    // Copied out: nothing stays borrowed while the restore runs.
    let Some((watched, anchor, state)) = LIVE.with(|live| live.borrow().as_ref().filter(|live| !live.state.stopped && !live.state.failed).map(|live| (live.watched, live.anchor, live.state))) else {
        return;
    };
    match watched.verdict(state, taken) {
        Verdict::Ignore => {}
        Verdict::StopWatching => update(|state| state.stopped = true),
        Verdict::Restore => {
            if forced_failure() {
                update(|state| state.failed = true);
                return;
            }
            give_back(&watched);
            update(|state| state.restored = true);
            // No window in front: the target is not in front either, so it was not raised.
            if let (Some(anchor), true) = (anchor, taken.window != 0) {
                restack(HWND(watched.target as *mut core::ffi::c_void), HWND(anchor as *mut core::ffi::c_void));
            }
        }
    }
}

/// Test builds only: whether the debug hook made this restore fail (`FAIL_NEXT_RESTORE`).
fn forced_failure() -> bool {
    #[cfg(debug_assertions)]
    {
        FAIL_NEXT_RESTORE.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// Test builds only: whether a restore's first call counts as refused (`REFUSE_FIRST_CALL`).
fn first_call_refused() -> bool {
    #[cfg(debug_assertions)]
    {
        REFUSE_FIRST_CALL.load(Ordering::SeqCst)
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

fn update(change: impl FnOnce(&mut WatchState)) {
    LIVE.with(|live| {
        if let Some(live) = live.borrow_mut().as_mut() {
            change(&mut live.state);
        }
    });
}

/// Gives the foreground back to the user's window: a plain `SetForegroundWindow` first; only when it
/// is refused, a zero relative mouse move (`SendInput`) and the plain call again, up to
/// `NUDGE_TRIES` times. The move makes this process the one that received the last input, which
/// Windows lets set the foreground; it does not move the cursor and is not the user's activity
/// (the idle wait ignores injected input).
/// Ticket 150, measured with the plain call forced refused while the user typed: attaching this
/// thread to the input of both the target's thread and the user's window's thread (the old
/// fallback) made the user's thread share the busy target's input queue for 100-260 ms, and the keys
/// typed meanwhile went to the target's control or arrived as system keys with no focus (lost in
/// every forced run); attaching only to the target's thread, or only to the user's, stayed refused
/// hundreds of times while the target kept the foreground (`restore_failed`); plain calls alone
/// stayed refused too (keys lost to the target in 6 of 10 runs). The move was never refused (162
/// forced restores). The key the user types while the target holds the foreground (which is also
/// what makes the plain call refused) still goes to the target: only keeping the target from taking
/// the foreground avoids that (ticket 155).
/// A call can leave no window in front; with none, the plain call is allowed, and a later look that
/// still finds none restores again (`Watched::verdict`).
/// What `SetForegroundWindow` answers is not otherwise used: while another thread's activation is
/// landing it answered false for a switch that then happened, and true for one the landing
/// activation undid; the foreground itself is looked at again instead (`watch`).
unsafe fn give_back(watched: &Watched) {
    let user = HWND(watched.user as *mut core::ffi::c_void);
    if !first_call_refused() && SetForegroundWindow(user).as_bool() {
        return;
    }
    #[cfg(debug_assertions)]
    REFUSED_RESTORES.fetch_add(1, Ordering::SeqCst);
    let nudge = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } } };
    for _ in 0..NUDGE_TRIES {
        let _ = SendInput(&[nudge], std::mem::size_of::<INPUT>() as i32);
        if SetForegroundWindow(user).as_bool() || foreground_inside(user) {
            break;
        }
        std::thread::sleep(NUDGE_EVERY);
    }
    if GetForegroundWindow().0.is_null() {
        let _ = SetForegroundWindow(user);
    }
}

/// How many times a refused restore moves the mouse by zero and calls again, and how long it waits
/// between tries (`give_back`).
const NUDGE_TRIES: usize = 3;
const NUDGE_EVERY: Duration = Duration::from_millis(5);

/// Puts `target` back right below `anchor`, without activating it. Asynchronous: the target's
/// thread is busy with the action, and waiting for it (measured about 216 ms) would hold up the
/// next restore.
unsafe fn restack(target: HWND, anchor: HWND) {
    let _ = SetWindowPos(target, Some(anchor), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
}

/// The nearest visible top-level window above `target` in the z-order, below which it is put back
/// after a restore. Topmost windows do not count (placing the target below one would make it
/// topmost), so a target below only topmost windows has none and its place is left alone.
unsafe fn anchor_above(target: HWND) -> Option<HWND> {
    let mut current = target;
    for _ in 0..4096 {
        let above = GetWindow(current, GW_HWNDPREV).ok()?;
        if above.0.is_null() || GetWindowLongPtrW(above, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 {
            return None;
        }
        if IsWindowVisible(above).as_bool() {
            return Some(above);
        }
        current = above;
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_late_watch_waits_for_its_action_at_most_late_action_watch() {
        let since = Instant::now();
        assert!(!super::late_wait_over(false, since, since));
        assert!(!super::late_wait_over(false, since, since + super::LATE_ACTION_WATCH - Duration::from_millis(1)));
        assert!(super::late_wait_over(false, since, since + super::LATE_ACTION_WATCH));
        assert!(super::late_wait_over(true, since, since), "a call that returned ends the wait at once");
    }

    use super::*;

    const USER: isize = 0x100;
    const TARGET: isize = 0x200;
    const TARGET_PID: u32 = 42;
    const WATCHED: Watched = Watched { user: USER, target: TARGET, target_pid: TARGET_PID };

    const NOT_RESTORED: WatchState = WatchState { restored: false, failed: false, stopped: false };
    const RESTORED: WatchState = WatchState { restored: true, failed: false, stopped: false };

    fn activation(window: isize, root: isize, pid: u32) -> Activation {
        Activation { window, root, pid }
    }

    #[test]
    fn a_window_of_the_target_process_taking_the_foreground_is_restored() {
        // The target itself, a child control of it (the Win32 proxy fronts the Button), and another
        // window of its process (a dialog the action opened).
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(TARGET, TARGET, TARGET_PID)), Verdict::Restore);
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(0x201, TARGET, TARGET_PID)), Verdict::Restore);
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(0x300, 0x300, TARGET_PID)), Verdict::Restore);
    }

    #[test]
    fn a_window_of_another_process_whose_root_is_the_target_is_restored() {
        // A UWP app's CoreWindow inside its ApplicationFrameHost frame, the target.
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(0x210, TARGET, 7)), Verdict::Restore);
    }

    #[test]
    fn a_window_of_another_process_stops_the_watch_without_a_restore() {
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(0x400, 0x400, 7)), Verdict::StopWatching);
    }

    #[test]
    fn the_users_own_window_and_no_window_are_ignored_and_the_watch_goes_on() {
        // The user's window comes back with a restore's own event; Calculator fronts a second
        // window after that, which must still be restored.
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(USER, USER, 7)), Verdict::Ignore);
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(0x101, USER, 7)), Verdict::Ignore);
        assert_eq!(WATCHED.verdict(NOT_RESTORED, activation(0, 0, 0)), Verdict::Ignore);
    }

    #[test]
    fn no_window_in_front_after_a_restore_is_given_back_to_the_user_again() {
        // Detaching the user's thread after a restore was seen to leave no foreground window, which
        // takes the next SetForegroundWindow; before any restore no window is someone else's switch.
        assert_eq!(WATCHED.verdict(RESTORED, activation(0, 0, 0)), Verdict::Restore);
        assert_eq!(WATCHED.verdict(RESTORED, activation(USER, USER, 7)), Verdict::Ignore);
        assert_eq!(WATCHED.verdict(RESTORED, activation(0x400, 0x400, 7)), Verdict::StopWatching);
    }

    #[test]
    fn the_user_window_of_the_target_process_is_never_restored_away() {
        let same_app = Watched { user: USER, target: TARGET, target_pid: 7 };
        assert_eq!(same_app.verdict(NOT_RESTORED, activation(USER, USER, 7)), Verdict::Ignore);
    }

    #[test]
    fn the_watch_lasts_until_500_ms_after_the_action_returned() {
        let start = Instant::now();
        assert!(!watch_over(None, start + Duration::from_secs(60)), "the watch ended before the action returned");
        let returned = start + Duration::from_millis(100);
        assert!(!watch_over(Some(returned), returned));
        assert!(!watch_over(Some(returned), returned + Duration::from_millis(499)));
        assert!(watch_over(Some(returned), returned + Duration::from_millis(500)));
        assert!(watch_over(Some(returned), returned + Duration::from_millis(900)));
    }

    #[test]
    fn after_the_watch_the_result_waits_up_to_1_s_for_the_target_to_be_back_below_its_anchor() {
        let returned = Instant::now();
        let over = returned + WATCH_AFTER_ACTION;
        assert!(restack_wait_over(returned, over, true), "a target already back below its anchor was waited for");
        assert!(!restack_wait_over(returned, over, false));
        assert!(!restack_wait_over(returned, over + Duration::from_millis(999), false));
        assert!(restack_wait_over(returned, over + Duration::from_millis(1000), false));
    }

    #[test]
    fn a_restore_that_left_the_user_window_in_front_is_reported_restored() {
        let state = WatchState { restored: true, ..WatchState::default() };
        assert_eq!(report(&WATCHED, state, activation(USER, USER, 7)), Report::Restored);
    }

    #[test]
    fn a_window_counts_as_the_users_when_it_is_that_window_or_inside_it() {
        assert!(activation(USER, USER, 7).inside(USER));
        assert!(activation(0x101, USER, 7).inside(USER));
        assert!(!activation(0x400, 0x400, 7).inside(USER));
        assert!(!activation(0, 0, 0).inside(USER));
    }

    #[test]
    fn with_no_user_window_a_window_whose_root_is_unknown_is_not_the_users() {
        // Ticket 153: nothing was in front when the action began (user 0). A window whose GA_ROOT
        // lookup failed (root 0) matches it only because both are zero: it is not the user's.
        // Still nothing in front is as it was.
        assert!(!activation(0x400, 0, 7).inside(0));
        assert!(activation(0, 0, 0).inside(0));
    }

    #[test]
    fn the_user_window_being_the_target_is_never_reported_restore_failed_for_a_control_inside_it() {
        // The user launched the app and watches the AI: the target is the user's window, and the
        // control a proxy or a UWP app fronts inside it (a child, root = the user's window) is no
        // loss of the user's window. Nothing was restored, so nothing is reported.
        let own = Watched { user: USER, target: USER, target_pid: 7 };
        for last in [activation(USER, USER, 7), activation(0x101, USER, 7)] {
            assert_eq!(report(&own, WatchState::default(), last), Report::Unchanged, "{last:?}");
        }
        // A restore made, the child still in front: the user's window is in front, as `Restored` says.
        assert_eq!(report(&own, RESTORED, activation(0x101, USER, 7)), Report::Restored);
        // A different window of the same app taking the foreground is still the target keeping it.
        assert_eq!(report(&own, WatchState::default(), activation(0x300, 0x300, 7)), Report::RestoreFailed);
    }

    #[test]
    fn a_failed_restore_or_a_target_still_in_front_is_reported_restore_failed() {
        let failed = WatchState { failed: true, ..WatchState::default() };
        assert_eq!(report(&WATCHED, failed, activation(TARGET, TARGET, TARGET_PID)), Report::RestoreFailed);
        assert_eq!(report(&WATCHED, failed, activation(USER, USER, 7)), Report::RestoreFailed);
        let restored = WatchState { restored: true, ..WatchState::default() };
        assert_eq!(report(&WATCHED, restored, activation(0x201, TARGET, TARGET_PID)), Report::RestoreFailed);
        assert_eq!(report(&WATCHED, WatchState::default(), activation(TARGET, TARGET, TARGET_PID)), Report::RestoreFailed);
    }

    #[test]
    fn a_restore_that_ended_with_no_window_in_front_is_reported_restore_failed() {
        assert_eq!(report(&WATCHED, RESTORED, activation(0, 0, 0)), Report::RestoreFailed);
        // Another app in front at the end took it itself: not a failed restore.
        assert_eq!(report(&WATCHED, RESTORED, activation(0x400, 0x400, 9)), Report::Unchanged);
        // No restore made: no window in front is nothing the watch did.
        assert_eq!(report(&WATCHED, NOT_RESTORED, activation(0, 0, 0)), Report::Unchanged);
    }

    #[test]
    fn no_restore_or_another_app_in_front_is_reported_unchanged() {
        assert_eq!(report(&WATCHED, WatchState::default(), activation(USER, USER, 7)), Report::Unchanged);
        let stopped = WatchState { restored: true, stopped: true, ..WatchState::default() };
        assert_eq!(report(&WATCHED, stopped, activation(0x400, 0x400, 9)), Report::Unchanged);
        let gone_elsewhere = WatchState { stopped: true, ..WatchState::default() };
        assert_eq!(report(&WATCHED, gone_elsewhere, activation(0x400, 0x400, 9)), Report::Unchanged);
    }
}
