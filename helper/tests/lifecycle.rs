use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn helper_exits_when_the_server_pid_is_gone() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
        .args(["--server-pid", "4294967294"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn helper");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("wait").is_some() {
            return;
        }
        if started.elapsed() > Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("helper stayed alive after its server pid disappeared");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
