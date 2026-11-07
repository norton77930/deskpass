use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn assert_build_id(id: &str) {
    assert_eq!(id.len(), 16, "build id {id:?}");
    assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "build id {id:?}");
}

fn flagged_build_id() -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
        .arg("--build-id")
        .stdin(Stdio::null())
        .output()
        .expect("run helper")
}

#[test]
fn build_id_flag_prints_the_id_and_exits() {
    let output = flagged_build_id();
    assert!(output.status.success(), "{:?}", output.status);
    assert_build_id(String::from_utf8_lossy(&output.stdout).trim());
}

#[test]
fn ping_reports_the_same_build_id() {
    let expected = String::from_utf8_lossy(&flagged_build_id().stdout).trim().to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
        .args(["--server-pid", &std::process::id().to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn helper");
    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, r#"{{"id":1,"method":"ping","params":{{}}}}"#).expect("send ping");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let reply = loop {
        let line = lines.next().expect("helper closed stdout").expect("read line");
        let value: serde_json::Value = serde_json::from_str(&line).expect("json line");
        if value["id"] == 1 {
            break value;
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(reply["result"]["version"], "17", "{reply}");
    assert_build_id(reply["result"]["build"].as_str().unwrap_or(""));
    assert_eq!(reply["result"]["build"], expected.as_str(), "{reply}");
}
