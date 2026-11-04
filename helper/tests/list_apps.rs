//! `list_apps` (ticket 78) against the real helper. The Start Menu and UserAssist are replaced by
//! a temporary folder of shortcuts and a temporary HKCU key (`CUMCP_TEST_START_MENU_DIRS`,
//! `CUMCP_TEST_USERASSIST_KEY`), both removed afterwards; `shell:AppsFolder` is read as it is.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};

#[path = "interactive_lock.rs"]
mod interactive_lock;

// FILETIMEs of 2024-01-01, 2024-06-01 and 2024-09-01 (UTC midnight).
const JANUARY: u64 = 133_485_408_000_000_000;
const JUNE: u64 = 133_616_736_000_000_000;
const SEPTEMBER: u64 = 133_696_224_000_000_000;

#[test]
fn list_apps_merges_shortcuts_and_user_assist_into_one_entry_per_app() {
    let fixture = env!("CARGO_BIN_EXE_fixture-window");
    let answerer = env!("CARGO_BIN_EXE_prompt-answerer");
    let notepad = format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()));

    let scratch = Scratch::new();
    let programs = scratch.dir.join("Programs");
    std::fs::create_dir_all(programs.join("Sub")).expect("start menu folders");
    let tag = &scratch.tag;
    make_shortcut(&programs.join(format!("Fixture {tag}.lnk")), fixture);
    make_shortcut(&programs.join("Sub").join(format!("Answerer {tag}.lnk")), answerer);
    make_shortcut(&programs.join(format!("Pad {tag}.lnk")), &notepad);

    let count_key = format!(r"{}\{{4FA1B2C3-0000-4000-8000-000000000078}}\Count", scratch.key);
    // The path in upper case: UserAssist and the shortcut need not agree on case.
    add_user_assist(&count_key, &fixture.to_uppercase(), 5, JANUARY);
    add_user_assist(&count_key, answerer, 2, JUNE);
    add_user_assist(&count_key, r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe", 9, SEPTEMBER);
    // Records that are not programs are not read: a control entry and a shortcut's path.
    add_user_assist(&count_key, "UEME_CTLSESSION", 40, SEPTEMBER + 1);
    add_user_assist(&count_key, &format!(r"{}\Fixture {tag}.lnk", programs.display()), 77, SEPTEMBER + 2);

    let mut helper = Helper::spawn(&scratch);
    let response = helper.request("list_apps", json!({}));
    assert_eq!(response["ok"], true, "{response}");
    let apps = response["result"]["apps"].as_array().expect("apps").clone();
    let total = response["result"]["total"].as_u64().expect("total") as usize;
    assert!(apps.len() <= 200 && total >= apps.len(), "{} apps listed of {total}", apps.len());

    let by_path = |path: &str| -> Vec<(usize, &Value)> {
        let id = format!("exe:{path}").to_lowercase();
        apps.iter().enumerate().filter(|(_, app)| app["app_id"].as_str().is_some_and(|app_id| app_id.to_lowercase() == id)).collect()
    };

    let found = by_path(fixture);
    assert_eq!(found.len(), 1, "the shortcut's app appears once: {found:?}");
    let (fixture_at, fixture_app) = found[0];
    assert_eq!(fixture_app["name"], format!("Fixture {tag}"), "{fixture_app}");
    assert_eq!(fixture_app["kind"], "desktop");
    assert_eq!(fixture_app["use_count"], 5, "UserAssist merged onto the shortcut's app: {fixture_app}");
    assert_eq!(fixture_app["last_used"], "2024-01-01T00:00:00Z", "{fixture_app}");
    assert!(fixture_app["running"].is_boolean());

    let found = by_path(answerer);
    assert_eq!(found.len(), 1, "{found:?}");
    let (answerer_at, answerer_app) = found[0];
    assert_eq!(answerer_app["name"], format!("Answerer {tag}"));
    assert_eq!((answerer_app["use_count"].clone(), answerer_app["last_used"].clone()), (json!(2), json!("2024-06-01T00:00:00Z")));

    // Notepad may also be listed by the real shell:AppsFolder: still one entry, with the
    // KNOWNFOLDERID-form UserAssist record merged.
    let found = by_path(&notepad);
    assert_eq!(found.len(), 1, "{found:?}");
    let (notepad_at, notepad_app) = found[0];
    assert_eq!((notepad_app["use_count"].clone(), notepad_app["last_used"].clone()), (json!(9), json!("2024-09-01T00:00:00Z")), "{notepad_app}");

    // Newest first.
    assert!(notepad_at < answerer_at && answerer_at < fixture_at, "order {notepad_at}, {answerer_at}, {fixture_at}");

    // The control entry and the shortcut's own path made no entries and no counts.
    assert!(!apps.iter().any(|app| app["use_count"] == 40 || app["use_count"] == 77), "{apps:?}");
    assert!(!apps.iter().any(|app| app["app_id"].as_str().is_some_and(|id| id.contains("UEME_") || id.to_lowercase().ends_with(".lnk"))));

    // Every entry: the shape, no blocked flag (the server adds it), used ones first and newest first.
    let mut previous: Option<&str> = None;
    let mut unused_seen = false;
    for app in &apps {
        let id = app["app_id"].as_str().expect("app_id");
        assert!(id.starts_with("uwp:") || id.starts_with("exe:") || id.starts_with("shell:"), "{app}");
        assert!(app["name"].as_str().is_some_and(|name| !name.is_empty()), "{app}");
        assert_eq!(app["kind"], if id.starts_with("uwp:") { "uwp" } else { "desktop" }, "{app}");
        assert!(app["running"].is_boolean(), "{app}");
        assert!(app.get("blocked").is_none(), "{app}");
        match app.get("last_used") {
            Some(last_used) => {
                assert!(!unused_seen, "an app with no last use came before one with: {app}");
                let last_used = last_used.as_str().expect("ISO date");
                assert!(previous.is_none_or(|newer| newer >= last_used), "{last_used} listed after {previous:?}");
                previous = Some(last_used);
            }
            None => unused_seen = true,
        }
        if let Some(count) = app.get("use_count") {
            assert!(count.is_u64(), "{app}");
        }
    }

    // shell:AppsFolder lists the packaged apps (Calculator, Settings, ...), read as it is.
    assert!(
        apps.iter().any(|app| app["app_id"].as_str().is_some_and(|id| id.starts_with("uwp:") && id.contains('!'))),
        "no uwp: app among {total}: {:?}",
        apps.iter().map(|app| app["app_id"].clone()).collect::<Vec<_>>()
    );
}

/// Ticket 83: a `shell:` entry carries the program it runs, exactly what `resolve_app_id` gives
/// for it (and none when that cannot tell), so the server can mark a high-risk one `blocked`; a
/// shortcut to the Windows explorer.exe (a folder link, like the SDK's) is not an app.
#[test]
fn list_apps_gives_the_program_of_a_shell_entry_and_lists_no_explorer_shortcut() {
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let explorer = format!(r"{windows}\explorer.exe");
    let scratch = Scratch::new();
    let programs = scratch.dir.join("Programs");
    std::fs::create_dir_all(&programs).expect("start menu folder");
    make_shortcut(&programs.join(format!("Folder Link {}.lnk", scratch.tag)), &explorer);

    let mut helper = Helper::spawn(&scratch);
    let response = helper.request("list_apps", json!({}));
    assert_eq!(response["ok"], true, "{response}");
    let apps = response["result"]["apps"].as_array().expect("apps").clone();

    assert!(
        !apps.iter().any(|app| app["app_id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(&format!("exe:{explorer}")))),
        "explorer.exe is listed as an app: {:?}",
        apps.iter().filter(|app| app["app_id"].as_str().is_some_and(|id| id.to_lowercase().ends_with(r"\explorer.exe"))).collect::<Vec<_>>()
    );
    // File Explorer itself stays listed.
    assert!(
        apps.iter().any(|app| app["app_id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case("shell:Microsoft.Windows.Explorer"))),
        "File Explorer is not listed"
    );
    // Nor is a shell: shortcut to it (the SDK's own Start Menu entry is one).
    assert!(
        !apps.iter().any(|app| app["program"].as_str().is_some_and(|program| program.eq_ignore_ascii_case(&explorer))),
        "{:?}",
        apps.iter().filter(|app| app["program"].as_str().is_some_and(|program| program.eq_ignore_ascii_case(&explorer))).collect::<Vec<_>>()
    );

    let mut resolved = 0;
    for app in &apps {
        let id = app["app_id"].as_str().expect("app_id");
        if !id.starts_with("shell:") {
            assert!(app.get("program").is_none(), "only shell: entries carry a program: {app}");
            continue;
        }
        let resolve = helper.request("resolve_app_id", json!({ "app_id": id }));
        if resolve["ok"] == true {
            assert_eq!(app["program"], resolve["result"]["app"], "{app} vs {resolve}");
            resolved += 1;
        } else {
            assert!(app.get("program").is_none(), "{app} has a program, resolve_app_id has none: {resolve}");
        }
    }
    // Windows' own tools (Task Manager, Event Viewer, ...) are shell: entries that run a program.
    assert!(resolved > 0, "no shell: entry resolved to a program among {:?}", apps.iter().map(|app| app["app_id"].clone()).collect::<Vec<_>>());
}

#[test]
fn list_apps_marks_the_app_of_an_open_window_running() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = env!("CARGO_BIN_EXE_fixture-window");
    let scratch = Scratch::new();
    let programs = scratch.dir.join("Programs");
    std::fs::create_dir_all(&programs).expect("start menu folder");
    make_shortcut(&programs.join(format!("Running {}.lnk", scratch.tag)), fixture);

    let mut helper = Helper::spawn(&scratch);
    let id = format!("exe:{fixture}").to_lowercase();
    let find = |response: &Value| -> Option<Value> {
        response["result"]["apps"].as_array()?.iter().find(|app| app["app_id"].as_str().is_some_and(|app_id| app_id.to_lowercase() == id)).cloned()
    };
    let before = helper.request("list_apps", json!({}));
    let _ = find(&before).expect("the shortcut's app is listed");

    let _window = KillOnDrop(Command::new(fixture).spawn().expect("spawn fixture window"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_apps", json!({}));
        if find(&last).is_some_and(|app| app["running"] == true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the app of the open fixture window was not marked running: {:?}", find(&last));
}

/// A temporary Start Menu folder and a temporary HKCU key, both removed on drop (also when a test
/// panics).
struct Scratch {
    dir: PathBuf,
    /// The HKCU subkey that stands in for UserAssist.
    key: String,
    tag: String,
}

impl Scratch {
    fn new() -> Self {
        let tag = format!("{}x{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let dir = std::env::temp_dir().join(format!("cumcp-list-apps-{tag}"));
        Self { dir, key: format!(r"Software\ComputerUseMcpTest\{tag}"), tag }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = reg(&["delete", &format!(r"HKCU\{}", self.key), "/f"]);
        // The shared parent goes too once nothing else is under it.
        let parent = r"HKCU\Software\ComputerUseMcpTest";
        if reg(&["query", parent]).is_some_and(|listing| listing.lines().all(|line| line.trim().is_empty() || line.trim().eq_ignore_ascii_case(parent))) {
            let _ = reg(&["delete", parent, "/f"]);
        }
    }
}

fn reg(args: &[&str]) -> Option<String> {
    let output = Command::new("reg.exe").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn make_shortcut(link: &Path, target: &str) {
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:CUMCP_LNK); $s.TargetPath = $env:CUMCP_TARGET; $s.Save()",
        ])
        .env("CUMCP_LNK", link)
        .env("CUMCP_TARGET", target)
        .status()
        .expect("run powershell");
    assert!(status.success() && link.exists(), "could not create the shortcut {}", link.display());
}

/// A UserAssist value: the ROT13 of `name`, with a 72-byte record of the run count (offset 4) and
/// the last run (offset 60).
fn add_user_assist(count_key: &str, name: &str, count: u32, filetime: u64) {
    let mut record = [0u8; 72];
    record[4..8].copy_from_slice(&count.to_le_bytes());
    record[60..68].copy_from_slice(&filetime.to_le_bytes());
    let hex: String = record.iter().map(|byte| format!("{byte:02x}")).collect();
    let encoded: String = name
        .chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
            other => other,
        })
        .collect();
    reg(&["add", &format!(r"HKCU\{count_key}"), "/v", &encoded, "/t", "REG_BINARY", "/d", &hex, "/f"]).unwrap_or_else(|| panic!("could not write the UserAssist value for {name}"));
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
    fn spawn(scratch: &Scratch) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &std::process::id().to_string()])
            .env("CUMCP_TEST_START_MENU_DIRS", scratch.dir.join("Programs"))
            .env("CUMCP_TEST_USERASSIST_KEY", &scratch.key)
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
