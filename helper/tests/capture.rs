use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn capture_fixture_jpeg_matches_the_window_size() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let mut window_id = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        window_id = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).map(|_| window["window_id"].as_str().unwrap_or("").to_string())
            })
        });
        if window_id.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let window_id = window_id.expect("fixture was not listed");
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    assert_eq!(info["ok"], true, "{info}");
    let width = info["result"]["width"].as_i64().unwrap();
    let height = info["result"]["height"].as_i64().unwrap();
    assert!(width > 0 && height > 0, "{info}");
    let shot = helper.request(
        "capture_window",
        json!({ "window_id": window_id, "target_width": width, "target_height": height }),
    );
    drop(helper);
    drop(fixture);
    assert_eq!(shot["ok"], true, "{shot}");
    assert_eq!(shot["result"]["original_width"].as_i64(), Some(width));
    assert_eq!(shot["result"]["original_height"].as_i64(), Some(height));
    let encoded = shot["result"]["image_base64"].as_str().expect("image");
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).expect("base64");
    let image = image::load_from_memory(&bytes).expect("jpeg");
    assert_eq!(image.width(), width as u32);
    assert_eq!(image.height(), height as u32);
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else {
            return false;
        };
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
    reader: BufReader<std::process::ChildStdout>,
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
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        Self { child, reader, next_id: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("response");
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
            if value.get("event").is_some() {
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
