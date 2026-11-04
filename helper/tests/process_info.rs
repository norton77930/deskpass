//! `process_info` answers the server's lock check: whether a pid runs, and since when.
//! Not interactive: it sends no input.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

#[test]
fn process_info_reports_running_exited_and_missing_processes() {
    let mut helper = Helper::spawn(std::process::id());

    let now_ms = unix_ms(SystemTime::now());
    let own = helper.request("process_info", json!({ "pid": std::process::id() }));
    assert_eq!(own["result"]["alive"], true, "{own}");
    let own_start = own["result"]["started_ms"].as_i64().unwrap_or_else(|| panic!("no start time for this test process: {own}"));
    assert!(own_start <= now_ms && own_start > now_ms - 3_600_000, "this test process started at {own_start}, now is {now_ms}");

    let before_ms = unix_ms(SystemTime::now());
    let mut running = Helper::spawn(std::process::id());
    let after_ms = unix_ms(SystemTime::now());
    let child = helper.request("process_info", json!({ "pid": running.child.id() }));
    assert_eq!(child["result"]["alive"], true, "{child}");
    let child_start = child["result"]["started_ms"].as_i64().unwrap_or_else(|| panic!("no start time for a child: {child}"));
    assert!(child_start >= before_ms - 50 && child_start <= after_ms + 50, "child started at {child_start}, spawned between {before_ms} and {after_ms}");
    let _ = running.child.kill();
    let _ = running.child.wait();
    // The Child still holds a handle, so the process object exists but has exited.
    let exited = helper.request("process_info", json!({ "pid": running.child.id() }));
    assert_eq!(exited, json!({ "id": exited["id"], "ok": true, "result": { "alive": false } }));

    let missing = helper.request("process_info", json!({ "pid": 4294967292u32 }));
    assert_eq!(missing["result"], json!({ "alive": false }), "{missing}");

    for pid in [json!(0), json!(-1), json!("12"), json!(1.5), json!(4294967296u64)] {
        let refused = helper.request("process_info", json!({ "pid": pid }));
        assert_eq!(refused["error"]["code"], "invalid_params", "pid {pid}: {refused}");
    }
}

fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).expect("clock").as_millis() as i64
}

struct Helper {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Helper {
    fn spawn(server_pid: u32) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &server_pid.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("helper");
        let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        assert!(line.contains("ready") && Instant::now() < deadline, "{line}");
        Self { child, reader, next_id: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
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
