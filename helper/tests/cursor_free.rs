//! Ticket 80: screenshots (and so zoom, which crops the same capture) do not contain the mouse
//! cursor. The cursor is parked on the middle of a uniformly colored window, then moved off it;
//! the pixels around that middle must match in the two screenshots.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SetCursorPos};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// Half the side of the square around the window's middle that is compared; larger than the
/// arrow cursor, which hangs about 12 x 20 pixels down and right of its hot spot.
const PATCH_RADIUS: u32 = 30;

#[test]
fn a_screenshot_does_not_contain_the_mouse_cursor() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--present", "gdi"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_present_window(&mut helper);
    let hwnd = HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.expect("window rect");
    let (middle_x, middle_y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);

    unsafe { SetCursorPos(middle_x, middle_y) }.expect("SetCursorPos onto the window");
    std::thread::sleep(Duration::from_millis(300));
    let cursor_over_it = screenshot(&mut helper, &window_id);
    unsafe { SetCursorPos(rect.right + 200, rect.bottom + 200) }.expect("SetCursorPos off the window");
    std::thread::sleep(Duration::from_millis(300));
    let cursor_elsewhere = screenshot(&mut helper, &window_id);
    drop(helper);
    drop(fixture);

    assert_eq!(cursor_over_it.dimensions(), cursor_elsewhere.dimensions());
    let (cx, cy) = (cursor_over_it.width() / 2, cursor_over_it.height() / 2);
    let mut worst = 0i16;
    for y in cy - PATCH_RADIUS..=cy + PATCH_RADIUS {
        for x in cx - PATCH_RADIUS..=cx + PATCH_RADIUS {
            let (a, b) = (cursor_over_it.get_pixel(x, y).0, cursor_elsewhere.get_pixel(x, y).0);
            worst = worst.max(a.iter().zip(b).map(|(a, b)| (i16::from(*a) - i16::from(b)).abs()).max().unwrap());
        }
    }
    assert!(worst < 48, "the cursor is in the screenshot: the pixels around the middle differ by up to {worst}");
}

fn screenshot(helper: &mut Helper, window_id: &str) -> image::RgbImage {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let (width, height) = (info["result"]["width"].as_i64().unwrap(), info["result"]["height"].as_i64().unwrap());
    let shot = helper.request("capture_window", json!({ "window_id": window_id, "target_width": width, "target_height": height }));
    assert_eq!(shot["ok"], true, "{shot}");
    let bytes = base64::engine::general_purpose::STANDARD.decode(shot["result"]["image_base64"].as_str().expect("image")).expect("base64");
    image::load_from_memory(&bytes).expect("jpeg").to_rgb8()
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
