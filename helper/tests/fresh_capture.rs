//! Ticket 42: a screenshot taken right after input shows what the input changed, not an older
//! frame Windows Graphics Capture had cached for the window. The stale frame seen in VS Code did
//! not reproduce with these fixtures (GDI, a window swap chain, DirectComposition); this guards
//! the behaviour and is the base for a reproduction if one is found.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// `PRESENT_COLORS` in `fixture-window.rs`: red before the first key press, then blue, red, ...
const COLORS: [[u8; 3]; 2] = [[255, 0, 0], [0, 0, 255]];
const PRESSES: u32 = 6;

#[test]
fn a_screenshot_right_after_a_key_press_shows_a_gdi_window_changed() {
    screenshot_after_each_key_press_shows_the_new_color("gdi");
}

#[test]
fn a_screenshot_right_after_a_key_press_shows_a_swap_chain_window_changed() {
    screenshot_after_each_key_press_shows_the_new_color("swapchain");
}

#[test]
fn a_screenshot_right_after_a_key_press_shows_a_direct_composition_window_changed() {
    screenshot_after_each_key_press_shows_the_new_color("dcomp");
}

/// The fixture changes its whole client area to the next color on every key press and presents
/// it before `press_key` answers. Each screenshot taken right after must show the new color.
fn screenshot_after_each_key_press_shows_the_new_color(mode: &str) {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--present", mode]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_present_window(&mut helper);
    let hwnd = HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "foreground failed, so the test must fail rather than skip: {activated}");
    let first = center_color(&mut helper, &window_id);
    assert!(close_to(first, COLORS[0]), "{mode}: before any key the window shows {first:?}, not red");
    let mut seen = Vec::new();
    for press in 1..=PRESSES {
        assert_eq!(unsafe { GetForegroundWindow() }, hwnd, "refusing to send input: the fixture is not in the foreground");
        let pressed = helper.request("press_key", json!({ "window_id": window_id, "keys": ["space"] }));
        assert_eq!(pressed["ok"], true, "{pressed}");
        let started = Instant::now();
        let color = center_color(&mut helper, &window_id);
        let took = started.elapsed();
        let title = window_title(hwnd);
        assert!(title.ends_with(&format!("n={press}")), "{mode}: the fixture did not handle press {press}: {title}");
        seen.push((press, color, close_to(color, COLORS[(press % 2) as usize]), took));
    }
    drop(helper);
    drop(fixture);
    eprintln!("{mode}: {seen:?}");
    let stale: Vec<_> = seen.iter().filter(|(_, _, fresh, _)| !fresh).collect();
    assert!(stale.is_empty(), "{mode}: screenshots right after a key press showed the color from before it: {stale:?} (all: {seen:?})");
}

fn center_color(helper: &mut Helper, window_id: &str) -> [u8; 3] {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (width, height) = (info["result"]["width"].as_i64().unwrap(), info["result"]["height"].as_i64().unwrap());
    let shot = helper.request("capture_window", json!({ "window_id": window_id, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    let bytes = base64::engine::general_purpose::STANDARD.decode(shot["result"]["image_base64"].as_str().expect("image")).expect("base64");
    let image = image::load_from_memory(&bytes).expect("jpeg").to_rgb8();
    image.get_pixel(image.width() / 2, image.height() / 2).0
}

/// Within JPEG error of `expected`.
fn close_to(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| (i16::from(*a) - i16::from(e)).abs() < 48)
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn wait_for_present_window(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| window["title"].as_str().filter(|title| title.starts_with("CUMCP_PRESENT")).and_then(|_| window["window_id"].as_str().map(str::to_string)))
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the --present fixture was not listed");
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
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let value = self.incoming.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("response");
            if value.get("event").is_some() || value["id"].as_u64() != Some(id) {
                continue;
            }
            return value;
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
