use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn injected_escape_does_not_emit_stopped() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let started = helper.request("session_start", json!({}));
    assert_eq!(started["ok"], true, "{started}");

    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the injected Escape test must fail rather than skip: {activated}");

    let escape = helper.request("press_key", json!({ "window_id": window_id, "keys": ["escape"] }));
    assert_eq!(escape["ok"], true, "{escape}");
    helper.drain();
    assert_eq!(helper.stopped, 0, "helper-injected Escape must not emit stopped");
    let title = title_of(&mut helper, &window_id);
    assert!(title.contains("key:escape"), "{title}");
    let ended = helper.request("session_end", json!({}));
    assert_eq!(ended["ok"], true, "{ended}");
    drop(fixture);
}

#[test]
fn stopping_mid_type_leaves_a_prefix_and_refuses_input_until_resumed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
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

    let text: String = (0..500).map(|index| (b'a' + (index % 26) as u8) as char).collect();
    require_foreground(hwnd);
    let typing = helper.send("type_text", json!({ "window_id": window_id, "text": text }));
    wait_for(Duration::from_secs(5), "typing never started", || len_of(&window_title(hwnd)) > 0);
    helper.send("debug_stop", json!({}));
    let typed = helper.response(typing);
    assert_eq!(typed["ok"], false, "type_text finished instead of stopping: {typed}");
    assert_eq!(typed["error"]["code"], "interrupted", "{typed}");
    let at_stop = len_of(&window_title(hwnd));

    // Characters injected before the stop may still be in the fixture's input queue.
    let title = settled_title(hwnd);
    let length = len_of(&title);
    let shown = edit_of(&title);
    eprintln!("type_text stopped: {at_stop} characters shown at the stop, {length} of 500 after the queue drained");
    assert!(length > 0 && length <= 400, "the fixture should hold a clearly shorter prefix, got {length}: {title}");
    assert!(text.starts_with(&shown), "the fixture text is not a prefix of the typed text: {title}");
    assert_released("after the interrupted type_text");

    require_foreground(hwnd);
    let later = helper.request("type_text", json!({ "window_id": window_id, "text": "zzz" }));
    assert_eq!(later["error"]["code"], "interrupted", "type_text ran while stopped: {later}");
    require_foreground(hwnd);
    let key = helper.request("press_key", json!({ "window_id": window_id, "keys": ["shift", "x"] }));
    assert_eq!(key["error"]["code"], "interrupted", "press_key ran while stopped: {key}");
    require_foreground(hwnd);
    let clicked = helper.request(
        "click",
        json!({ "window_id": window_id, "x": 60, "y": 60, "button": "left", "click_count": 1, "modifiers": ["ctrl"] }),
    );
    assert_eq!(clicked["error"]["code"], "interrupted", "click ran while stopped: {clicked}");
    std::thread::sleep(Duration::from_millis(200));
    let unchanged = window_title(hwnd);
    assert_eq!(len_of(&unchanged), length, "input reached the fixture while stopped: {unchanged}");
    assert!(!unchanged.contains("click:left"), "a click reached the fixture while stopped: {unchanged}");
    assert_released("after refused input");

    let resumed = helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    require_foreground(hwnd);
    let after = helper.request("type_text", json!({ "window_id": window_id, "text": "!" }));
    assert_eq!(after["ok"], true, "type_text after resume: {after}");
    assert_eq!(len_of(&window_title(hwnd)), length + 1, "typing did not work after resume");
    let ended = helper.request("session_end", json!({}));
    assert_eq!(ended["ok"], true, "{ended}");
    assert_released("at the end");
    drop(helper);
    drop(fixture);
}

#[test]
fn stopping_mid_drag_releases_the_left_button() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let started = helper.request("session_start", json!({}));
    assert_eq!(started["ok"], true, "{started}");
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the stop test must fail rather than skip: {activated}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let (from_x, to_x, y) = (width * 30 / 480, width * 440 / 480, height * 190 / 360);
    assert_released("before the drag");

    require_foreground(hwnd);
    let dragging = helper.send("drag", json!({ "window_id": window_id, "from_x": from_x, "from_y": y, "to_x": to_x, "to_y": y, "modifiers": ["shift"] }));
    wait_for(Duration::from_secs(5), "the drag never pressed the left button", || key_down(VK_LBUTTON.0));
    let shift_held = key_down(VK_SHIFT.0);
    helper.send("debug_stop", json!({}));
    let dragged = helper.response(dragging);
    assert_eq!(dragged["ok"], false, "drag finished instead of stopping: {dragged}");
    assert_eq!(dragged["error"]["code"], "interrupted", "{dragged}");
    let title = settled_title(hwnd);
    assert_released("after the interrupted drag");
    assert!(shift_held, "Shift was not down while the drag held the left button");

    let (start, end) = recorded_drag(&title).unwrap_or_else(|| panic!("the fixture saw no button release: {title}"));
    eprintln!("drag stopped at {start:?} -> {end:?}; {} px were requested", to_x - from_x);
    assert!(
        i64::from(end.0 - start.0) < (to_x - from_x) * 3 / 4,
        "the drag reached its end instead of stopping mid-way: {title}"
    );

    require_foreground(hwnd);
    let again = helper.request("drag", json!({ "window_id": window_id, "from_x": from_x, "from_y": y, "to_x": to_x, "to_y": y }));
    assert_eq!(again["error"]["code"], "interrupted", "drag ran while stopped: {again}");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(recorded_drag(&window_title(hwnd)), Some((start, end)), "a drag reached the fixture while stopped");
    assert_released("at the end");
    let ended = helper.request("session_end", json!({}));
    assert_eq!(ended["ok"], true, "{ended}");
    drop(helper);
    drop(fixture);
}

#[test]
fn stopping_mid_scroll_sends_fewer_notches_and_refuses_more() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let started = helper.request("session_start", json!({}));
    assert_eq!(started["ok"], true, "{started}");
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the stop test must fail rather than skip: {activated}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let (x, y) = (width * 200 / 480, height * 160 / 360);

    require_foreground(hwnd);
    let scrolling = helper.send("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "down", "amount": 50 }));
    wait_for(Duration::from_secs(5), "the scroll never started", || wheel_count(&window_title(hwnd)) > 0);
    helper.send("debug_stop", json!({}));
    let scrolled = helper.response(scrolling);
    assert_eq!(scrolled["ok"], false, "scroll finished instead of stopping: {scrolled}");
    assert_eq!(scrolled["error"]["code"], "interrupted", "{scrolled}");
    let title = settled_title(hwnd);
    let count = wheel_count(&title);
    eprintln!("scroll stopped after {count} of 50 notches");
    assert!(count > 0 && count < 50, "a stop should leave a clearly shorter run of notches, got {count}: {title}");

    require_foreground(hwnd);
    let again = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "down", "amount": 5 }));
    assert_eq!(again["error"]["code"], "interrupted", "scroll ran while stopped: {again}");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(wheel_count(&window_title(hwnd)), count, "a wheel event reached the fixture while stopped");
    let ended = helper.request("session_end", json!({}));
    assert_eq!(ended["ok"], true, "{ended}");
    drop(helper);
    drop(fixture);
}

/// How many vertical wheel events the fixture reports in its title (`wheel=v:n=<count>,...`).
fn wheel_count(title: &str) -> u32 {
    title
        .split(" | wheel=v:n=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

/// The last `drag:x1,y1-x2,y2` event in the fixture title.
fn recorded_drag(title: &str) -> Option<((i32, i32), (i32, i32))> {
    let (_, rest) = title.rsplit_once("drag:")?;
    let (from, to) = rest.split_once('-')?;
    let pair = |text: &str| -> Option<(i32, i32)> {
        let mut parts = text.split(',');
        Some((parts.next()?.trim().parse().ok()?, parts.next()?.trim().parse().ok()?))
    };
    Some((pair(from)?, pair(to)?))
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
    let deadline = std::time::Instant::now() + limit;
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The fixture's title once it has not changed for half a second.
fn settled_title(hwnd: HWND) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut title = window_title(hwnd);
    let mut since = std::time::Instant::now();
    while since.elapsed() < Duration::from_millis(500) {
        assert!(std::time::Instant::now() < deadline, "the fixture kept changing after the stop: {title}");
        std::thread::sleep(Duration::from_millis(20));
        let now = window_title(hwnd);
        if now != title {
            title = now;
            since = std::time::Instant::now();
        }
    }
    title
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

fn parse_hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

fn title_of(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
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
    stopped: u32,
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
        let helper = Self { child, incoming, next_id: 1, stopped: 0 };
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
        loop {
            let value = self.incoming.recv_timeout(Duration::from_secs(15)).expect("response");
            if value.get("event").and_then(|event| event.as_str()) == Some("stopped") {
                self.stopped += 1;
                continue;
            }
            if value.get("event").is_some() || value["id"].as_u64() != Some(id) {
                continue;
            }
            return value;
        }
    }

    fn drain(&mut self) {
        while let Ok(value) = self.incoming.recv_timeout(Duration::from_millis(400)) {
            if value.get("event").and_then(|event| event.as_str()) == Some("stopped") {
                self.stopped += 1;
            }
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
