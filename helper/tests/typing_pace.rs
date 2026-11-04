use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, GetKeyboardLayoutList, UnloadKeyboardLayout, HKL, VK_CONTROL, VK_LBUTTON, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// Milliseconds the fixture's edit sleeps on every character (`--slow-input`), so a target that
/// processes input slower than it is injected is reproducible without outside load.
const SLOW_MS: &str = "10";

/// Characters a stop may still let through: the helper keeps at most one key unread by the target,
/// plus one it has read but not yet shown, with room for the stop reaching the helper late.
const IN_FLIGHT_MARGIN: usize = 8;

/// The server's timeout for a `type_text` request (`TYPE_TEXT_TIMEOUT_MS` in `src/config.ts`).
/// Past it the server kills the helper, and nothing restores the keyboard layout.
const SERVER_TIMEOUT: Duration = Duration::from_secs(30);

/// Text too long to type within one call's time limit answers `input_failed` before the server
/// gives up on the request, says how much was typed, and still restores the keyboard layout
/// (non-ASCII text switches the target to the English layout while typing).
#[test]
fn text_too_long_for_one_call_stops_at_the_time_limit_and_restores_the_layout() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-input", SLOW_MS]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the typing test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    let target_layout = layout_of(hwnd);
    click_edit(&mut helper, &window_id);

    let total = 3000;
    let text = "你".repeat(total);
    require_foreground(hwnd);
    let started = Instant::now();
    let typing = helper.send("type_text", json!({ "window_id": window_id, "text": text }));
    let typed = helper.response_within(typing, SERVER_TIMEOUT).unwrap_or_else(|| {
        panic!("type_text did not answer within the server's {SERVER_TIMEOUT:?}; the fixture has {} characters", len_of(&window_title(hwnd)))
    });
    let elapsed = started.elapsed();
    std::thread::sleep(Duration::from_millis(300));
    let title = window_title(hwnd);
    eprintln!("type_text of {total} characters answered after {elapsed:?} with {typed}; the fixture has {}", len_of(&title));
    assert_eq!(typed["ok"], false, "{typed}");
    assert_eq!(typed["error"]["code"], "input_failed", "{typed}");
    let message = typed["error"]["message"].as_str().unwrap_or("");
    let count: usize = message
        .strip_prefix("Typed ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("the error does not say how many characters were typed: {message}"));
    assert!(message.contains(&format!("of {total} characters")), "the error does not give the total: {message}");
    assert!(count > 0 && count < total, "{message}");
    assert_eq!(len_of(&title), count, "the fixture holds a different number of characters than reported: {title}");
    assert_eq!(layout_of(hwnd), target_layout, "fixture thread layout was not restored");
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    assert_eq!(layout_value(restore.foreground_thread), restore.foreground_layout, "foreground thread layout changed");
    assert_released("after the time limit");
    drop(helper);
    drop(fixture);
}

/// `type_text` answers ok only once the target has processed every character: the title read
/// right after the response already holds the whole text.
#[test]
fn type_text_returns_only_after_a_slow_target_processed_every_character() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-input", SLOW_MS]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the typing test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    click_edit(&mut helper, &window_id);

    let text: String = (0..60).map(|index| (b'a' + (index % 26) as u8) as char).collect();
    require_foreground(hwnd);
    let started = Instant::now();
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": text }));
    let elapsed = started.elapsed();
    let title = window_title(hwnd);
    assert_eq!(typed["ok"], true, "{typed}");
    eprintln!("type_text of {} characters into a {SLOW_MS} ms/character target took {elapsed:?}", text.len());
    assert_eq!(len_of(&title), text.len(), "type_text answered ok before the target processed the text: {title}");
    assert_eq!(edit_of(&title), text, "{title}");
    assert_released("after typing");
    drop(helper);
    drop(fixture);
}

/// A stop in the middle of typing into a slow target lets through only the few characters already
/// handed to it: the text stops growing within a second instead of draining a long queue.
#[test]
fn stopping_a_slow_target_mid_type_lets_only_a_few_characters_through() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-input", SLOW_MS]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let started = helper.request("session_start", json!({}));
    assert_eq!(started["ok"], true, "{started}");
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the stop test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    click_edit(&mut helper, &window_id);
    assert_released("before typing");

    let text: String = (0..300).map(|index| (b'a' + (index % 26) as u8) as char).collect();
    require_foreground(hwnd);
    let typing = helper.send("type_text", json!({ "window_id": window_id, "text": text }));
    wait_for(Duration::from_secs(5), "typing never started", || len_of(&window_title(hwnd)) > 0);
    helper.send("debug_stop", json!({}));
    let stopped_at = Instant::now();
    let at_stop = len_of(&window_title(hwnd));
    let typed = helper.response(typing);

    std::thread::sleep(Duration::from_secs(1).saturating_sub(stopped_at.elapsed()));
    let settled = window_title(hwnd);
    std::thread::sleep(Duration::from_millis(500));
    let later = window_title(hwnd);
    let length = len_of(&later);
    eprintln!("stopped at {at_stop} characters; {} a second later, {length} after 1.5 s", len_of(&settled));
    assert_eq!(settled, later, "the target was still receiving characters a second after the stop");
    assert!(length >= at_stop && length - at_stop <= IN_FLIGHT_MARGIN, "{} characters arrived after the stop (at most {IN_FLIGHT_MARGIN}): {later}", length - at_stop);
    assert!(text.starts_with(&edit_of(&later)), "the fixture text is not a prefix of the typed text: {later}");
    assert_eq!(typed["ok"], false, "type_text finished instead of stopping: {typed}");
    assert_eq!(typed["error"]["code"], "interrupted", "{typed}");
    assert_released("after the interrupted type_text");
    let ended = helper.request("session_end", json!({}));
    assert_eq!(ended["ok"], true, "{ended}");
    drop(helper);
    drop(fixture);
}

/// A target that stops reading its input mid-text: `type_text` gives up with a message that says
/// how many characters were typed out of how many, and that the next one was sent but not
/// confirmed read.
#[test]
fn a_target_that_stops_reading_gets_a_message_with_the_typed_count() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    // Longer than the helper's 5-second read limit on every character.
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--slow-input", "6000"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the typing test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    click_edit(&mut helper, &window_id);

    require_foreground(hwnd);
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "abc" }));
    eprintln!("type_text into a target that stopped reading: {typed}");
    assert_eq!(typed["ok"], false, "{typed}");
    assert_eq!(typed["error"]["code"], "input_failed", "{typed}");
    let message = typed["error"]["message"].as_str().unwrap_or("");
    // Usually character 1. Windows may resync the key state of a thread that has not read input
    // for seconds from the system's, which can look like the key was read; then a later one is
    // named, with the count before it.
    let unconfirmed: usize = message
        .split("did not read character ")
        .nth(1)
        .and_then(|rest| rest.split(" of 3").next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("the error does not name the unconfirmed character out of 3: {message}"));
    assert!((1..=3).contains(&unconfirmed), "{message}");
    let typed_count = unconfirmed - 1;
    assert!(message.contains(&format!("; {typed_count} of 3 characters were typed")), "the error does not give the typed count: {message}");
    assert!(message.contains(&format!("Character {unconfirmed} was sent but not confirmed")), "the error does not say the character was sent: {message}");
    assert!(len_of(&window_title(hwnd)) <= unconfirmed, "more characters reached the fixture than were sent: {}", window_title(hwnd));
    assert_released("after the stalled type_text");
    drop(helper);
    drop(fixture);
}

/// Milliseconds the fixture's edit stays busy on every character while it still answers sent
/// messages (`--busy-pump`): longer than the 100 ms after which `wait_typed` asks the target.
const BUSY_PUMP_MS: &str = "400";

/// Ticket 49 (F11): a target that answers sent messages while it is still busy with a character
/// answers the two `WM_NULL` of `wait_typed`'s fallback before it has read the key's release.
/// Typing still runs at most one key ahead of it (before ticket 49: 4 keys ahead of 5, and the
/// call succeeded with only the first read), and the call answers once it has read every one.
#[test]
fn typing_into_a_target_that_answers_sent_messages_while_busy_runs_at_most_one_key_ahead() {
    let Some((_interactive, fixture, mut helper, window_id, hwnd)) = open_busy_pump() else { return };
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "12345" }));
    let answered = window_title(hwnd);
    // Every character the edit was sent, once it had time to handle them.
    wait_for(Duration::from_secs(5), "the fixture did not get every character", || ahead_of(&window_title(hwnd)).len() == 5 && edit_of(&window_title(hwnd)) == "12345");
    let title = window_title(hwnd);
    drop(helper);
    drop(fixture);
    let ahead = ahead_of(&title);
    eprintln!("type_text into a target busy {BUSY_PUMP_MS} ms per character: {typed}; ahead per key {ahead:?}; at the answer: {answered}");
    assert_eq!(typed["ok"], true, "{typed}");
    assert_eq!(edit_of(&title), "12345", "{title}");
    assert!(ahead.iter().all(|ahead| *ahead <= 1), "typing ran more than one key ahead of the target: {ahead:?}");
    assert_eq!(edit_of(&answered), "12345", "type_text answered ok before the target read every character: {answered}");
}

/// Ticket 49 (F11): Enter in typed text never takes that fallback: the key after it is sent only
/// once the target finished handling Enter, even when it answers sent messages meanwhile.
#[test]
fn the_key_after_enter_waits_for_a_target_that_answers_sent_messages_while_busy() {
    let Some((_interactive, fixture, mut helper, window_id, hwnd)) = open_busy_pump() else { return };
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "1\n2" }));
    let title = window_title(hwnd);
    drop(helper);
    drop(fixture);
    let ahead = ahead_of(&title);
    eprintln!("type_text with Enter into a busy target: {typed}; ahead per key {ahead:?}");
    assert_eq!(typed["ok"], true, "{typed}");
    assert_eq!(ahead.len(), 3, "{title}");
    assert_eq!(ahead[2], 0, "the key after Enter was sent before the target finished handling Enter: {ahead:?}");
    assert!(edit_of(&title).ends_with('2'), "{title}");
}

fn open_busy_pump() -> Option<(interactive_lock::InteractiveLock, KillOnDrop, Helper, String, HWND)> {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return None;
    }
    let interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--busy-pump", BUSY_PUMP_MS]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the typing test must fail rather than skip: {activated}");
    require_foreground(hwnd);
    click_edit(&mut helper, &window_id);
    require_foreground(hwnd);
    Some((interactive, fixture, helper, window_id, hwnd))
}

/// The `--busy-pump` fixture's ` | ahead=` list: per key press read, how many characters before it
/// were still being handled when it was sent.
fn ahead_of(title: &str) -> Vec<usize> {
    title.split(" | ahead=").nth(1).unwrap_or("").split(',').filter_map(|value| value.parse().ok()).collect()
}

fn click_edit(helper: &mut Helper, window_id: &str) {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let clicked = helper.request(
        "click",
        json!({ "window_id": window_id, "x": width * 240 / 480, "y": height * 24 / 360, "button": "left", "click_count": 1 }),
    );
    assert_eq!(clicked["ok"], true, "{clicked}");
}

fn require_foreground(expected: HWND) {
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, expected, "refusing to send input: the fixture is not in the foreground");
}

fn key_down(key: u16) -> bool {
    unsafe { (GetAsyncKeyState(i32::from(key)) as u16) & 0x8000 != 0 }
}

fn assert_released(when: &str) {
    for (name, key) in [("Shift", VK_SHIFT.0), ("Ctrl", VK_CONTROL.0), ("Alt", VK_MENU.0), ("the left button", VK_LBUTTON.0)] {
        assert!(!key_down(key), "{name} is stuck down {when}");
    }
}

fn wait_for(limit: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The fixture's title, read directly so it works while the helper is busy.
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn edit_of(title: &str) -> String {
    title.split("edit=").nth(1).unwrap_or("").split(" | len=").next().unwrap_or("").to_string()
}

fn len_of(title: &str) -> usize {
    title.split("len=").nth(1).unwrap_or("0").split(" | ev=").next().unwrap_or("0").parse().unwrap_or(0)
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
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).and_then(|_| window["window_id"].as_str().map(str::to_string))
            })
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
        let helper = Self { child, incoming, next_id: 1 };
        let ready = helper.incoming.recv_timeout(Duration::from_secs(5)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        helper
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.response(id)
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        id
    }

    /// The response to request `id`; events and responses to other requests are skipped.
    fn response(&mut self, id: u64) -> Value {
        self.response_within(id, Duration::from_secs(15)).expect("response")
    }

    /// The response to request `id` if it arrives within `limit`.
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
