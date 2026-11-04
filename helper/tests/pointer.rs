use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn scroll_drag_and_hover_are_recorded_on_the_fixture() {
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
        panic!("foreground failed, so the pointer test must fail rather than skip: {activated}");
    }
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let x = width * 200 / 480;
    let y = height * 160 / 360;
    let scrolled = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "down", "amount": 3 }));
    assert_eq!(scrolled["ok"], true, "{scrolled}");
    let title = title_of(&mut helper, &window_id);
    assert!(title.contains("scroll:"), "{title}");
    let dragged = helper.request("drag", json!({ "window_id": window_id, "from_x": width * 30 / 480, "from_y": height * 120 / 360, "to_x": width * 80 / 480, "to_y": height * 180 / 360 }));
    assert_eq!(dragged["ok"], true, "{dragged}");
    assert!(title_of(&mut helper, &window_id).contains("drag:"), "drag was not recorded");
    let moved = helper.request("move_mouse", json!({ "window_id": window_id, "x": width * 40 / 480, "y": height * 140 / 360 }));
    assert_eq!(moved["ok"], true, "{moved}");
    assert!(title_of(&mut helper, &window_id).contains("hover:"), "hover was not recorded");
    drop(helper);
    drop(fixture);
}

#[test]
fn shift_drag_holds_shift_from_press_to_release() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the drag test must fail rather than skip: {activated}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    assert_released("before the drag");
    require_foreground(&window_id);
    let dragged = helper.request(
        "drag",
        json!({ "window_id": window_id, "from_x": width * 30 / 480, "from_y": height * 120 / 360, "to_x": width * 80 / 480, "to_y": height * 180 / 360, "modifiers": ["shift"] }),
    );
    assert_eq!(dragged["ok"], true, "{dragged}");
    assert_released("after the drag");
    let title = title_of(&mut helper, &window_id);
    assert!(title.contains("drag:"), "the drag was not recorded: {title}");
    assert!(title.contains("dragmods:shift/shift"), "Shift was not held when the button went down and came up: {title}");
    drop(helper);
    drop(fixture);
}

/// Real wheel input is one event per notch: `amount` N reaches the target as N events of 120, never
/// as one event of N x 120 (Chrome caps what a single event scrolls, so 50 notches in one event
/// moved a page about as far as 5).
#[test]
fn scroll_sends_one_wheel_event_per_notch() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the scroll test must fail rather than skip: {activated}");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let (x, y) = (width * 200 / 480, height * 160 / 360);

    require_foreground(&window_id);
    let down = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "down", "amount": 5 }));
    assert_eq!(down["ok"], true, "{down}");
    let title = title_of(&mut helper, &window_id);
    assert_eq!(wheel_of(&title, 'v'), Some(Wheel { count: 5, sum: -600, min: -120, max: -120 }), "5 notches down must be 5 events of -120: {title}");
    assert_eq!(wheel_of(&title, 'h'), None, "{title}");

    require_foreground(&window_id);
    let up = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "up", "amount": 2 }));
    assert_eq!(up["ok"], true, "{up}");
    let title = title_of(&mut helper, &window_id);
    assert_eq!(wheel_of(&title, 'v'), Some(Wheel { count: 7, sum: -360, min: -120, max: 120 }), "2 notches up add 2 events of 120: {title}");

    require_foreground(&window_id);
    let right = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "right", "amount": 4 }));
    assert_eq!(right["ok"], true, "{right}");
    let title = title_of(&mut helper, &window_id);
    assert_eq!(wheel_of(&title, 'h'), Some(Wheel { count: 4, sum: 480, min: 120, max: 120 }), "4 notches right must be 4 events of 120: {title}");

    require_foreground(&window_id);
    let left = helper.request("scroll", json!({ "window_id": window_id, "x": x, "y": y, "direction": "left", "amount": 3 }));
    assert_eq!(left["ok"], true, "{left}");
    let title = title_of(&mut helper, &window_id);
    assert_eq!(wheel_of(&title, 'h'), Some(Wheel { count: 7, sum: 120, min: -120, max: 120 }), "3 notches left add 3 events of -120: {title}");
    assert_eq!(wheel_of(&title, 'v'), Some(Wheel { count: 7, sum: -360, min: -120, max: 120 }), "horizontal scrolling sent vertical events: {title}");
    drop(helper);
    drop(fixture);
}

/// Ticket 53 (F35): a click waits until the target has read its release, for at most 0.3 s. The
/// scroll area stays busy after every press (`--slow-pointer`), so the release waits in its queue.
/// A target that reads it within the cap gets a plain success; a busier one still gets a success
/// (a failure would invite a retry that clicks twice), marked `read_confirmed: false`, and the
/// click lands once it is done. The same holds while the busy target answers sent messages
/// (`--slow-pointer-pumps`), which would make `WM_NULL` replies count it as read, and for a
/// double click (busy after each press, so 100 ms each stays within the cap) and a drag. Either
/// way the call answers within a second (ticket 54).
#[test]
fn a_click_on_a_busy_target_succeeds_only_once_the_target_read_it() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let cases = [
        (Pointer::Click, 150, false),
        (Pointer::Click, 150, true),
        (Pointer::Click, 700, false),
        (Pointer::Click, 700, true),
        (Pointer::DoubleClick, 100, false),
        (Pointer::DoubleClick, 700, false),
        (Pointer::Drag, 150, false),
        (Pointer::Drag, 700, false),
    ];
    for (pointer, busy, pumps) in cases {
        let mut args = vec!["--slow-pointer".to_string(), busy.to_string()];
        if pumps {
            args.push("--slow-pointer-pumps".to_string());
        }
        let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(&args).spawn().expect("fixture"));
        let mut helper = Helper::spawn();
        let window_id = wait_for_fixture(&mut helper);
        let activated = helper.request("activate_window", json!({ "window_id": window_id }));
        assert_eq!(activated["ok"], true, "foreground failed, so the click test must fail rather than skip: {activated}");
        let info = helper.request("get_window", json!({ "window_id": window_id }));
        let width = info["result"]["width"].as_i64().unwrap_or(480);
        let height = info["result"]["height"].as_i64().unwrap_or(360);
        require_foreground(&window_id);
        // The scroll area records a press and release as `drag:` (one per click: its class
        // takes no double clicks), with no `drag:` before.
        let (x, y) = (width * 200 / 480, height * 160 / 360);
        let (method, params, releases) = match pointer {
            Pointer::Click => ("click", json!({ "window_id": window_id, "x": x, "y": y }), 1),
            Pointer::DoubleClick => ("click", json!({ "window_id": window_id, "x": x, "y": y, "click_count": 2 }), 2),
            Pointer::Drag => ("drag", json!({ "window_id": window_id, "from_x": width * 30 / 480, "from_y": height * 120 / 360, "to_x": width * 80 / 480, "to_y": height * 180 / 360 }), 1),
        };
        let started = Instant::now();
        let clicked = helper.request(method, params);
        let elapsed = started.elapsed();
        let title = title_of(&mut helper, &window_id);
        let case = format!("{pointer:?}, busy {busy} ms, pumps {pumps}");
        assert_eq!(clicked["ok"], true, "{case}: a click or drag is never turned into a failure that invites a retry: {clicked}");
        assert!(elapsed < Duration::from_secs(1), "{case}: the call took {elapsed:?}: {clicked}");
        if busy as usize * releases < 300 {
            assert_eq!(title.matches("drag:").count(), releases, "{case}: the call succeeded before the target read the last release: {title}");
            assert!(clicked["result"].get("read_confirmed").is_none(), "{case}: a target that reads it within the cap: {clicked}");
        } else {
            assert_eq!(clicked["result"]["read_confirmed"], false, "{case}: {clicked}");
            // It was sent all the same: the target reads it once it is done.
            let deadline = Instant::now() + Duration::from_secs(3);
            while title_of(&mut helper, &window_id).matches("drag:").count() < releases {
                assert!(Instant::now() < deadline, "{case}: the busy target never read every release: {}", title_of(&mut helper, &window_id));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        assert_released(&format!("after the call ({case})"));
        drop(helper);
        drop(fixture);
    }
}

#[derive(Debug, Clone, Copy)]
enum Pointer {
    Click,
    DoubleClick,
    Drag,
}

#[derive(Debug, PartialEq)]
struct Wheel {
    count: u32,
    sum: i32,
    min: i32,
    max: i32,
}

/// The wheel summary the fixture shows for one axis (`v` or `h`) in its title: `wheel=v:n=5,sum=-600,min=-120,max=-120;h:...`.
fn wheel_of(title: &str, axis: char) -> Option<Wheel> {
    let wheel = title.split(" | wheel=").nth(1)?.split(" | ").next()?;
    let part = wheel.split(';').find_map(|part| part.strip_prefix(&format!("{axis}:")))?;
    let field = |name: &str| part.split(',').find_map(|field| field.strip_prefix(&format!("{name}="))).and_then(|value| value.parse::<i32>().ok());
    Some(Wheel { count: field("n")? as u32, sum: field("sum")?, min: field("min")?, max: field("max")? })
}

fn require_foreground(window_id: &str) {
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground.0 as isize, window_id.parse::<isize>().expect("hwnd"), "refusing to send input: the fixture is not in the foreground");
}

fn assert_released(when: &str) {
    for (name, key) in [("Shift", VK_SHIFT.0), ("Ctrl", VK_CONTROL.0), ("Alt", VK_MENU.0), ("the left button", VK_LBUTTON.0)] {
        assert!(unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000 == 0, "{name} is stuck down {when}");
    }
}

/// Checked before the helper looks up the window, so this sends no input (window 1 does not exist).
#[test]
fn scroll_refuses_an_amount_that_is_not_a_whole_number() {
    let mut helper = Helper::spawn();
    for amount in [json!(2.5), json!("3"), json!(0), json!(-2), json!(null)] {
        let scrolled = helper.request("scroll", json!({ "window_id": "1", "x": 1, "y": 1, "direction": "down", "amount": amount }));
        assert_eq!(scrolled["error"]["code"], "invalid_params", "amount {amount}: {scrolled}");
        assert!(scrolled["error"]["message"].as_str().unwrap_or("").contains("whole number"), "amount {amount}: {scrolled}");
    }
    let defaulted = helper.request("scroll", json!({ "window_id": "1", "x": 1, "y": 1, "direction": "down" }));
    assert_eq!(defaulted["error"]["code"], "window_not_found", "a missing amount defaults to 3: {defaulted}");
}

fn title_of(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}
fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| windows.iter().find_map(|window| window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).and_then(|_| window["window_id"].as_str().map(str::to_string)))) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture was not listed");
}
fn interactive_desktop() -> bool {
    unsafe { let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else { return false }; let _ = CloseDesktop(desktop); true }
}
struct KillOnDrop(Child);
impl Drop for KillOnDrop { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
struct Helper { child: Child, reader: BufReader<std::process::ChildStdout>, next_id: u64 }
impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper")).args(["--server-pid", &std::process::id().to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("helper");
        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        Self { child, reader, next_id: 1 }
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id; self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({"id": id, "method": method, "params": params})).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({"raw": line}));
            if value.get("event").is_some() { continue; }
            return value;
        }
    }
}
impl Drop for Helper { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); } }
