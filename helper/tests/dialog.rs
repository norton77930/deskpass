use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindow, GetWindowThreadProcessId, GUITHREADINFO, GW_OWNER,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The fixture's "Dialog" button opens a modal dialog. While it is open, input aimed at the
/// fixture is refused with `dialog_open` naming the dialog; the dialog is listed with its owner
/// and is itself a normal target (screenshot, click, type, keys); Enter closes it, after which
/// the fixture takes input again.
#[test]
fn an_open_dialog_is_listed_operable_and_blocks_input_to_its_owner() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, app) = wait_for_window(&mut helper, "CUMCP_FIXTURE");

    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let (width, height) = size(&mut helper, &main);
    expect_foreground(&[&main], "before clicking the Dialog button");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "clicking the button that opens the dialog failed: {opened}");

    let (dialog, dialog_app) = wait_for_window(&mut helper, "CUMCP_DIALOG");
    assert_eq!(dialog_app, app, "the dialog's app identity differs from its owner's");
    let listed = helper.request("list_windows", json!({}));
    let entry = listed["result"]["windows"].as_array().and_then(|windows| windows.iter().find(|window| window["window_id"] == dialog.as_str())).cloned();
    assert_eq!(entry.as_ref().map(|entry| entry["owner_window_id"].clone()), Some(json!(main)), "dialog entry: {entry:?}");
    let main_entry = listed["result"]["windows"].as_array().and_then(|windows| windows.iter().find(|window| window["window_id"] == main.as_str())).cloned();
    assert_eq!(main_entry.as_ref().and_then(|entry| entry.get("owner_window_id")), None, "the owner is not owned: {main_entry:?}");
    let described = helper.request("get_window", json!({ "window_id": dialog }));
    assert_eq!(described["result"]["owner_window_id"], json!(main), "{described}");
    expect_foreground(&[&dialog], "after the dialog opened");

    // Activating the owner, as before any input, is refused the same way and names the dialog (F20).
    let refused_activation = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(refused_activation["error"]["code"], "dialog_open", "{refused_activation}");
    assert_eq!(refused_activation["error"]["data"]["window_id"], json!(dialog), "{refused_activation}");

    // Input aimed at the owner is refused and names the dialog; nothing reaches either window.
    let before = title(&mut helper, &main);
    expect_foreground(&[&main, &dialog], "before clicking the owner");
    let refused = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 62 / 360 }));
    assert_eq!(refused["error"]["code"], "dialog_open", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(dialog), "{refused}");
    assert!(refused["error"]["data"]["title"].as_str().is_some_and(|title| title.starts_with("CUMCP_DIALOG")), "{refused}");
    expect_foreground(&[&main, &dialog], "before typing into the owner");
    let refused_typing = helper.request("type_text", json!({ "window_id": main, "text": "no" }));
    assert_eq!(refused_typing["error"]["code"], "dialog_open", "{refused_typing}");
    assert_eq!(title(&mut helper, &main), before, "input reached the owner while its dialog was open");
    assert!(title(&mut helper, &dialog).contains("edit= |"), "input aimed at the owner reached the dialog");

    // The dialog is an ordinary target.
    let activated = helper.request("activate_window", json!({ "window_id": dialog }));
    assert_eq!(activated["ok"], true, "{activated}");
    let (dialog_width, dialog_height) = size(&mut helper, &dialog);
    let captured = helper.request("capture_window", json!({ "window_id": dialog, "target_width": dialog_width, "target_height": dialog_height }));
    assert_eq!(captured["ok"], true, "{captured}");
    assert!((captured["result"]["original_width"].as_i64().unwrap_or(0) - dialog_width).abs() <= 2, "{}", captured["result"]["original_width"]);
    let bytes = base64::engine::general_purpose::STANDARD.decode(captured["result"]["image_base64"].as_str().expect("image")).expect("base64");
    let picture = image::load_from_memory(&bytes).expect("jpeg").to_luma8();
    assert_eq!(i64::from(picture.width()), captured["result"]["image_width"].as_i64().unwrap_or(-1), "image width");
    let light = picture.pixels().filter(|pixel| pixel.0[0] > 200).count();
    let dark = picture.pixels().filter(|pixel| pixel.0[0] < 80).count();
    assert!(light > 0 && dark > 0, "the dialog screenshot has no content ({light} light, {dark} dark pixels)");
    expect_foreground(&[&dialog], "before clicking the dialog");
    let clicked = helper.request("click", json!({ "window_id": dialog, "x": dialog_width * 85 / 100, "y": dialog_height / 2 }));
    assert_eq!(clicked["ok"], true, "{clicked}");
    let dialog_title = title(&mut helper, &dialog);
    assert!(dialog_title.contains("dlg:click"), "the click did not reach the dialog: {clicked}; {}", evidence(&mut helper, &dialog_title, &main));
    expect_foreground(&[&dialog], "before typing into the dialog");
    let typed = helper.request("type_text", json!({ "window_id": dialog, "text": "dlg15" }));
    assert_eq!(typed["ok"], true, "{typed}");
    let dialog_title = title(&mut helper, &dialog);
    assert!(dialog_title.contains("edit=dlg15 |"), "the text did not reach the dialog: {typed}; {}", evidence(&mut helper, &dialog_title, &main));
    expect_foreground(&[&dialog], "before pressing Enter in the dialog");
    let pressed = helper.request("press_key", json!({ "window_id": dialog, "keys": ["return"] }));
    assert_eq!(pressed["ok"], true, "Enter that closes the dialog is not a failure: {pressed}");

    // Enter closed the dialog; the owner takes input again.
    wait_until(|| title(&mut helper, &main).contains("dialog:closed:1"), "the dialog did not close with IDOK");
    let still_listed = helper.request("list_windows", json!({}))["result"]["windows"]
        .as_array()
        .is_some_and(|windows| windows.iter().any(|window| window["window_id"] == dialog.as_str()));
    assert!(!still_listed, "the closed dialog is still listed");
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "{activated}");
    expect_foreground(&[&main], "before clicking the fixture again");
    let clicked = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 62 / 360 }));
    assert_eq!(clicked["ok"], true, "the fixture is not operable after its dialog closed: {clicked}");
    assert!(title(&mut helper, &main).contains("click:left"), "the click after the dialog closed was not recorded");
    drop(helper);
    drop(fixture);
}

/// A dialog with no title, like Paint's or Office's save prompt, is still a target: `dialog_open`
/// from its owner names it, it is listed with its owner, and it can be captured and take a key
/// press. A visible window with no title and no owner stays unlisted, and is not reported as
/// closed while it is open.
#[test]
fn an_untitled_dialog_is_listed_and_operable_but_an_untitled_window_without_an_owner_is_not() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).arg("--untitled-dialog").spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, app) = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    // The fixture creates this window after its main window, so it may not exist yet.
    let mut stray = String::new();
    wait_until(
        || match unsafe { FindWindowW(w!("CUMCP_STRAY"), None) } {
            Ok(found) => {
                stray = (found.0 as isize).to_string();
                true
            }
            Err(_) => false,
        },
        "the fixture's untitled window without an owner was not created",
    );

    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let (width, height) = size(&mut helper, &main);
    expect_foreground(&[&main], "before clicking the Dialog button");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "clicking the button that opens the dialog failed: {opened}");
    let mut dialog = String::new();
    wait_until(
        || unsafe {
            let foreground = GetForegroundWindow();
            dialog = (foreground.0 as isize).to_string();
            GetWindow(foreground, GW_OWNER).is_ok_and(|owner| owner == hwnd(&main))
        },
        "the untitled dialog did not take the foreground",
    );

    // Input aimed at the owner names the dialog, and that id is a target.
    expect_foreground(&[&main, &dialog], "before clicking the owner");
    let refused = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 62 / 360 }));
    assert_eq!(refused["error"]["code"], "dialog_open", "{refused}");
    assert_eq!(refused["error"]["data"]["window_id"], json!(dialog), "{refused}");
    let described = helper.request("get_window", json!({ "window_id": dialog }));
    assert_eq!(described["ok"], true, "the dialog named by dialog_open is not a target: {described}");
    assert_eq!(described["result"]["owner_window_id"], json!(main), "{described}");
    assert_eq!(described["result"]["title"], "", "{described}");
    assert_eq!(described["result"]["app"], json!(app), "the dialog's app identity differs from its owner's: {described}");
    let listed = helper.request("list_windows", json!({}));
    let windows = listed["result"]["windows"].as_array().cloned().unwrap_or_default();
    let entry = windows.iter().find(|window| window["window_id"] == dialog.as_str());
    assert_eq!(entry.map(|entry| entry["owner_window_id"].clone()), Some(json!(main)), "the untitled dialog is not listed with its owner: {listed}");

    // It can be captured and takes a key press: Esc closes it with IDCANCEL.
    let (dialog_width, dialog_height) = size(&mut helper, &dialog);
    let captured = helper.request("capture_window", json!({ "window_id": dialog, "target_width": dialog_width, "target_height": dialog_height }));
    assert_eq!(captured["ok"], true, "{captured}");
    assert!((captured["result"]["original_width"].as_i64().unwrap_or(0) - dialog_width).abs() <= 2, "{}", captured["result"]["original_width"]);
    expect_foreground(&[&dialog], "before pressing Esc in the dialog");
    let pressed = helper.request("press_key", json!({ "window_id": dialog, "keys": ["escape"] }));
    assert_eq!(pressed["ok"], true, "Esc that closes the dialog is not a failure: {pressed}");
    wait_until(|| title(&mut helper, &main).contains("dialog:closed:2"), "Esc did not reach the untitled dialog");

    // The untitled window without an owner is open but not listed, and not called closed.
    let listed = helper.request("list_windows", json!({}));
    let windows = listed["result"]["windows"].as_array().cloned().unwrap_or_default();
    assert!(!windows.iter().any(|window| window["window_id"] == stray.as_str()), "the untitled window without an owner is listed: {listed}");
    let refused = helper.request("get_window", json!({ "window_id": stray }));
    assert_eq!(refused["error"]["code"], "window_not_operable", "{refused}");
    assert!(!refused["error"]["message"].as_str().unwrap_or("").contains("no longer open"), "{refused}");
    drop(helper);
    drop(fixture);
}

/// Typing whose Enter closes the dialog it targets ends there: the rest of the text is not sent
/// to the window that takes the foreground next (here the dialog's owner, whose edit has the
/// focus on the same thread). Text whose last character closes the dialog still succeeds.
#[test]
fn typing_stops_where_it_closes_the_dialog() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let (main, _) = wait_for_window(&mut helper, "CUMCP_FIXTURE");
    let activated = helper.request("activate_window", json!({ "window_id": main }));
    assert_eq!(activated["ok"], true, "could not foreground the fixture: {activated}");
    let (width, height) = size(&mut helper, &main);
    let owner_edit = "edit= | len=0 |";
    assert!(title(&mut helper, &main).contains(owner_edit), "the owner's edit is not empty to begin with");

    // The last character closes the dialog: success.
    let dialog = open_dialog(&mut helper, &main, width, height);
    expect_foreground(&[&dialog], "before typing text that ends with Enter");
    let typed = helper.request("type_text", json!({ "window_id": dialog, "text": "ab\n" }));
    assert_eq!(typed["ok"], true, "Enter that closes the dialog as the last character is not a failure: {typed}");
    wait_until(|| title(&mut helper, &main).contains("dialog:closed:1"), "the typed Enter did not close the dialog");

    // Characters after the one that closed the dialog are not sent anywhere.
    let dialog = open_dialog(&mut helper, &main, width, height);
    let owner_before = title(&mut helper, &main);
    expect_foreground(&[&dialog], "before typing text with Enter in the middle");
    let typed = helper.request("type_text", json!({ "window_id": dialog, "text": "ab\ncd" }));
    std::thread::sleep(Duration::from_millis(500));
    let owner = title(&mut helper, &main);
    let foreground = unsafe { GetForegroundWindow() };
    eprintln!("after the dialog closed: foreground {:?} (owner {main}), owner {owner:?}", foreground.0);
    // The fixture records characters its window receives as `char:` events, and its edit's text
    // is in the title: the only change is the dialog having closed.
    assert_eq!(owner, format!("{owner_before},dialog:closed:1"), "text typed after the dialog closed reached its owner: {typed}");
    assert!(owner.contains(owner_edit), "{owner}");
    assert_eq!(typed["ok"], false, "type_text reported success although the dialog closed with text left: {typed}");
    assert_eq!(typed["error"]["code"], "input_failed", "{typed}");
    let message = typed["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("closed after character 3 of 5"), "the error does not say where the dialog closed: {message}");
    drop(helper);
    drop(fixture);
}

/// Clicks the fixture's Dialog button and returns the dialog that opened.
fn open_dialog(helper: &mut Helper, main: &str, width: i64, height: i64) -> String {
    expect_foreground(&[&main.to_string()], "before clicking the Dialog button");
    let opened = helper.request("click", json!({ "window_id": main, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "clicking the button that opens the dialog failed: {opened}");
    wait_for_window(helper, "CUMCP_DIALOG").0
}

/// What the windows looked like when an assertion failed: the dialog title that was read, the
/// dialog title again half a second later (late vs lost), the owner's title, and the foreground
/// window with its thread's focus window.
fn evidence(helper: &mut Helper, dialog_title: &str, main: &str) -> String {
    std::thread::sleep(Duration::from_millis(500));
    let later = helper.request("list_windows", json!({}))["result"]["windows"]
        .as_array()
        .and_then(|windows| windows.iter().find(|window| window["title"].as_str().is_some_and(|title| title.starts_with("CUMCP_DIALOG"))).map(|window| window["title"].clone()));
    let owner = title(helper, main);
    let (foreground, focus, class) = unsafe {
        let foreground = GetForegroundWindow();
        let mut gui = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        let focus = if GetGUIThreadInfo(GetWindowThreadProcessId(foreground, None), &mut gui).is_ok() { gui.hwndFocus } else { HWND::default() };
        let mut name = [0u16; 64];
        let length = GetClassNameW(focus, &mut name);
        (foreground, focus, String::from_utf16_lossy(&name[..length.max(0) as usize]))
    };
    format!("dialog title {dialog_title:?}, 500 ms later {later:?}, owner {owner:?}, foreground {:?}, focus {:?} ({class})", foreground.0, focus.0)
}

/// Fails, without sending anything, unless the foreground window is one of `expected`.
fn expect_foreground(expected: &[&String], when: &str) {
    let foreground = unsafe { GetForegroundWindow() };
    assert!(
        expected.iter().any(|id| hwnd(id) == foreground),
        "{when}: the foreground window {:?} is not one of {expected:?}, so no input is sent",
        foreground.0
    );
}

fn hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

fn size(helper: &mut Helper, window_id: &str) -> (i64, i64) {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64();
    let height = info["result"]["height"].as_i64();
    width.zip(height).unwrap_or_else(|| panic!("get_window failed: {info}"))
}

fn title(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

fn wait_until(mut condition: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{message}");
}

/// The window id and app of the first listed window whose title starts with `prefix`.
fn wait_for_window(helper: &mut Helper, prefix: &str) -> (String, String) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = helper.request("list_windows", json!({}));
        if let Some(found) = last["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"]
                    .as_str()
                    .filter(|title| title.starts_with(prefix))
                    .and_then(|_| Some((window["window_id"].as_str()?.to_string(), window["app"].as_str()?.to_string())))
            })
        }) {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{prefix} was not listed: {last}");
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
