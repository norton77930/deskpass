use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn list_windows_includes_the_fixture_window() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();

    let fixture = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_fixture-window"))
            .spawn()
            .expect("spawn fixture window"),
    );
    let mut helper = Helper::spawn();
    let mut found = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        let response = helper.request("list_windows", json!({}));
        last = response.to_string();
        let windows = response
            .get("result")
            .and_then(|result| result.get("windows"))
            .and_then(|windows| windows.as_array())
            .cloned()
            .unwrap_or_default();
        if windows.iter().any(|window| {
            window
                .get("title")
                .and_then(|title| title.as_str())
                .is_some_and(|title| title.starts_with("CUMCP_FIXTURE"))
        }) {
            found = true;
            let window = windows
                .iter()
                .find(|window| {
                    window
                        .get("title")
                        .and_then(|title| title.as_str())
                        .is_some_and(|title| title.starts_with("CUMCP_FIXTURE"))
                })
                .expect("fixture");
            assert!(window.get("window_id").and_then(|value| value.as_str()).is_some_and(|id| !id.is_empty()));
            assert!(window.get("app").and_then(|value| value.as_str()).is_some_and(|app| app.to_ascii_lowercase().ends_with("fixture-window.exe")));
            assert!(window.get("app_name").and_then(|value| value.as_str()).is_some());
            assert_eq!(window.get("minimized").and_then(|value| value.as_bool()), Some(false));
            assert!(window.get("elevated").and_then(|value| value.as_bool()).is_some());
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(helper);
    drop(fixture);
    assert!(found, "list_windows did not include CUMCP_FIXTURE. last response: {last}");
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
            .expect("spawn helper");
        let stdout = child.stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        let ready: Value = serde_json::from_str(&line).expect("ready json");
        assert_eq!(ready.get("event").and_then(|event| event.as_str()), Some("ready"));
        Self { child, reader, next_id: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).expect("write");
        stdin.flush().expect("flush");
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read");
        serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }))
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
