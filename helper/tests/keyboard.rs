use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, GetKeyboardLayoutList, RegisterHotKey, UnloadKeyboardLayout, UnregisterHotKey, HKL, HOT_KEY_MODIFIERS, MOD_ALT,
    MOD_CONTROL, MOD_SHIFT, VK_CONTROL, VK_F13, VK_LBUTTON, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, PeekMessageW, PostMessageW, SendMessageTimeoutW, MSG, PM_REMOVE, SMTO_BLOCK,
    WM_HOTKEY, WM_IME_CONTROL, WM_INPUTLANGCHANGEREQUEST,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The server's timeout for a `press_key` request (`DEFAULT_REQUEST_TIMEOUT_MS` in
/// `src/config.ts`). Past it the server kills the helper, and nothing reopens the input method
/// `press_key` closed.
const SERVER_TIMEOUT: Duration = Duration::from_secs(10);

const IMC_GETOPENSTATUS: usize = 5;
const IMC_SETOPENSTATUS: usize = 6;

#[test]
fn types_chinese_replaces_after_ctrl_a_and_records_escape() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    if activated["ok"] != true {
        panic!("foreground failed, so the keyboard test must fail rather than skip: {activated}");
    }
    click_edit(&mut helper, &window_id);
    // 😀 is a surrogate pair: both halves go in one SendInput and the pair is read as one key.
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "你好😀" }));
    assert_eq!(typed["ok"], true, "{typed}");
    let title = title_of(&mut helper, &window_id);
    assert!(title.contains("edit=你好😀 |"), "{title}");
    let select_all = helper.request("press_key", json!({ "window_id": window_id, "keys": ["ctrl", "a"] }));
    assert_eq!(select_all["ok"], true, "{select_all}");
    // The helper probed Ctrl+A for a global hotkey before sending the A (ticket 52): the probe's
    // own registration must be gone, so this thread can register the same combination.
    let probed = TestHotkey::register_combination(0x54, MOD_CONTROL, u16::from(b'A'));
    assert!(probed.is_ok(), "Ctrl+A is still registered after press_key [ctrl, a] answered (the hotkey probe left its registration): {probed:?}");
    drop(probed);
    let replaced = helper.request("type_text", json!({ "window_id": window_id, "text": "乙" }));
    assert_eq!(replaced["ok"], true, "{replaced}");
    let replaced_title = title_of(&mut helper, &window_id);
    assert!(replaced_title.contains("edit=乙"), "{replaced_title}");
    let escape = helper.request("press_key", json!({ "window_id": window_id, "keys": ["escape"] }));
    assert_eq!(escape["ok"], true, "{escape}");
    let final_title = title_of(&mut helper, &window_id);
    assert!(final_title.contains("key:escape"), "{final_title}");
    drop(helper);
    drop(fixture);
}

/// The main key of a combination reaches the target with its modifier down, even when the
/// target's key state is reset from the system's (as attaching another thread to its input, or a
/// foreground change, does) while the target is still handling the modifier. Here the test
/// attaches itself to the fixture's input while the fixture is busy with the Ctrl press.
#[test]
fn ctrl_a_reaches_the_target_with_ctrl_down_across_a_key_state_reset() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-keys", "300"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the keyboard test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    click_edit(&mut helper, &window_id);
    assert_released("before the key press");

    require_foreground(hwnd);
    let pressing = helper.send("press_key", json!({ "window_id": window_id, "keys": ["ctrl", "a"] }));
    wait_for(Duration::from_secs(5), || format!("the fixture never handled the Ctrl press: {}", window_title(hwnd)), || window_title(hwnd).contains("slow:17"));
    let attached = ResetKeyState::attach(hwnd);
    let pressed = helper.response_within(pressing, SERVER_TIMEOUT).expect("press_key did not answer within the server's timeout");
    drop(attached);
    let title = window_title(hwnd);
    assert_eq!(pressed["ok"], true, "{pressed}; {title}");
    assert!(title.contains("key:a:ctrl"), "the target read A without Ctrl down: {title}");
    assert_released("after the key press");
    drop(helper);
    drop(fixture);
}

/// Ticket 52 (F27 c): a combination registered as a global hotkey (here by this test, with
/// `RegisterHotKey`) never reaches the target: the system hands the main key to the hotkey's
/// owner. `press_key` fails with `input_failed` soon after the main key went unread, saying a
/// hotkey may have taken it, instead of waiting out the call's limit and the drain after it.
#[test]
fn press_key_whose_main_key_a_global_hotkey_takes_fails_soon_and_says_so() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the keyboard test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    click_edit(&mut helper, &window_id);
    assert_released("before the key press");

    let hotkey = TestHotkey::register();
    require_foreground(hwnd);
    let started = Instant::now();
    let pressing = helper.send("press_key", json!({ "window_id": window_id, "keys": ["ctrl", "alt", "shift", "f13"] }));
    let pressed = helper.response_within(pressing, SERVER_TIMEOUT).expect("press_key did not answer within the server's timeout");
    let elapsed = started.elapsed();
    let fired = hotkey.fired();
    drop(hotkey);
    let title = window_title(hwnd);
    eprintln!("press_key of a registered hotkey answered after {elapsed:?} (hotkey fired: {fired}): {pressed}; {title}");
    assert!(fired, "the test's hotkey never fired, so the system did not take the key: {pressed}; {title}");
    assert_eq!(pressed["ok"], false, "{pressed}; {title}");
    assert_eq!(pressed["error"]["code"], "input_failed", "{pressed}");
    let message = pressed["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("hotkey"), "the failure does not say a hotkey may have taken the key: {message}");
    // The key gets `HOTKEY_READ_LIMIT` (0.5 s) after the modifiers were read: about 0.7 s.
    assert!(elapsed < Duration::from_millis(1500), "press_key took {elapsed:?} to give up on a key a hotkey took: {pressed}");
    assert_released("after the key press");
    drop(helper);
    drop(fixture);
}

/// A global hotkey of this thread (`RegisterHotKey` with no window, so `WM_HOTKEY` is posted to
/// this thread's queue), registered without `MOD_NOREPEAT` as most programs do (ticket 54).
/// Unregistered when dropped, on every path a test ends by, panics included.
#[derive(Debug)]
struct TestHotkey {
    id: i32,
}

impl TestHotkey {
    /// Ctrl+Alt+Shift+F13: a combination no one presses.
    fn register() -> Self {
        Self::register_combination(0x52, MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_F13.0)
            .expect("could not register Ctrl+Alt+Shift+F13 as a test hotkey (another program has it?)")
    }

    fn register_combination(id: i32, modifiers: HOT_KEY_MODIFIERS, key: u16) -> windows::core::Result<Self> {
        unsafe { RegisterHotKey(None, id, modifiers, u32::from(key)).map(|()| Self { id }) }
    }

    /// Whether the hotkey's `WM_HOTKEY` reached this thread.
    fn fired(&self) -> bool {
        let mut message = MSG::default();
        unsafe { PeekMessageW(&mut message, None, WM_HOTKEY, WM_HOTKEY, PM_REMOVE).as_bool() }
    }
}

impl Drop for TestHotkey {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(None, self.id);
        }
    }
}

/// A target that is slow to take the input method request: `press_key` answers before the server
/// gives up on it (either way, ok or input_failed), and the input method ends up as it was.
#[test]
fn press_key_into_a_target_slow_to_switch_its_ime_answers_in_time_and_restores_it() {
    press_key_into_a_slow_target_answers_in_time_and_restores_the_ime(&["--slow-ime", "6000"], &["ctrl", "a"], None);
}


/// A target that reads each key press seconds apart: `press_key` answers before the server gives
/// up on it, and the input method and layout end up as they were. The A, which the target reads
/// only after `press_key` answered, still comes with Ctrl and Shift down and is not composed by the
/// input method (`slow:229`, VK_PROCESSKEY): it is reopened only after the target read the keys
/// (ticket 54).
#[test]
fn press_key_into_a_target_slow_to_read_keys_answers_in_time_and_restores_the_ime() {
    press_key_into_a_slow_target_answers_in_time_and_restores_the_ime(&["--slow-keys", "4000"], &["ctrl", "shift", "a"], Some("key:a:shift+ctrl,slow:65"));
}

/// Ticket 54 (F27 b): a target busy 4 s after every key press reads Ctrl at once, Shift at 4 s
/// and A at about 8 s, after `press_key` gave up at the 5-second limit and answered at the 7-second
/// drain. The late A must still come with Ctrl and Shift down, not as a plain A typed into the
/// edit: the target's key state stays shared with the helper (in the background, after the
/// answer) until it read the last release. The fixture's input method is closed first, so the A
/// reaches the edit as itself rather than through the input method.
#[test]
fn a_main_key_the_target_reads_after_press_key_answered_still_comes_with_its_modifiers() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-keys", "4000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the keyboard test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    let _ime = ImeClosed::close(hwnd);
    click_edit(&mut helper, &window_id);
    assert_released("before the key press");

    require_foreground(hwnd);
    let started = Instant::now();
    late_main_key_press(&mut helper, &window_id, hwnd, started);
    wait_for(Duration::from_secs(20), || format!("the fixture never read the A: {}", window_title(hwnd)), || window_title(hwnd).contains("key:a:"));
    let title = window_title(hwnd);
    eprintln!("the fixture read the A {:?} after the request: {title}", started.elapsed());
    assert!(title.contains("key:a:shift+ctrl"), "the target read the late A without Ctrl and Shift down: {title}");
    assert!(title.contains("edit= |"), "the late A was typed into the edit: {title}");
    assert_released("after the key press");
    drop(helper);
    drop(fixture);
}

/// Ticket 54: a `press_key` whose target had not read its last release when it answered waits
/// for that in the background, and the next input call ends that wait before it sends anything
/// (waiting briefly for it, then detaching and reopening the input method), so the two are never
/// attached to the target together. The helper says on stderr why the wait ended.
#[test]
fn the_next_input_call_ends_a_late_press_keys_background_wait_first() {
    late_press_key_ended_by(EndBy::NextInputCall);
}

/// Ticket 54: a stop ends such a background wait at once.
#[test]
fn a_stop_ends_a_late_press_keys_background_wait() {
    late_press_key_ended_by(EndBy::Stop);
}

/// Ticket 54: the helper's exit (stdin closed) ends such a background wait before the process
/// exits, without waiting for the target.
#[test]
fn the_helpers_exit_ends_a_late_press_keys_background_wait() {
    late_press_key_ended_by(EndBy::Exit);
}

enum EndBy {
    NextInputCall,
    Stop,
    Exit,
}

fn late_press_key_ended_by(end: EndBy) {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-keys", "4000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the keyboard test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    let _ime = ImeClosed::close(hwnd);
    click_edit(&mut helper, &window_id);
    assert_released("before the key press");

    require_foreground(hwnd);
    let started = Instant::now();
    late_main_key_press(&mut helper, &window_id, hwnd, started);
    let ending = Instant::now();
    let (expected, done) = match end {
        EndBy::NextInputCall => {
            // Any input method; the fixture is busy, so this one may fail to raise it.
            let moving = helper.send("move_mouse", json!({ "window_id": window_id, "x": 20, "y": 30 }));
            let moved = helper.response_within(moving, SERVER_TIMEOUT).expect("move_mouse did not answer within the server's timeout");
            eprintln!("the next input call answered after {:?}: {moved}", ending.elapsed());
            assert!(moved["ok"] == true || moved["error"]["code"] == "input_failed", "{moved}");
            (ENDED_BY_CALL_OR_EXIT, ending.elapsed())
        }
        EndBy::Stop => {
            helper.send("debug_stop", json!({}));
            ("press_key: stopped before the target read the last release", Duration::ZERO)
        }
        EndBy::Exit => {
            drop(helper.child.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(5);
            while helper.child.try_wait().expect("helper status").is_none() {
                assert!(Instant::now() < deadline, "the helper did not exit within 5 s of its stdin closing: {}", helper.stderr());
                std::thread::sleep(Duration::from_millis(5));
            }
            eprintln!("the helper exited {:?} after its stdin closed", ending.elapsed());
            (ENDED_BY_CALL_OR_EXIT, ending.elapsed())
        }
    };
    wait_for(Duration::from_secs(1) + done, || format!("the helper did not report the background wait ending: {}", helper.stderr()), || helper.stderr().contains(expected));
    let stderr = helper.stderr();
    eprintln!("the background wait was reported {:?} after the answer; stderr: {}", ending.elapsed(), stderr.trim());
    // When it ended, as the helper logs it (`<seconds> s after the request`). Not ended, the wait
    // would last until the fixture reads the releases, about 12 s after the request.
    let logged = stderr[stderr.find(expected).unwrap() + expected.len()..].split_whitespace().next().and_then(|seconds| seconds.parse::<f64>().ok());
    let logged = logged.unwrap_or_else(|| panic!("no time after the background wait's line: {stderr}"));
    assert!(logged < 8.5, "the background wait ended {logged} s after the request, not soon after the answer at about 7 s: {stderr}");
    assert_released("after the key press");
    drop(helper);
    drop(fixture);
}

const ENDED_BY_CALL_OR_EXIT: &str = "press_key: ended by the next input call or the helper's exit before the target read the last release";

/// Sends `press_key [ctrl, shift, a]` to a fixture busy 4 s after every press and checks it
/// answered `input_failed` in time, before the fixture read the A (or the run shows nothing late).
fn late_main_key_press(helper: &mut Helper, window_id: &str, hwnd: HWND, started: Instant) -> Value {
    let pressing = helper.send("press_key", json!({ "window_id": window_id, "keys": ["ctrl", "shift", "a"] }));
    let pressed = helper
        .response_within(pressing, SERVER_TIMEOUT)
        .unwrap_or_else(|| panic!("press_key did not answer within the server's {SERVER_TIMEOUT:?}: {}", window_title(hwnd)));
    let answered = started.elapsed();
    let title = window_title(hwnd);
    eprintln!("press_key [ctrl, shift, a] answered after {answered:?}: {pressed}; {title}");
    assert_eq!(pressed["error"]["code"], "input_failed", "expected the 5-second limit to end the call before the fixture read the A: {pressed}; {title}");
    assert!(answered < Duration::from_secs(8), "press_key took {answered:?} to answer: {pressed}");
    assert!(!title.contains("key:a:"), "the fixture read the A before press_key answered, so nothing was late: {title}");
    pressed
}

/// The fixture's input method closed (`IMC_SETOPENSTATUS` 0) and reopened when dropped if it was
/// open, so keys reach its edit as themselves.
struct ImeClosed {
    ime: HWND,
    was_open: bool,
}

impl ImeClosed {
    fn close(hwnd: HWND) -> Self {
        let was_open = ime_is_open(hwnd);
        unsafe {
            let ime = ImmGetDefaultIMEWnd(hwnd);
            if !ime.0.is_null() {
                let answered = SendMessageTimeoutW(ime, WM_IME_CONTROL, WPARAM(IMC_SETOPENSTATUS), LPARAM(0), SMTO_BLOCK, 2_000, None);
                assert!(answered.0 != 0, "the fixture did not take the request closing its input method");
            }
            let closed = Self { ime, was_open };
            assert!(!ime_is_open(hwnd), "the fixture's input method is still open");
            closed
        }
    }
}

impl Drop for ImeClosed {
    fn drop(&mut self) {
        if self.was_open && !self.ime.0.is_null() {
            unsafe {
                let _ = SendMessageTimeoutW(self.ime, WM_IME_CONTROL, WPARAM(IMC_SETOPENSTATUS), LPARAM(1), SMTO_BLOCK, 5_000, None);
            }
        }
    }
}

/// Ticket 27 major-1: a `press_key` that could not close a slow target's input method within
/// `IME_REQUEST_LIMIT_MS` leaves a reopen pending in a background thread (`ImeGuard::drop`). The
/// very next `press_key` on the same target must not read the input method as closed while that
/// reopen might still be ahead of it in the target's queue, or the target could end up composing
/// the second key once it catches up to both, while `press_key` reports it as sent plainly.
#[test]
fn press_key_after_a_delayed_ime_reopen_does_not_let_the_ime_compose_the_next_key() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-ime", "3000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the keyboard test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    let target_layout = layout_of(hwnd);
    let ime_open = ime_is_open(hwnd);
    if target_layout == 0x04040404 {
        assert!(ime_open, "zh-TW is the fixture layout but the IME is closed");
    }
    click_edit(&mut helper, &window_id);

    require_foreground(hwnd);
    let started = Instant::now();
    let first = helper.request("press_key", json!({ "window_id": window_id, "keys": ["b"] }));
    eprintln!("first press_key answered after {:?}: {first}", started.elapsed());
    assert_eq!(
        first["error"]["code"], "input_failed",
        "expected the first press_key to fail closing a target too slow to answer within 500 ms, leaving a reopen pending: {first}"
    );

    require_foreground(hwnd);
    let second_started = Instant::now();
    let second = helper.request("press_key", json!({ "window_id": window_id, "keys": ["c"] }));
    let title = window_title(hwnd);
    eprintln!("second press_key answered after {:?}: {second}; {title}", second_started.elapsed());
    assert!(second["ok"] == true || second["error"]["code"] == "input_failed", "{second}; {title}");
    if second["ok"] == true {
        assert!(title.contains("edit=c"), "the second key did not reach the target as itself, so the input method must have composed it while press_key reported it sent: {title}");
    } else {
        assert!(!title.contains("edit=c"), "input_failed but the character reached the target anyway: {title}");
    }

    wait_for(Duration::from_secs(20), || format!("IME open status did not return to {ime_open}: {}", window_title(hwnd)), || ime_is_open(hwnd) == ime_open);
    let mut settled_title = window_title(hwnd);
    let mut quiet_since = Instant::now();
    wait_for(Duration::from_secs(15), || format!("the fixture kept handling input: {}", window_title(hwnd)), || {
        let now = window_title(hwnd);
        if now != settled_title {
            settled_title = now;
            quiet_since = Instant::now();
        }
        quiet_since.elapsed() >= Duration::from_secs(5)
    });
    assert_eq!(ime_is_open(hwnd), ime_open, "IME open status changed once the fixture handled the keys: {settled_title}");
    assert_eq!(layout_of(hwnd), target_layout, "fixture thread layout changed");
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    assert_eq!(layout_value(restore.foreground_thread), restore.foreground_layout, "foreground thread layout changed");
    assert_released("after the key press");
    drop(helper);
    drop(fixture);
}

/// `late`: events the fixture must have recorded once it is done (`None`: no check).
fn press_key_into_a_slow_target_answers_in_time_and_restores_the_ime(args: &[&str], keys: &[&str], late: Option<&str>) {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(args).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the keyboard test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    let target_layout = layout_of(hwnd);
    let ime_open = ime_is_open(hwnd);
    if target_layout == 0x04040404 {
        assert!(ime_open, "zh-TW is the fixture layout but the IME is closed");
    }
    click_edit(&mut helper, &window_id);

    require_foreground(hwnd);
    let started = Instant::now();
    let pressing = helper.send("press_key", json!({ "window_id": window_id, "keys": keys }));
    let pressed = helper
        .response_within(pressing, SERVER_TIMEOUT)
        .unwrap_or_else(|| panic!("press_key did not answer within the server's {SERVER_TIMEOUT:?}: {}", window_title(hwnd)));
    eprintln!("press_key {keys:?} with {args:?} answered after {:?}: {pressed}", started.elapsed());
    assert!(pressed["ok"] == true || pressed["error"]["code"] == "input_failed", "{pressed}");
    // A busy fixture takes the helper's input method request only once it is done with the keys;
    // the helper stays alive meanwhile, as it does for the server.
    wait_for(Duration::from_secs(20), || format!("IME open status did not return to {ime_open}: {}", window_title(hwnd)), || ime_is_open(hwnd) == ime_open);
    eprintln!("IME open status back to {ime_open} after {:?}: {}", started.elapsed(), window_title(hwnd));
    // Still so once the fixture has handled every key it was sent (its title stops changing).
    let mut title = window_title(hwnd);
    let mut quiet_since = Instant::now();
    wait_for(Duration::from_secs(30), || format!("the fixture kept handling input: {}", window_title(hwnd)), || {
        let now = window_title(hwnd);
        if now != title {
            title = now;
            quiet_since = Instant::now();
        }
        quiet_since.elapsed() >= Duration::from_secs(5)
    });
    assert_eq!(ime_is_open(hwnd), ime_open, "IME open status changed once the fixture handled the keys: {title}");
    if let Some(late) = late {
        assert!(title.contains(late), "the fixture did not read the keys as {late}: {title}");
    }
    assert_eq!(layout_of(hwnd), target_layout, "fixture thread layout changed");
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    assert_eq!(layout_value(restore.foreground_thread), restore.foreground_layout, "foreground thread layout changed");
    assert_released("after the key press");
    drop(helper);
    drop(fixture);
}

/// Attaching this thread to the target's input resets the shared key state to the system's, as a
/// foreground change can. Detached when dropped.
struct ResetKeyState {
    current: u32,
    target: u32,
}

impl ResetKeyState {
    fn attach(hwnd: HWND) -> Self {
        unsafe {
            let current = GetCurrentThreadId();
            let target = GetWindowThreadProcessId(hwnd, None);
            assert!(AttachThreadInput(current, target, true).as_bool(), "could not attach to the fixture's input");
            Self { current, target }
        }
    }
}

impl Drop for ResetKeyState {
    fn drop(&mut self) {
        unsafe {
            let _ = AttachThreadInput(self.current, self.target, false);
        }
    }
}

fn click_edit(helper: &mut Helper, window_id: &str) {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let clicked = helper.request("click", json!({ "window_id": window_id, "x": width * 240 / 480, "y": height * 24 / 360, "button": "left", "click_count": 1 }));
    assert_eq!(clicked["ok"], true, "{clicked}");
}

fn title_of(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

/// The fixture's title, read directly so it works while the helper is busy.
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn require_foreground(expected: HWND) {
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, expected, "refusing to send input: the fixture is not in the foreground");
}

fn assert_released(when: &str) {
    for (name, key) in [("Shift", VK_SHIFT.0), ("Ctrl", VK_CONTROL.0), ("Alt", VK_MENU.0), ("the left button", VK_LBUTTON.0)] {
        let down = unsafe { (GetAsyncKeyState(i32::from(key)) as u16) & 0x8000 != 0 };
        assert!(!down, "{name} is stuck down {when}");
    }
}

fn wait_for(limit: Duration, what: impl Fn() -> String, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < deadline, "{}", what());
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Whether the fixture's input method is open. The answer does not wait for requests still queued
/// for a busy fixture, so callers poll for the state they expect.
fn ime_is_open(hwnd: HWND) -> bool {
    unsafe {
        let ime = ImmGetDefaultIMEWnd(hwnd);
        if ime.0.is_null() {
            return false;
        }
        let mut open = 0usize;
        let answered = SendMessageTimeoutW(ime, WM_IME_CONTROL, WPARAM(IMC_GETOPENSTATUS), LPARAM(0), SMTO_BLOCK, 20_000, Some(&mut open));
        assert!(answered.0 != 0, "the fixture did not answer the IME status request within 20 seconds");
        open != 0
    }
}

fn layout_of(hwnd: HWND) -> u64 {
    unsafe { layout_value(GetWindowThreadProcessId(hwnd, None)) }
}

fn layout_value(thread: u32) -> u64 {
    unsafe { GetKeyboardLayout(thread).0 as usize as u64 }
}

fn layout_list() -> Vec<u64> {
    unsafe {
        let mut buffer = [HKL::default(); 32];
        let count = GetKeyboardLayoutList(Some(&mut buffer));
        buffer.iter().take(count.max(0) as usize).map(|layout| layout.0 as usize as u64).collect()
    }
}

/// Puts the foreground thread's layout and the layout list back as they were, even when the test
/// fails because the helper was killed before it could restore them.
struct LayoutGuard {
    foreground: HWND,
    foreground_thread: u32,
    foreground_layout: u64,
    list: Vec<u64>,
}

impl LayoutGuard {
    fn capture() -> Self {
        unsafe {
            let foreground = GetForegroundWindow();
            let foreground_thread = GetWindowThreadProcessId(foreground, None);
            Self { foreground, foreground_thread, foreground_layout: layout_value(foreground_thread), list: layout_list() }
        }
    }
}

impl Drop for LayoutGuard {
    fn drop(&mut self) {
        unsafe {
            if layout_value(self.foreground_thread) != self.foreground_layout && !self.foreground.0.is_null() {
                let _ = PostMessageW(Some(self.foreground), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(self.foreground_layout as isize));
                let deadline = Instant::now() + Duration::from_millis(500);
                while Instant::now() < deadline && layout_value(self.foreground_thread) != self.foreground_layout {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            let mut buffer = [HKL::default(); 32];
            let count = GetKeyboardLayoutList(Some(&mut buffer));
            for layout in buffer.iter().take(count.max(0) as usize) {
                if !self.list.contains(&(layout.0 as usize as u64)) {
                    let _ = UnloadKeyboardLayout(*layout);
                }
            }
        }
    }
}

fn parse_hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).and_then(|_| window["window_id"].as_str().map(str::to_string)))
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture was not listed");
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
    incoming: Receiver<Value>,
    next_id: u64,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
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
        let stderr = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let mut errors = BufReader::new(child.stderr.take().expect("stderr"));
        let collected = stderr.clone();
        std::thread::spawn(move || loop {
            let mut line = String::new();
            match errors.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => collected.lock().unwrap().push_str(&line),
            }
        });
        let (tx, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&line) {
                            let _ = tx.send(value);
                        }
                    }
                }
            }
        });
        let helper = Self { child, incoming, next_id: 1, stderr };
        let ready = helper.incoming.recv_timeout(Duration::from_secs(5)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        helper
    }

    /// Everything the helper wrote to stderr so far.
    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.response_within(id, Duration::from_secs(15)).expect("response")
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        id
    }

    /// The response to request `id` if it arrives within `limit`; events and responses to other
    /// requests are skipped.
    fn response_within(&mut self, id: u64, limit: Duration) -> Option<Value> {
        let deadline = Instant::now() + limit;
        loop {
            let value = self.incoming.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok()?;
            if value.get("event").is_some() || value["id"].as_u64() != Some(id) {
                continue;
            }
            return Some(value);
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
