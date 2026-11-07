//! Ticket 44: Windows refuses `AttachThreadInput` to a UWP app's CoreWindow thread (and to its
//! ApplicationFrameHost.exe frame's), so the keys for such an app are sent without following its
//! input queue: each once the CoreWindow answered two sent messages in a row after the one before,
//! with every other check before each key kept, and the result says the reads were not confirmed.
//!
//! The release helper takes that path only for a frame that `identify` resolves through the
//! system's ApplicationFrameHost.exe path, which no test can redirect. So this test compiles
//! `src/input.rs` (with the modules it uses) into its own binary, the way `frame_host.rs` compiles
//! `src/windows_list.rs`, and calls the path directly with a frame and a keyboard window of two
//! different processes: a fixture window as the frame, and a second fixture's `CoreWindow` put into
//! it (`--core-window-child`), which reports what it read in its title. Nothing here is reachable in
//! the release helper. `input.rs`'s own unit tests run in this binary as well.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::json;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyboardLayout, GetKeyboardLayoutList, SetFocus, HKL, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetForegroundWindow, GetGUIThreadInfo, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SendMessageTimeoutW, GUITHREADINFO, SMTO_BLOCK, WM_IME_CONTROL,
    WM_NULL,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[allow(dead_code, unused_imports)]
#[path = "../src/input.rs"]
mod input;
#[allow(dead_code)]
#[path = "../src/popups.rs"]
mod popups;
#[allow(dead_code)]
#[path = "../src/protocol.rs"]
mod protocol;
#[allow(dead_code)]
#[path = "../src/windows_list.rs"]
mod windows_list;

/// The stop state `input.rs` checks (`crate::esc::stopped`), which Esc and `debug_stop` set in the
/// helper: set here only by `stop_after` (ticket 49), under the interactive lock. The real `esc.rs`
/// installs a keyboard hook and its unit tests set and clear the process-wide stop.
mod esc {
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static STOP: AtomicBool = AtomicBool::new(false);

    pub fn stopped() -> bool {
        STOP.load(Ordering::SeqCst)
    }

    /// No request is in flight here, so nothing is marked as acting (ticket 73).
    pub fn current_request() -> Option<u64> {
        None
    }

    /// `esc::act` without the response lock: refuses when stopped, otherwise sends.
    pub fn act<T, E>(_: Option<u64>, send: impl FnOnce() -> Result<T, E>) -> Option<Result<T, E>> {
        if stopped() { None } else { Some(send()) }
    }
}

/// The element lookup `input.rs` calls for a `click` on an element (`crate::uia::element_point`,
/// ticket 62); these tests click no elements.
mod uia {
    pub fn element_point(_: windows::Win32::Foundation::HWND, _: &serde_json::Value) -> Result<(i32, i32), crate::protocol::Failure> {
        unreachable!("these tests click no elements")
    }

    /// The focused value `paste_text` reads (ticket 77); these tests paste nothing.
    pub fn focused_value(_: windows::Win32::Foundation::HWND, _: std::time::Instant) -> Option<String> {
        unreachable!("these tests paste nothing")
    }

    pub fn focused_value_shows(_: windows::Win32::Foundation::HWND, _: String, _: &str, _: std::time::Duration) -> bool {
        unreachable!("these tests paste nothing")
    }

    /// The password field check (ticket 92): these fixtures have no password field, so it says
    /// what a check UI Automation cannot answer says, and input goes ahead.
    pub const PASSWORD_CHECK_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

    pub fn focus_is_password(_: std::time::Instant) -> bool {
        false
    }

    pub fn password_field(_: &str) -> crate::protocol::Failure {
        unreachable!("these fixtures have no password field")
    }

    /// The background element click (ticket 140); these tests click no elements.
    pub fn click_element(_: &serde_json::Value) -> Result<serde_json::Value, crate::protocol::Failure> {
        unreachable!("these tests click no elements")
    }

    /// The background click at a point (ticket 141); these tests click no points in the background.
    pub fn click_point(_: &serde_json::Value, _: i32, _: i32) -> Result<serde_json::Value, crate::protocol::Failure> {
        unreachable!("these tests click no points in the background")
    }

    /// The background scroll (reference-gaps G5); these tests scroll nothing in the background.
    pub fn scroll_point(_: &serde_json::Value, _: i32, _: i32, _: &str, _: u64) -> Result<serde_json::Value, crate::protocol::Failure> {
        unreachable!("these tests scroll nothing in the background")
    }
}

/// The foreground restore report `input.rs` turns into a result (ticket 138); these tests take no
/// background action.
#[allow(dead_code)]
mod restore {
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub enum Report {
        Restored,
        RestoreFailed,
        Unchanged,
    }

    pub unsafe fn foreground_inside(_user: windows::Win32::Foundation::HWND) -> bool {
        unreachable!("these tests take no background action")
    }

    /// `foreground` ends a background action's late watch; these tests take none, so there is none.
    pub unsafe fn end_late_watches(_target: windows::Win32::Foundation::HWND) {}
}

#[allow(dead_code)]
#[path = "../src/clipboard.rs"]
mod clipboard;

const CORE_WINDOW_CLASS: windows::core::PCWSTR = w!("Windows.UI.Core.CoreWindow");

/// Text and a chord reach the app's CoreWindow, in order, while the frame holds the foreground,
/// and succeed marked `read_confirmed: false`. Non-ASCII text still switches the app's thread to
/// the English layout and back, and the chord closes and reopens its input method. ASCII text is
/// digits and symbols only: with the zh-TW input method open on the fixture's thread, letters
/// typed into its `Windows.UI.Core.CoreWindow` are composed by the input method (ticket 44).
#[test]
fn keys_for_an_app_whose_input_cannot_be_followed_reach_it_and_are_marked_unconfirmed() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&[]);
    app.activate();
    let layout = layout_of(app.core);
    let layouts = layout_list();
    let ime_open = ime_is_open(app.core);

    let typed = unsafe { input::type_unconfirmed(app.frame, app.core, "7*8\n你好", Instant::now()) }.expect("type_text");
    assert_eq!(typed, json!({ "read_confirmed": false }));
    app.wait_for_title(|title| part(title, "text=") == "7*8⏎你好");
    assert_eq!(layout_of(app.core), layout, "the app's keyboard layout was not restored");
    assert_eq!(layout_list(), layouts, "the keyboard layout list changed");

    let keys = [VIRTUAL_KEY(VK_CONTROL.0), VIRTUAL_KEY(u16::from(b'A'))];
    let pressed = unsafe { input::press_unconfirmed(app.frame, app.core, &keys, Instant::now()) }.expect("press_key ctrl+a");
    assert_eq!(pressed, json!({ "read_confirmed": false }));
    let pressed = unsafe { input::press_unconfirmed(app.frame, app.core, &[VIRTUAL_KEY(0x0D)], Instant::now()) }.expect("press_key Return");
    assert_eq!(pressed, json!({ "read_confirmed": false }));
    app.wait_for_title(|title| part(title, "keys=").ends_with("d17,d65,u65,u17,d13,u13"));
    assert!(part(&app.title(), "text=").ends_with('⏎'), "{}", app.title());
    assert_eq!(ime_is_open(app.core), ime_open, "the app's input method was not restored");
    assert_released();
}

/// An app that does not take the English layout within its 500 ms is typed into on its own layout
/// rather than failed, and the result says so with `layout_switched: false`: an input method open
/// there may compose or hold the characters. The app's layout and the layout list are unchanged.
/// What it read is not checked: with the zh-TW input method open, the fixture's CoreWindow hands
/// even digits to the input method.
#[test]
fn text_for_an_app_that_keeps_its_own_layout_is_typed_and_says_the_layout_was_not_switched() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&["--core-window-keep-layout"]);
    app.activate();
    let layout = layout_of(app.core);
    assert_ne!(layout.0 as usize & 0xffff, 0x0409, "the fixture's thread is already on an English layout: {:#x}", layout.0 as usize);
    let layouts = layout_list();

    let typed = unsafe { input::type_unconfirmed(app.frame, app.core, "12", Instant::now()) }.expect("type_text");
    assert_eq!(typed, json!({ "read_confirmed": false, "layout_switched": false }));
    assert_eq!(layout_of(app.core), layout, "the app's keyboard layout changed");
    assert_eq!(layout_list(), layouts, "the keyboard layout list changed");
    app.settle();
    assert_released();
}

/// An app that stops answering after a character ends typing at the bounded wait with
/// `input_failed` "did not respond", saying what was sent; nothing more is sent.
#[test]
fn an_app_that_stops_answering_ends_typing_saying_what_was_sent() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&["--core-window-hang", "7000"]);
    app.activate();
    let started = Instant::now();
    let failure = unsafe { input::type_unconfirmed(app.frame, app.core, "123", started) }.expect_err("an app that does not answer");
    let elapsed = started.elapsed();
    assert_eq!(failure.code, "input_failed", "{}", failure.message);
    // The pacing can run one key ahead of the app (`wait_for_answers`): it may answer between
    // reading the first key and handling its character, where it hangs.
    let sent = [1, 2]
        .into_iter()
        .find(|sent| {
            failure.message == format!(
                "The app in the target window did not respond within 5 seconds after character {sent} of 3 was sent; {sent} of 3 characters were sent, without confirmation that the app read them, and the remaining {} were not sent.",
                3 - sent
            )
        })
        .unwrap_or_else(|| panic!("{}", failure.message));
    assert!(elapsed >= Duration::from_secs(5) && elapsed < Duration::from_secs(7), "{elapsed:?}");
    // Once it reads again, it has read everything that was sent, and nothing more was.
    app.answers_within(Duration::from_secs(10));
    app.settle();
    assert_eq!(part(&app.title(), "text="), "123"[..sent], "{}", app.title());
    assert_released();
}

/// Ticket 43's rule on this path: the app taking the foreground away (the fixture opens a window
/// of its own over a `9`) after a character that is not the last stops typing, and the failure
/// says exactly how much was sent; after the last one it is success, naming the new foreground.
#[test]
fn losing_the_foreground_stops_typing_before_the_last_character_and_not_after_it() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&[]);
    app.activate();
    let text = "12934567";
    let failure = unsafe { input::type_unconfirmed(app.frame, app.core, text, Instant::now()) }.expect_err("the frame lost the foreground");
    assert_eq!(failure.code, "input_failed");
    let sent = (3..text.len())
        .find(|sent| failure.message == format!("The target window lost the foreground after character {sent} of 8; the remaining {} were not sent.", 8 - sent))
        .unwrap_or_else(|| panic!("{}", failure.message));
    app.settle();
    assert_eq!(part(&app.title(), "text="), "129", "{}", app.title());
    // The pacing is not confirmation: a key sent while the frame was still in front, before the app
    // handled the `9`, reaches the window it opened. Exactly the reported characters were sent.
    let steal = app.steal_window();
    assert_eq!(part(&window_title(steal), "text="), text[3..sent], "reported {sent} sent: {}", failure.message);
    eprintln!("ran ahead of the app by {} key(s)", sent - 3);

    app.activate();
    // Ticket 50 (F38): right after the foreground is taken back from the app's own window, a key
    // can reach the app only after it answered both the pacing and the wait before the result
    // (measured: 4 answers, then the `9`, about 5 ms late), so the window the `9` opens was missed
    // in about 1 of 16 runs. The app gets that time first (0 of 150 since): this checks the
    // foreground rule, not the pacing, which is not confirmation.
    app.settle();
    let typed = unsafe { input::type_unconfirmed(app.frame, app.core, "9", Instant::now()) }.expect("the last character was sent in front");
    assert_eq!(typed["read_confirmed"], json!(false), "{typed}");
    assert_eq!(typed["foreground_changed"]["target_closed"], json!(false), "{typed}");
    assert_eq!(typed["foreground_changed"]["foreground"]["window_id"], json!((steal.0 as isize).to_string()), "{typed}");
    app.wait_for_title(|title| part(title, "text=") == "1299");
    assert_released();
}

const VK_F5: VIRTUAL_KEY = VIRTUAL_KEY(0x74);
const VK_F6: VIRTUAL_KEY = VIRTUAL_KEY(0x75);

/// Ticket 49 (F12): an app that stops answering after a key of a chord that is not the last ends
/// the key press at the bounded wait, saying how many keys were pressed; the pressed ones are
/// released (in reverse order) and the app gets until the drain limit to take them; nothing more
/// is pressed.
#[test]
fn an_app_that_stops_answering_partway_through_a_key_press_gets_the_pressed_keys_released() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&["--core-window-hang-keydown", "7000"]);
    app.activate();
    let started = Instant::now();
    let failure = unsafe { input::press_unconfirmed(app.frame, app.core, &[VIRTUAL_KEY(VK_SHIFT.0), VK_F5, VK_F6], started) }.expect_err("an app that does not answer");
    let elapsed = started.elapsed();
    assert_eq!(failure.code, "input_failed", "{}", failure.message);
    // As for text, the pacing can run one key ahead of the app (`wait_for_answers`). The wait's
    // own 5 seconds and the call's 5-second limit end together here.
    let pressed = [1, 2]
        .into_iter()
        .find(|pressed| {
            ["within 5 seconds", "within the 5-second limit for one call"].iter().any(|limit| {
                failure.message == format!(
                    "The app in the target window did not respond {limit} after key {pressed} of 3 was pressed; the keys pressed so far were released, and the remaining {} were not sent.",
                    3 - pressed
                )
            })
        })
        .unwrap_or_else(|| panic!("{}", failure.message));
    // The drain after the releases ends at `PRESS_KEY_DRAIN_LIMIT` (7 s from the request), when
    // the app answers again, then the input method is reopened.
    assert!(elapsed >= Duration::from_secs(5) && elapsed < Duration::from_millis(8500), "{elapsed:?}");
    app.answers_within(Duration::from_secs(10));
    app.settle();
    let expected = if pressed == 1 { "d16,u16" } else { "d16,d116,u116,u16" };
    assert_eq!(part(&app.title(), "keys="), expected, "reported {pressed} pressed: {}", app.title());
    assert_released();
}

/// Ticket 49 (F12): the app taking the foreground away (a `9` opens a window of its own) during a
/// key press that is not done presses no more keys: `input_failed`, the keys pressed released.
#[test]
fn losing_the_foreground_partway_through_a_key_press_presses_no_more_keys() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&[]);
    app.activate();
    let keys = [VIRTUAL_KEY(u16::from(b'9')), VIRTUAL_KEY(u16::from(b'1')), VIRTUAL_KEY(u16::from(b'2'))];
    let failure = unsafe { input::press_unconfirmed(app.frame, app.core, &keys, Instant::now()) }.expect_err("the frame lost the foreground");
    assert_eq!(failure.code, "input_failed");
    assert_eq!(
        failure.message,
        "The target window lost the foreground during the key press; the remaining keys were not sent, and the keys pressed so far were released."
    );
    app.settle();
    let steal = app.steal_window();
    // The pacing may let the `1` through before the app handled the `9` (ticket 44); never the `2`.
    let (title, stolen) = (app.title(), window_title(steal));
    eprintln!("after the hand-off: {title}; {stolen}");
    assert!(part(&title, "keys=").starts_with("d57"), "{title}");
    assert!(!part(&title, "keys=").contains("d50") && !part(&stolen, "text=").contains('2'), "a key was pressed after the hand-off: {title}; {stolen}");
    assert_released();
}

/// Ticket 49 (F12): an app that stops answering after the last release fails the key press saying
/// every key was pressed and released, without confirmation.
#[test]
fn an_app_that_stops_answering_after_the_last_release_fails_saying_every_key_was_sent() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&["--core-window-hang-keyup", "7000"]);
    app.activate();
    // Ticket 54: with the app's input method still open, the call closes it (`ImeGuard`) just
    // before the F5, and in some runs the app answered sent messages about 130 ms before it read
    // the F5, then answered twice more with the release already queued (sent messages come ahead
    // of queued input) and only then read it and hung: the pacing, which is not confirmation,
    // counted the release as read and the call succeeded (4 of 20 runs; 2 of 20 with the code
    // before ticket 52). This test is about the hang after the release, so the input method is
    // closed and the app settled first (then 0 of 20).
    close_ime(app.core);
    app.settle();
    assert!(!ime_is_open(app.core), "the CoreWindow's input method is still open");
    let started = Instant::now();
    // When the app read the press and the release (its title shows `d116`, `u116`), so a result
    // that does not fit says which came first (ticket 54).
    let reads = ReadTimes::watch(app.core, started, &["d116", "u116"]);
    let pressed = unsafe { input::press_unconfirmed(app.frame, app.core, &[VK_F5], started) };
    let elapsed = started.elapsed();
    let title = app.title();
    let read_at = reads.stop();
    eprintln!("press_unconfirmed [F5] returned after {elapsed:?}; the app read {read_at:?} (from the request); its title then: {title}");
    let failure = pressed.err().unwrap_or_else(|| panic!("an app that does not answer after the release: the key press succeeded after {elapsed:?}; the app read {read_at:?}; its title then: {title}"));
    assert_eq!(failure.code, "input_failed", "{}", failure.message);
    // Its own 5 seconds and the call's 5-second limit end together here.
    assert!(
        ["within 5 seconds", "within the 5-second limit for one call"].iter().any(|limit| failure.message
            == format!("The app in the target window did not respond {limit} after the key press; every key was pressed and released, without confirmation that the app read them.")),
        "{}",
        failure.message
    );
    assert!(elapsed >= Duration::from_secs(5) && elapsed < Duration::from_millis(6500), "{elapsed:?}");
    app.answers_within(Duration::from_secs(10));
    app.settle();
    assert_eq!(part(&app.title(), "keys="), "d116,u116", "{}", app.title());
    assert_released();
}

/// Ticket 49 (F12): a stop (Esc, `debug_stop`) while typing waits for an app that stopped
/// answering ends typing at once as `interrupted`, sends nothing more, and still restores the
/// app's keyboard layout (once the app takes the request).
#[test]
fn a_stop_while_typing_ends_it_as_interrupted_without_waiting_out_the_app() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&["--core-window-hang", "3000"]);
    app.activate();
    let layout = layout_of(app.core);
    let started = Instant::now();
    let stop = stop_after(Duration::from_millis(500));
    let failure = unsafe { input::type_unconfirmed(app.frame, app.core, "123", started) }.expect_err("a stop");
    let elapsed = started.elapsed();
    drop(stop);
    assert_eq!(failure.code, "interrupted", "{}", failure.message);
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
    app.answers_within(Duration::from_secs(10));
    app.settle();
    let text = part(&app.title(), "text=");
    assert!(text == "1" || text == "12", "{}", app.title());
    wait_for(Duration::from_secs(5), "the app's keyboard layout to be restored", || (layout_of(app.core) == layout).then_some(()));
    assert_released();
}

/// Ticket 49 (F12): a stop while a key press waits for an app that stopped answering ends it at
/// once as `interrupted` and releases the pressed keys, with no drain wait.
#[test]
fn a_stop_during_a_key_press_releases_the_keys_without_waiting_for_the_app() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&["--core-window-hang-keydown", "3000"]);
    app.activate();
    let started = Instant::now();
    let stop = stop_after(Duration::from_millis(500));
    let failure = unsafe { input::press_unconfirmed(app.frame, app.core, &[VIRTUAL_KEY(VK_SHIFT.0), VK_F5], started) }.expect_err("a stop");
    let elapsed = started.elapsed();
    drop(stop);
    assert_eq!(failure.code, "interrupted", "{}", failure.message);
    // The wait ends at the stop; reopening the input method of the busy app gives up after 500 ms.
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
    assert_released();
    app.answers_within(Duration::from_secs(10));
    app.settle();
    let keys = part(&app.title(), "keys=");
    assert!(keys == "d16,u16" || keys == "d16,d116,u116,u16", "{}", app.title());
}

/// Ticket 49 (F12): a key press after whose last key the app opens a window of its own succeeds
/// with both `read_confirmed: false` and `foreground_changed` naming that window.
#[test]
fn a_key_press_after_which_the_app_opens_a_window_succeeds_unconfirmed_naming_it() {
    let Some(_interactive) = interactive() else { return };
    let app = App::open(&[]);
    app.activate();
    let pressed = unsafe { input::press_unconfirmed(app.frame, app.core, &[VIRTUAL_KEY(u16::from(b'9'))], Instant::now()) }.expect("press_key 9");
    let steal = app.steal_window();
    assert_eq!(pressed["read_confirmed"], json!(false), "{pressed}");
    assert_eq!(pressed["foreground_changed"]["target_closed"], json!(false), "{pressed}");
    assert_eq!(pressed["foreground_changed"]["foreground"]["window_id"], json!((steal.0 as isize).to_string()), "{pressed}");
    assert_eq!(part(&app.title(), "text="), "9", "{}", app.title());
    assert_released();
}

/// Sets the stop `input.rs` checks after `delay`; cleared again when dropped.
fn stop_after(delay: Duration) -> StopAfter {
    let thread = std::thread::spawn(move || {
        std::thread::sleep(delay);
        esc::STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    StopAfter(Some(thread))
}

struct StopAfter(Option<std::thread::JoinHandle<()>>);

impl Drop for StopAfter {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            let _ = thread.join();
        }
        esc::STOP.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Watches a CoreWindow fixture's title every millisecond, from another thread, and records how
/// long after `started` each of `events` first showed in its `keys=` part.
struct ReadTimes {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<Vec<(String, Option<Duration>)>>,
}

impl ReadTimes {
    fn watch(core: HWND, started: Instant, events: &[&str]) -> Self {
        let core = core.0 as isize;
        let events: Vec<String> = events.iter().map(|event| event.to_string()).collect();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watching = done.clone();
        let thread = std::thread::spawn(move || {
            let mut seen: Vec<(String, Option<Duration>)> = events.into_iter().map(|event| (event, None)).collect();
            while !watching.load(std::sync::atomic::Ordering::SeqCst) {
                let keys = part(&window_title(HWND(core as *mut core::ffi::c_void)), "keys=");
                for (event, at) in &mut seen {
                    if at.is_none() && keys.split(',').any(|read| read == event) {
                        *at = Some(started.elapsed());
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            seen
        });
        Self { done, thread }
    }

    fn stop(self) -> Vec<(String, Option<Duration>)> {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread.join().expect("the title watcher")
    }
}

/// A fixture window as the frame, and a CoreWindow of a second fixture process inside it.
struct App {
    frame: HWND,
    core: HWND,
    core_process: KillOnDrop,
    _frame_process: KillOnDrop,
}

impl App {
    fn open(core_args: &[&str]) -> Self {
        let frame_process = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("frame fixture"));
        let frame = wait_for(Duration::from_secs(5), "the frame fixture's window", || {
            find_window(w!("CUMCP_FIXTURE"), frame_process.0.id()).filter(|window| unsafe { IsWindowVisible(*window) }.as_bool())
        });
        let core_process = KillOnDrop(
            Command::new(env!("CARGO_BIN_EXE_fixture-window"))
                .arg("--core-window-child")
                .arg((frame.0 as isize).to_string())
                .args(core_args)
                .spawn()
                .expect("core window fixture"),
        );
        let core = wait_for(Duration::from_secs(5), "the CoreWindow in the frame", || unsafe { FindWindowExW(Some(frame), None, CORE_WINDOW_CLASS, None) }.ok());
        Self { frame, core, core_process, _frame_process: frame_process }
    }

    /// Brings the frame to the foreground the way `activate_window` does, then gives the
    /// CoreWindow the keyboard focus, as ApplicationFrameHost.exe does for the app inside it.
    fn activate(&self) {
        let activated = input::activate_window(&json!({ "window_id": (self.frame.0 as isize).to_string() }));
        assert!(activated.is_ok(), "activate_window: {:?}", activated.err());
        assert_eq!(unsafe { GetForegroundWindow() }, self.frame, "the frame is not in front");
        unsafe {
            let current = GetCurrentThreadId();
            let thread = GetWindowThreadProcessId(self.core, None);
            assert!(AttachThreadInput(current, thread, true).as_bool(), "AttachThreadInput to the fixture");
            let _ = SetFocus(Some(self.core));
            let _ = AttachThreadInput(current, thread, false);
            wait_for(Duration::from_secs(2), "the CoreWindow's keyboard focus", || {
                let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
                (GetGUIThreadInfo(thread, &mut gui).is_ok() && gui.hwndFocus == self.core).then_some(())
            });
        }
    }

    /// Waits until the CoreWindow's title satisfies `ready`.
    fn wait_for_title(&self, ready: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready(&self.title()) {
            assert!(Instant::now() < deadline, "timed out; the CoreWindow reports {}", self.title());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits for the CoreWindow's thread to answer a sent message.
    fn answers_within(&self, limit: Duration) {
        let mut answer = 0usize;
        let sent = unsafe { SendMessageTimeoutW(self.core, WM_NULL, WPARAM(0), LPARAM(0), SMTO_BLOCK, limit.as_millis() as u32, Some(&mut answer)) };
        assert_ne!(sent.0, 0, "the CoreWindow fixture does not answer");
    }

    /// Gives the CoreWindow time to read anything still queued for it, so a check after this sees
    /// whether more was sent than reported.
    fn settle(&self) {
        std::thread::sleep(Duration::from_millis(500));
        self.answers_within(Duration::from_secs(2));
    }

    fn title(&self) -> String {
        window_title(self.core)
    }

    /// The window the CoreWindow fixture opened over a `9`.
    fn steal_window(&self) -> HWND {
        find_window(w!("CUMCP_STEAL"), self.core_process.0.id()).expect("the fixture's CUMCP_STEAL window")
    }
}

fn window_title(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

/// One part of the CoreWindow fixture's title (`text=`, `keys=`).
fn part(title: &str, name: &str) -> String {
    title.split(" | ").find_map(|part| part.strip_prefix(name)).unwrap_or("").to_string()
}

fn find_window(class: windows::core::PCWSTR, pid: u32) -> Option<HWND> {
    let mut after = None;
    loop {
        let window = unsafe { FindWindowExW(None, after, class, None) }.ok()?;
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };
        if owner == pid {
            return Some(window);
        }
        after = Some(window);
    }
}

fn wait_for<T>(limit: Duration, what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn layout_of(window: HWND) -> HKL {
    unsafe { GetKeyboardLayout(GetWindowThreadProcessId(window, None)) }
}

fn layout_list() -> Vec<isize> {
    let mut layouts = [HKL::default(); 32];
    let count = unsafe { GetKeyboardLayoutList(Some(&mut layouts)) };
    layouts.iter().take(count.max(0) as usize).map(|layout| layout.0 as isize).collect()
}

fn ime_is_open(window: HWND) -> bool {
    const IMC_GETOPENSTATUS: usize = 5;
    unsafe {
        let ime = ImmGetDefaultIMEWnd(window);
        if ime.0.is_null() {
            return false;
        }
        let mut open = 0usize;
        let _ = SendMessageTimeoutW(ime, WM_IME_CONTROL, WPARAM(IMC_GETOPENSTATUS), LPARAM(0), SMTO_BLOCK, 1000, Some(&mut open));
        open != 0
    }
}

fn close_ime(window: HWND) {
    const IMC_SETOPENSTATUS: usize = 6;
    unsafe {
        let ime = ImmGetDefaultIMEWnd(window);
        if !ime.0.is_null() {
            let _ = SendMessageTimeoutW(ime, WM_IME_CONTROL, WPARAM(IMC_SETOPENSTATUS), LPARAM(0), SMTO_BLOCK, 1000, None);
        }
    }
}

fn assert_released() {
    for (name, key) in [("Shift", VK_SHIFT.0), ("Ctrl", VK_CONTROL.0), ("Alt", VK_MENU.0)] {
        let down = unsafe { (GetAsyncKeyState(i32::from(key)) as u16) & 0x8000 != 0 };
        assert!(!down, "{name} is stuck down");
    }
}

/// The interactive-desktop lock, or None (with a SKIP line) when there is no desktop to type on.
fn interactive() -> Option<interactive_lock::InteractiveLock> {
    let desktop = unsafe { OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) };
    let Ok(desktop) = desktop else {
        eprintln!("SKIP interactive: no desktop");
        return None;
    };
    let _ = unsafe { CloseDesktop(desktop) };
    Some(interactive_lock::acquire())
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
