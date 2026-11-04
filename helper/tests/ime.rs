use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, UnloadKeyboardLayout, HKL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, PostMessageW, SendMessageW, WM_IME_CONTROL, WM_INPUTLANGCHANGEREQUEST,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

const IMC_GETOPENSTATUS: usize = 5;

#[test]
fn press_key_and_type_text_land_under_the_current_ime_and_restore_it() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    require_foreground(hwnd);
    let target_layout = layout_of(hwnd);
    let ime_open = ime_is_open(hwnd);
    eprintln!("ime-baseline layout={target_layout:#x} ime_open={ime_open} list={:?}", restore.list.iter().map(|item| format!("{item:#x}")).collect::<Vec<_>>());
    if target_layout == 0x04040404 {
        assert!(ime_open, "zh-TW is the fixture layout but the IME is closed");
    }
    click_edit(&mut helper, &window_id);
    require_foreground(hwnd);
    let typed_a = helper.request("press_key", json!({ "window_id": window_id, "keys": ["a"] }));
    assert_eq!(typed_a["ok"], true, "{typed_a}");
    assert_eq!(edit_of(&title_of(&mut helper, &window_id)), "a", "press_key a");
    replace(&mut helper, &window_id, hwnd, "你好");
    replace(&mut helper, &window_id, hwnd, "é");
    replace(&mut helper, &window_id, hwnd, "abc你好é");
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    assert_eq!(layout_value(restore.foreground_thread), restore.foreground_layout, "foreground thread layout changed");
    assert_eq!(layout_of(hwnd), target_layout, "fixture thread layout changed");
    assert_eq!(ime_is_open(hwnd), ime_open, "IME open status changed");
    drop(helper);
    drop(fixture);
}

#[test]
fn stopping_mid_type_restores_the_keyboard_layout() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    require_foreground(hwnd);
    let target_layout = layout_of(hwnd);
    let ime_open = ime_is_open(hwnd);
    click_edit(&mut helper, &window_id);
    require_foreground(hwnd);
    let text = "你".repeat(500);
    helper.send("type_text", json!({ "window_id": window_id, "text": text }));
    // Stop only once the fixture thread is on the English layout and text is arriving, so the
    // stop lands mid-type and the restore on the interrupted path is what gets checked.
    wait_for(Duration::from_secs(5), "typing under the English layout never started", || {
        layout_of(hwnd) & 0xFFFF == 0x0409 && len_of(&window_title(hwnd)) > 0
    });
    helper.send("debug_stop", json!({}));
    let typed = helper.recv();
    assert_eq!(typed["ok"], false, "type_text finished instead of stopping: {typed}");
    assert_eq!(typed["error"]["code"], "interrupted", "{typed}");
    let title = title_of(&mut helper, &window_id);
    let length = len_of(&title);
    assert!(length < 500, "fixture received the whole string ({length}): {title}");
    let stopped = helper.recv();
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    assert_eq!(layout_value(restore.foreground_thread), restore.foreground_layout, "foreground thread layout changed");
    assert_eq!(layout_of(hwnd), target_layout, "fixture thread layout changed");
    assert_eq!(ime_is_open(hwnd), ime_open, "IME open status changed");
    drop(helper);
    drop(fixture);
}

/// Typing whose Enter closes the target (here the fixture's modal dialog, whose default button
/// is OK) still puts the target thread back on its layout: the dialog is gone when the switch is
/// undone, so the request goes to a window of the same thread that is still open (its owner).
#[test]
fn typing_that_closes_the_dialog_restores_the_keyboard_layout() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    require_foreground(hwnd);
    let target_layout = layout_of(hwnd);
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let opened = helper.request("click", json!({ "window_id": window_id, "x": width * 70 / 480, "y": height * 314 / 360 }));
    assert_eq!(opened["ok"], true, "clicking the Dialog button failed: {opened}");
    let dialog_id = wait_for_window(&mut helper, "CUMCP_DIALOG");
    let dialog = parse_hwnd(&dialog_id);

    require_foreground(dialog);
    let typed = helper.request("type_text", json!({ "window_id": dialog_id, "text": "中文
" }));
    assert_eq!(typed["ok"], true, "Enter that closes the dialog is not a failure: {typed}");
    wait_for(Duration::from_secs(2), "the typed Enter did not close the dialog", || window_title(hwnd).contains("dialog:closed:1"));
    assert_eq!(layout_of(hwnd), target_layout, "fixture thread layout was not restored after its dialog closed");
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    assert_eq!(layout_value(restore.foreground_thread), restore.foreground_layout, "foreground thread layout changed");
    drop(helper);
    drop(fixture);
}

/// Ticket 52 (F32): typing whose last character closes the target's only window while its thread
/// lives on without any window. The thread's layout cannot be put back (a layout request needs a
/// window of that thread), so the helper says so on stderr, naming the thread and the layout it
/// was on; it still unloads the English layout it loaded, and no other thread's layout changes.
#[test]
fn typing_that_closes_the_threads_last_window_reports_the_layout_it_could_not_restore() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let restore = LayoutGuard::capture();
    let mut fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--close-on-char", "x"]).spawn().expect("fixture"));
    let mut helper = Helper::spawn();
    let window_id = wait_for_fixture(&mut helper);
    let hwnd = parse_hwnd(&window_id);
    let activated = helper.request("activate_window", json!({ "window_id": window_id }));
    assert_eq!(activated["ok"], true, "{activated}");
    require_foreground(hwnd);
    let thread = unsafe { GetWindowThreadProcessId(hwnd, None) };
    let target_layout = layout_of(hwnd);
    click_edit(&mut helper, &window_id);

    require_foreground(hwnd);
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": "中x" }));
    assert_eq!(typed["ok"], true, "every character was read, so the target closing over the last one is no failure: {typed}");
    assert!(fixture.0.try_wait().expect("fixture status").is_none(), "the fixture exited, so its thread was gone rather than left without a window");
    let expected = format!("keyboard layout: thread {thread} has no window left to restore {target_layout:#x}");
    wait_for(Duration::from_secs(2), &format!("the helper did not report the layout it could not restore ({expected})"), || helper.stderr().contains(&expected));
    eprintln!("fixture thread left on {:#x}: {}", layout_value(thread), helper.stderr().trim());
    assert_eq!(layout_list(), restore.list, "keyboard layout list changed");
    // The window that takes the foreground once the fixture's closes (here the one in front before
    // the test) is put on the English layout the closed window was on, until Windows moves it off
    // that layout once the helper unloaded it (measured: back within a second).
    let settling = Instant::now();
    wait_for(Duration::from_secs(2), "foreground thread layout changed", || layout_value(restore.foreground_thread) == restore.foreground_layout);
    eprintln!("foreground thread back on {:#x} after {:?}", restore.foreground_layout, settling.elapsed());
    drop(helper);
    drop(fixture);
}

fn wait_for_window(helper: &mut Helper, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with(prefix)).and_then(|_| window["window_id"].as_str().map(str::to_string))
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{prefix} was not listed");
}

fn wait_for(limit: Duration, message: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{message}");
}

/// The fixture's title, read directly: the helper is busy while it types.
fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn replace(helper: &mut Helper, window_id: &str, hwnd: HWND, text: &str) {
    require_foreground(hwnd);
    let select_all = helper.request("press_key", json!({ "window_id": window_id, "keys": ["ctrl", "a"] }));
    assert_eq!(select_all["ok"], true, "{select_all}");
    require_foreground(hwnd);
    let typed = helper.request("type_text", json!({ "window_id": window_id, "text": text }));
    assert_eq!(typed["ok"], true, "{typed}");
    assert_eq!(edit_of(&title_of(helper, window_id)), text, "{text}");
}

fn click_edit(helper: &mut Helper, window_id: &str) {
    let info = helper.request("get_window", json!({ "window_id": window_id }));
    let width = info["result"]["width"].as_i64().unwrap_or(480);
    let height = info["result"]["height"].as_i64().unwrap_or(360);
    let clicked = helper.request(
        "click",
        json!({ "window_id": window_id, "x": width * 240 / 480, "y": height * 24 / 360, "button": "left", "click_count": 1 }),
    );
    assert_eq!(clicked["ok"], true, "{clicked}");
}

fn require_foreground(expected: HWND) {
    let foreground = unsafe { GetForegroundWindow() };
    assert_eq!(foreground, expected, "refusing to send input: the fixture is not in the foreground");
}

fn edit_of(title: &str) -> String {
    title.split("edit=").nth(1).unwrap_or("").split(" | len=").next().unwrap_or("").to_string()
}

fn len_of(title: &str) -> usize {
    title.split("len=").nth(1).unwrap_or("0").split(" | ev=").next().unwrap_or("0").parse().unwrap_or(0)
}

fn title_of(helper: &mut Helper, window_id: &str) -> String {
    helper.request("get_window", json!({ "window_id": window_id }))["result"]["title"].as_str().unwrap_or("").to_string()
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        if let Some(id) = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).and_then(|_| window["window_id"].as_str().map(str::to_string))
            })
        }) {
            return id;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("fixture was not listed");
}

fn parse_hwnd(window_id: &str) -> HWND {
    HWND(window_id.parse::<isize>().expect("hwnd") as *mut core::ffi::c_void)
}

fn layout_of(hwnd: HWND) -> u64 {
    unsafe { layout_value(GetWindowThreadProcessId(hwnd, None)) }
}

fn layout_value(thread: u32) -> u64 {
    unsafe { GetKeyboardLayout(thread).0 as usize as u64 }
}

fn ime_is_open(hwnd: HWND) -> bool {
    unsafe {
        let ime = ImmGetDefaultIMEWnd(hwnd);
        if ime.0.is_null() {
            return false;
        }
        SendMessageW(ime, WM_IME_CONTROL, Some(WPARAM(IMC_GETOPENSTATUS)), Some(LPARAM(0))).0 != 0
    }
}

fn layout_list() -> Vec<u64> {
    unsafe {
        let mut buffer = [HKL::default(); 32];
        let count = GetKeyboardLayoutList(Some(&mut buffer));
        buffer.iter().take(count.max(0) as usize).map(|layout| layout.0 as usize as u64).collect()
    }
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else { return false };
        let _ = CloseDesktop(desktop);
        true
    }
}

struct LayoutGuard {
    foreground: HWND,
    foreground_thread: u32,
    foreground_layout: u64,
    list: Vec<u64>,
}

impl LayoutGuard {
    fn capture() -> Self {
        unsafe {
            let foreground = GetForegroundWindow();
            let foreground_thread = GetWindowThreadProcessId(foreground, None);
            Self { foreground, foreground_thread, foreground_layout: layout_value(foreground_thread), list: layout_list() }
        }
    }
}

impl Drop for LayoutGuard {
    fn drop(&mut self) {
        unsafe {
            if layout_value(self.foreground_thread) != self.foreground_layout && !self.foreground.0.is_null() {
                let _ = PostMessageW(
                    Some(self.foreground),
                    WM_INPUTLANGCHANGEREQUEST,
                    WPARAM(0),
                    LPARAM(self.foreground_layout as isize),
                );
                let deadline = Instant::now() + Duration::from_millis(500);
                while Instant::now() < deadline && layout_value(self.foreground_thread) != self.foreground_layout {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            let mut buffer = [HKL::default(); 32];
            let count = GetKeyboardLayoutList(Some(&mut buffer));
            for layout in buffer.iter().take(count.max(0) as usize) {
                let value = layout.0 as usize as u64;
                if !self.list.contains(&value) {
                    let _ = UnloadKeyboardLayout(*layout);
                }
            }
        }
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
    /// Everything the helper wrote to stderr so far.
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
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
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let stderr = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let mut errors = BufReader::new(child.stderr.take().unwrap());
        let collected = stderr.clone();
        std::thread::spawn(move || loop {
            let mut line = String::new();
            match errors.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => collected.lock().unwrap().push_str(&line),
            }
        });
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        Self { child, reader, next_id: 1, stderr }
    }

    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.send(method, params);
        self.recv()
    }

    fn send(&mut self, method: &str, params: Value) {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        let _ = self.child.stdin.as_ref().unwrap().flush();
    }

    fn recv(&mut self) -> Value {
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
