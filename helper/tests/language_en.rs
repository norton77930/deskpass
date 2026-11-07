//! Open-source ticket 03: a helper started in English (`--lang en`) shows English only. This reads
//! what a person (or a screen reader) is shown, through UI Automation: the title and the buttons of
//! each dialog (the approval with its three buttons, the clipboard question, the "continue?"
//! question after Esc, the settings page's confirm window) and the text of the corner notice for a
//! background and a foreground delivery, and finds no CJK in any of them.
//!
//! It opens real dialogs and a notice on the desktop, so it needs one: it skips itself when
//! `DESKPASS_NO_DESKTOP` is set or there is no interactive desktop, like the other desktop tests
//! (CI only compiles it). The helper's string tables are tested without a desktop in
//! `helper/src/strings.rs`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ButtonControlTypeId};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, IsWindowVisible};

#[path = "interactive_lock.rs"]
mod interactive_lock;

const ASK_CLASS: PCWSTR = w!("DESKPASS_ASK_USER");
const CONFIRM_CLASS: &str = "DESKPASS_CONFIRM_CHANGE";
const APP: &str = "Demo App";

/// What UI Automation shows of a window: its title, its buttons' names and every other name in it.
struct Shown {
    title: String,
    buttons: Vec<String>,
    texts: Vec<String>,
}

/// Each dialog in English: its title, its buttons (the refusal last) and a phrase of its text.
#[test]
fn the_dialogs_are_in_english() {
    let Some(_lock) = interactive() else { return };
    let mut helper = Helper::spawn(&["--lang", "en"]);
    let pid = helper.pid();

    let approve = json!({ "kind": "approve_app", "app_name": APP, "app_path": "C:\\Apps\\demo.exe", "timeout_ms": 30_000, "allow_always": true });
    let clipboard = json!({ "kind": "read_clipboard", "timeout_ms": 30_000 });
    let resume = json!({ "kind": "continue", "timeout_ms": 30_000 });
    let confirm = json!({ "app_name": APP, "current_state": "Blocked (set by you)", "restored_state": "Normal: ask every time", "timeout_ms": 30_000 });
    let system_key = json!({ "kind": "system_key", "app_name": APP, "app_path": "C:\\Apps\\demo.exe", "combination": "alt+f4", "timeout_ms": 30_000 });
    let cases: [(&str, Value, PCWSTR, &str, &[&str], &str); 5] = [
        ("ask_user", approve, ASK_CLASS, "Deskpass: Approve app", &["Allow once", "Always allow", "Deny"], "Allow the AI to operate this app?"),
        ("ask_user", clipboard, ASK_CLASS, "Deskpass: Read clipboard", &["Allow once", "Deny"], "read the text on your clipboard"),
        ("ask_user", system_key, ASK_CLASS, "Deskpass: Allow key combination", &["Allow once", "Deny"], "press alt+f4 in this app"),
        ("ask_user", resume, ASK_CLASS, "Deskpass: Continue?", &["Continue", "Don't continue"], "You pressed Esc to stop Deskpass earlier. Let the AI continue?"),
        ("confirm_change", confirm, w!("DESKPASS_CONFIRM_CHANGE"), "Deskpass confirmation", &["Allow", "Cancel"], "The settings page asks to loosen a limit"),
    ];
    for (method, params, class, title, buttons, phrase) in cases {
        let asked = helper.send(method, params);
        let window = wait_for_window(pid, class);
        let shown = read_through_uia(window);
        assert_eq!(shown.title, title, "title of the {title} window");
        let mut names = shown.buttons.clone();
        names.sort();
        let mut expected: Vec<String> = buttons.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(names, expected, "buttons of the {title} window: {:?}", shown.buttons);
        assert!(shown.texts.iter().any(|text| text.contains(phrase)), "the {title} window does not say {phrase:?}: {:?}", shown.texts);
        for name in std::iter::once(&shown.title).chain(&shown.buttons).chain(&shown.texts) {
            assert!(!has_cjk(name), "CJK in the English {title} window: {name:?}");
        }
        // The refusal ends it: the default button (Enter would too, but needs the foreground).
        let pressed = Command::new(env!("CARGO_BIN_EXE_prompt-answerer"))
            .args(["--pid", &pid.to_string(), "--press", "deny", "--wait-ms", "10000", "--class", if method == "confirm_change" { CONFIRM_CLASS } else { "DESKPASS_ASK_USER" }])
            .output()
            .expect("prompt-answerer");
        assert!(pressed.status.success(), "prompt-answerer failed: {}", String::from_utf8_lossy(&pressed.stderr));
        let answered = helper.wait(asked, Duration::from_secs(10));
        assert_eq!(answered["ok"], true, "{answered}");
    }
}

/// The corner notice names the app and the stop key in English, for a background and a foreground delivery.
#[test]
fn the_corner_notice_is_in_english() {
    let Some(_lock) = interactive() else { return };
    let mut helper = Helper::spawn(&["--lang", "en"]);
    let pid = helper.pid();
    let _fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let target = wait_for_fixture(&mut helper);

    for (delivery, expected) in [
        ("background", format!("AI is operating {APP} · Ctrl+Alt+Esc to stop")),
        ("foreground", format!("AI is operating {APP} · Esc to stop")),
    ] {
        let shown = helper.request("overlay_show", json!({ "window_id": target, "app_name": APP, "delivery_mode": delivery }));
        assert_eq!(shown["ok"], true, "{shown}");
        let notice = wait_for_window(pid, w!("DESKPASS_NOTICE"));
        let mut read = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            read = read_through_uia(notice).title;
            if read == expected {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(read, expected, "the {delivery} notice read through UI Automation");
        assert!(!has_cjk(&read), "CJK in the English notice: {read:?}");
    }
    let _ = helper.request("overlay_hide", json!({}));
}

/// The window's name, its buttons' names and the other names in it, as UI Automation gives them.
fn read_through_uia(window: HWND) -> Shown {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).expect("UI Automation");
        let root = automation.ElementFromHandle(window).expect("the window as an element");
        let title = root.CurrentName().map(|name| name.to_string()).unwrap_or_default();
        let found = root.FindAll(TreeScope_Descendants, &automation.CreateTrueCondition().expect("condition")).expect("descendants");
        let (mut buttons, mut texts) = (Vec::new(), Vec::new());
        for at in 0..found.Length().unwrap_or(0) {
            let Ok(element) = found.GetElement(at) else { continue };
            let name = element.CurrentName().map(|name| name.to_string()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            // Only Deskpass's own controls, which are child windows (STATIC, BUTTON). The title
            // bar, its system menu and its Close button are drawn by the window frame, have no
            // window of their own and are named in the Windows display language, not Deskpass's.
            if element.CurrentNativeWindowHandle().map_or(true, |handle| handle.is_invalid()) {
                continue;
            }
            if element.CurrentControlType().is_ok_and(|kind| kind == UIA_ButtonControlTypeId) {
                buttons.push(name);
            } else {
                texts.push(name);
            }
        }
        Shown { title, buttons, texts }
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| matches!(c as u32, 0x3000..=0x303F | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xFF00..=0xFFEF))
}

/// A visible top-level window of `class` owned by process `pid`; panics when none appears.
fn wait_for_window(pid: u32, class: PCWSTR) -> HWND {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut after: Option<HWND> = None;
        while let Ok(window) = unsafe { FindWindowExW(None, after, class, None) } {
            let mut owner = 0u32;
            unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };
            if owner == pid && unsafe { IsWindowVisible(window) }.as_bool() {
                return window;
            }
            after = Some(window);
        }
        assert!(Instant::now() < deadline, "no window of the helper appeared");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                let title = window["title"].as_str().unwrap_or("");
                title.starts_with("CUMCP_FIXTURE").then(|| window["window_id"].as_str().unwrap_or("").to_string())
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("the fixture was not listed");
}

/// The interactive-desktop lock, or `None` (the test skips): `DESKPASS_NO_DESKTOP` set, or no desktop to show windows on.
fn interactive() -> Option<interactive_lock::InteractiveLock> {
    if std::env::var_os("DESKPASS_NO_DESKTOP").is_some() {
        eprintln!("SKIP interactive: DESKPASS_NO_DESKTOP");
        return None;
    }
    let Ok(desktop) = (unsafe { OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) }) else {
        eprintln!("SKIP interactive: no desktop");
        return None;
    };
    let _ = unsafe { CloseDesktop(desktop) };
    Some(interactive_lock::acquire())
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A helper whose responses may arrive out of order: a question's answer comes when it is answered.
struct Helper {
    child: Child,
    lines: Receiver<Value>,
    responses: HashMap<u64, Value>,
    next_id: u64,
}

impl Helper {
    fn spawn(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(args)
            .args(["--server-pid", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("helper");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let ready = lines.recv_timeout(Duration::from_secs(10)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        Self { child, lines, responses: HashMap::new(), next_id: 1 }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
        id
    }

    fn wait(&mut self, id: u64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = self.responses.remove(&id) {
                return value;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) => {
                    if let Some(got) = value.get("id").and_then(Value::as_u64) {
                        self.responses.insert(got, value);
                    }
                }
                Err(_) => panic!("no response to request {id} within {timeout:?}"),
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.wait(id, Duration::from_secs(15))
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
