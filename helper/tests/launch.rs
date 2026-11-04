use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[test]
fn launch_app_starts_the_fixture_and_returns_its_window() {
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let launched = helper.request("launch_app", json!({ "app": env!("CARGO_BIN_EXE_fixture-window") }));
    let pid = launched["result"]["pid"].as_u64();
    let title = launched["result"]["window"]["title"].as_str().unwrap_or("");
    if let Some(pid) = pid {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    }
    drop(helper);
    assert_eq!(launched["ok"], true, "{launched}");
    assert!(title.starts_with("CUMCP_FIXTURE"), "{launched}");
}

// The launcher cases run a private copy of the fixture, so no other test's fixture window shares
// its executable path and counts as "the same app". Variables reach the launched process through
// the helper, since `launch_app` cannot pass arguments.

#[test]
fn a_launcher_that_exits_nonzero_after_opening_a_window_returns_that_window() {
    let _interactive = interactive_lock::acquire();
    let fixture = FixtureCopy::new("trampoline");
    let mut helper = Helper::spawn_with_env(&[("CUMCP_FIXTURE_TRAMPOLINE", "1")]);
    let launched = helper.request("launch_app", json!({ "app": fixture.exe() }));
    assert_eq!(launched["ok"], true, "{launched}");
    let window = &launched["result"]["window"];
    assert!(window["title"].as_str().unwrap_or("").starts_with("CUMCP_FIXTURE"), "{launched}");
    assert!(window["app"].as_str().unwrap_or("").to_ascii_lowercase().ends_with(&fixture.file_name()), "{launched}");
    // The window belongs to the process the launcher started, not to the launcher.
    assert_ne!(window["pid"], launched["result"]["pid"], "{launched}");
}

#[test]
fn a_launcher_that_exits_nonzero_without_a_new_window_fails_even_if_the_app_already_had_one() {
    let _interactive = interactive_lock::acquire();
    let fixture = FixtureCopy::new("failing");
    let _existing = KillOnDrop(Command::new(fixture.exe()).spawn().expect("existing fixture window"));
    let mut helper = Helper::spawn_with_env(&[("CUMCP_FIXTURE_TRAMPOLINE", "1"), ("CUMCP_FIXTURE_TRAMPOLINE_NO_SPAWN", "1")]);
    helper.wait_for_window_of(&fixture.file_name());
    let launched = helper.request("launch_app", json!({ "app": fixture.exe() }));
    assert_eq!(launched["ok"], false, "{launched}");
    assert_eq!(launched["error"]["code"], "launch_failed", "{launched}");
    let message = launched["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("exit code: 1"), "the exit code is in the message: {launched}");
}

#[test]
fn a_launcher_that_exits_zero_without_a_new_window_reports_that_no_window_appeared() {
    let _interactive = interactive_lock::acquire();
    let fixture = FixtureCopy::new("quiet");
    let mut helper = Helper::spawn_with_env(&[("CUMCP_FIXTURE_TRAMPOLINE", "0"), ("CUMCP_FIXTURE_TRAMPOLINE_NO_SPAWN", "1")]);
    let launched = helper.request("launch_app", json!({ "app": fixture.exe() }));
    assert_eq!(launched["ok"], true, "{launched}");
    assert!(launched["result"]["window"].is_null(), "{launched}");
    assert!(launched["result"]["message"].as_str().unwrap_or("").contains("no new window appeared"), "{launched}");
}

#[test]
fn launch_app_returns_the_main_window_rather_than_an_owned_popup_in_front_of_it() {
    let _interactive = interactive_lock::acquire();
    let fixture = FixtureCopy::new("owned");
    let mut helper = Helper::spawn_with_env(&[("CUMCP_FIXTURE_OWNED_POPUP", "1")]);
    let launched = helper.request("launch_app", json!({ "app": fixture.exe() }));
    assert_eq!(launched["ok"], true, "{launched}");
    let window = &launched["result"]["window"];
    assert!(window["title"].as_str().unwrap_or("").starts_with("CUMCP_FIXTURE"), "not the untitled popup: {launched}");
    assert!(window["owner_window_id"].is_null(), "a main window has no owner: {launched}");
}

// Ticket 79: the main window is an unowned, visible, non-tool window of the launched app; a dialog
// shown before it is not the answer, unless nothing else appears.

#[test]
fn launch_app_waits_past_a_dialog_shown_first_for_the_main_window() {
    let _interactive = interactive_lock::acquire();
    let fixture = FixtureCopy::new("dialog-first");
    let mut helper = Helper::spawn_with_env(&[("CUMCP_FIXTURE_LAUNCH_DIALOG", "500")]);
    let launched = helper.request("launch_app", json!({ "app": fixture.exe() }));
    assert_eq!(launched["ok"], true, "{launched}");
    let window = &launched["result"]["window"];
    assert!(window["title"].as_str().unwrap_or("").starts_with("CUMCP_FIXTURE"), "not the dialog: {launched}");
    assert_eq!(launched["result"]["kind"], "main", "{launched}");
}

#[test]
fn launch_app_returns_a_dialog_marked_as_such_when_no_main_window_appears() {
    let _interactive = interactive_lock::acquire();
    let fixture = FixtureCopy::new("dialog-only");
    let mut helper = Helper::spawn_with_env(&[("CUMCP_FIXTURE_LAUNCH_DIALOG", "only")]);
    let started = Instant::now();
    let launched = helper.request("launch_app", json!({ "app": fixture.exe() }));
    assert_eq!(launched["ok"], true, "{launched}");
    assert_eq!(launched["result"]["window"]["title"], "CUMCP_LAUNCH_DIALOG", "{launched}");
    assert_eq!(launched["result"]["kind"], "dialog", "{launched}");
    // The main window was waited for as long as before (10 seconds).
    assert!(started.elapsed() >= Duration::from_secs(9), "returned after {:?}", started.elapsed());
}

const CALCULATOR: &str = "uwp:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";

#[test]
fn a_uwp_app_id_resolves_to_its_package_executable_and_launches_by_aumid() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let resolved = helper.request("resolve_app_id", json!({ "app_id": CALCULATOR }));
    if resolved["ok"] != true {
        assert_eq!(resolved["error"]["code"], "launch_failed", "{resolved}");
        eprintln!("SKIP Calculator is not installed: {resolved}");
        return;
    }
    let app = resolved["result"]["app"].as_str().expect("app").to_string();
    assert!(app.to_ascii_lowercase().ends_with(r"\calculatorapp.exe"), "{resolved}");
    assert!(std::path::Path::new(&app).is_absolute(), "{resolved}");
    assert_eq!(resolved["result"]["aumid"], "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", "{resolved}");
    if helper.has_window_of("calculatorapp.exe") {
        eprintln!("SKIP Calculator is already open; its window would not be new");
        return;
    }
    let launched = helper.request("launch_app", json!({ "app": app, "aumid": resolved["result"]["aumid"] }));
    let pid = launched["result"]["pid"].as_u64();
    let window = launched["result"]["window"].clone();
    let kind = launched["result"]["kind"].clone();
    if let Some(pid) = pid {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    }
    assert_eq!(launched["ok"], true, "{launched}");
    // Found the ticket-34 way: the frame host's window, described as the app inside it.
    assert!(window["app"].as_str().unwrap_or("").to_ascii_lowercase().ends_with(r"\calculatorapp.exe"), "{launched}");
    assert_eq!(kind, "main", "{launched}");
}

#[test]
fn launch_app_refuses_an_aumid_that_does_not_run_the_approved_executable() {
    // A private copy, so whatever starts by mistake is ended with it.
    let fixture = FixtureCopy::new("aumid-mismatch");
    let mut helper = Helper::spawn();
    let launched = helper.request("launch_app", json!({ "app": fixture.exe(), "aumid": "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App" }));
    assert_eq!(launched["ok"], false, "{launched}");
    assert_eq!(launched["error"]["code"], "launch_failed", "{launched}");
}

#[test]
fn an_app_id_that_names_nothing_installed_is_refused() {
    let mut helper = Helper::spawn();
    for app_id in ["uwp:Contoso.NotInstalled_0000000000000!App", "shell:Contoso.NotAnApp.79"] {
        let resolved = helper.request("resolve_app_id", json!({ "app_id": app_id }));
        assert_eq!(resolved["ok"], false, "{app_id}: {resolved}");
        assert_eq!(resolved["error"]["code"], "launch_failed", "{app_id}: {resolved}");
    }
    for app_id in ["notepad", "uwp:", r"shell:C:\Windows\System32\cmd.exe", "exe:relative.exe"] {
        let resolved = helper.request("resolve_app_id", json!({ "app_id": app_id }));
        assert_eq!(resolved["error"]["code"], "invalid_params", "{app_id}: {resolved}");
    }
}

#[test]
fn every_shell_app_id_listed_resolves_to_an_existing_executable_or_is_refused() {
    let mut helper = Helper::spawn();
    let listed = helper.request("list_apps", json!({}));
    let ids: Vec<String> = listed["result"]["apps"]
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(|app| app["app_id"].as_str().filter(|id| id.starts_with("shell:")).map(str::to_string))
        .take(10)
        .collect();
    let mut programs = 0;
    for app_id in &ids {
        let resolved = helper.request("resolve_app_id", json!({ "app_id": app_id }));
        if resolved["ok"] == true {
            programs += 1;
            let app = resolved["result"]["app"].as_str().expect("app");
            assert!(app.to_ascii_lowercase().ends_with(".exe") && std::path::Path::new(app).is_file(), "{app_id}: {resolved}");
            assert!(resolved["result"].get("aumid").is_none(), "a shell: app starts by its executable: {resolved}");
        } else {
            assert_eq!(resolved["error"]["code"], "launch_failed", "{app_id}: {resolved}");
        }
    }
    eprintln!("shell: app_ids resolved to a program: {programs} of {}", ids.len());
}

fn interactive_desktop() -> bool {
    use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else {
            return false;
        };
        let _ = CloseDesktop(desktop);
        true
    }
}

/// A copy of the fixture under a name no other test uses. Dropping it ends every process running
/// that name (the window the launcher opened is not a child the test holds) and deletes the copy.
struct FixtureCopy { dir: PathBuf, name: String }
impl FixtureCopy {
    fn new(label: &str) -> Self {
        let name = format!("cumcp-launch-{label}-{}.exe", std::process::id());
        let dir = std::env::temp_dir().join(format!("cumcp-launch-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::copy(env!("CARGO_BIN_EXE_fixture-window"), dir.join(&name)).expect("copy the fixture");
        Self { dir, name }
    }
    fn exe(&self) -> String { self.dir.join(&self.name).to_string_lossy().into_owned() }
    fn file_name(&self) -> String { self.name.to_ascii_lowercase() }
}
impl Drop for FixtureCopy {
    fn drop(&mut self) {
        let _ = Command::new("taskkill").args(["/F", "/IM", &self.name]).output();
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.dir).is_ok() { return; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

struct Helper { child: Child, reader: BufReader<std::process::ChildStdout>, next_id: u64 }
impl Helper {
    fn spawn() -> Self { Self::spawn_with_env(&[]) }
    fn spawn_with_env(vars: &[(&str, &str)]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper")).args(["--server-pid", &std::process::id().to_string()]).envs(vars.iter().copied()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("helper");
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        Self { child, reader, next_id: 1 }
    }
    fn wait_for_window_of(&mut self, file_name: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let listed = self.request("list_windows", json!({}));
            let found = listed["result"]["windows"].as_array().is_some_and(|windows| {
                windows.iter().any(|window| window["app"].as_str().is_some_and(|app| app.to_ascii_lowercase().ends_with(file_name)))
            });
            if found { return; }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the existing fixture window never appeared in list_windows");
    }
    fn has_window_of(&mut self, file_name: &str) -> bool {
        let listed = self.request("list_windows", json!({}));
        listed["result"]["windows"].as_array().is_some_and(|windows| {
            windows.iter().any(|window| window["app"].as_str().is_some_and(|app| app.to_ascii_lowercase().ends_with(file_name)))
        })
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
