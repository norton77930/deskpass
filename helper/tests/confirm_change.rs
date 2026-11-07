//! Ticket 114: `confirm_change`, Deskpass's native confirm window for a change the settings page
//! asks for that loosens a limit (restoring an app's default). It is protected like the `ask_user`
//! dialog: only the user can answer it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW, GetClientRect, GetDlgItem, GetForegroundWindow, GetGUIThreadInfo,
    GetMessageW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostThreadMessageW, RegisterClassExW, SetWindowPos, GUITHREADINFO,
    HWND_TOP, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_QUIT, WNDCLASSEXW, WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

#[path = "interactive_lock.rs"]
mod interactive_lock;

/// The confirm window's class and control ids (`helper/src/ask.rs`).
const CONFIRM_CLASS: &str = "DESKPASS_CONFIRM_CHANGE";
const ASK_CLASS: &str = "DESKPASS_ASK_USER";
const TEXT_ID: i32 = 100;
const ALLOW_ID: i32 = 101;
const CANCEL_ID: i32 = 102;
const REMINDER: &str = "如果不是你在設定網頁按的,請按「取消」";

fn sap(timeout_ms: u64) -> Value {
    json!({ "app_name": "SAP Logon", "current_state": "封鎖(你設定)", "restored_state": "一般:每次詢問", "timeout_ms": timeout_ms })
}

/// The window names the app and the change from the current state to the restored one, reminds
/// the user to cancel a change they did not ask for, and has "Allow" and "Cancel"; a click on
/// "Allow" answers `allow`, a click on "Cancel" answers `cancel`, and the window is gone after each.
#[test]
fn the_window_shows_the_change_and_its_buttons_answer_allow_and_cancel() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();

    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    let text = control_text(window, TEXT_ID);
    for part in ["SAP Logon", "封鎖(你設定)", "一般:每次詢問", REMINDER] {
        assert!(text.contains(part), "the window does not say {part:?}: {text:?}");
    }
    assert!(window_title(window).contains("Deskpass"), "{:?}", window_title(window));
    assert_eq!(control_text(window, ALLOW_ID), "允許");
    assert_eq!(control_text(window, CANCEL_ID), "取消");
    answer(helper.pid(), "allow");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "allow", "{response}");
    assert!(!unsafe { IsWindow(Some(window)) }.as_bool(), "the answered window is still open");

    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    answer(helper.pid(), "deny");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "cancel", "{response}");
    assert!(!unsafe { IsWindow(Some(window)) }.as_bool(), "the answered window is still open");
}

/// The focus starts on "Cancel", so Enter alone answers `cancel`, never `allow`.
#[test]
fn the_focus_starts_on_cancel_and_enter_cancels() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    wait_until(|| unsafe { GetForegroundWindow() } == window, "the window did not take the foreground");
    let cancel = unsafe { GetDlgItem(Some(window), CANCEL_ID) }.expect("cancel button");
    wait_until(|| focus_of(window) == Some(cancel), "the focus is not on Cancel");
    let pressed = answer(helper.pid(), "enter");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "cancel", "Enter alone must not allow: {response}");
    assert!(!unsafe { IsWindow(Some(window)) }.as_bool(), "the window is still open after Enter");
}

/// Closing the window (Alt+F4, its close command) answers `cancel`.
#[test]
fn closing_the_window_cancels() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    let pressed = answer(helper.pid(), "alt-f4");
    assert_eq!(pressed["foreground"], true, "{pressed}");
    let response = helper.wait(asked, Duration::from_secs(5));
    assert_eq!(response["result"]["answer"], "cancel", "closing must not allow: {response}");
    assert!(!unsafe { IsWindow(Some(window)) }.as_bool(), "the window is still open after it was closed");
    assert_eq!(windows_of(helper.pid(), CONFIRM_CLASS), Vec::<isize>::new());
}

/// An unanswered window closes itself when its time is up and answers `timeout`; nothing in the
/// request answers it in advance.
#[test]
fn an_unanswered_window_times_out_and_closes() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let started = Instant::now();
    let mut params = sap(1_500);
    for (key, value) in [("answer", json!("allow")), ("auto_answer", json!("allow")), ("skip", json!(true)), ("default", json!("allow"))] {
        params[key] = value;
    }
    let asked = helper.send("confirm_change", params);
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    let response = helper.wait(asked, Duration::from_secs(10));
    let elapsed = started.elapsed();
    assert_eq!(response["result"]["answer"], "timeout", "{response}");
    assert!(elapsed >= Duration::from_millis(1_400) && elapsed < Duration::from_secs(4), "timed out after {elapsed:?}");
    assert!(!unsafe { IsWindow(Some(window)) }.as_bool(), "the timed-out window is still open");
    assert_eq!(windows_of(helper.pid(), CONFIRM_CLASS), Vec::<isize>::new(), "a window is left behind");
}

/// While the window is open, every other request to the same helper is refused at once with
/// `prompt_open` (the `ask_user` dialog's answer) without running, including another
/// `confirm_change` and an `ask_user`; and while an `ask_user` dialog is open, `confirm_change` is
/// refused the same way. Nothing reaches the window.
#[test]
fn while_the_window_is_open_other_requests_are_refused_prompt_open() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    let id = window_id(window);
    let (x, y) = allow_offset(window);
    let attempts = [
        ("list_windows", json!({})),
        ("get_window", json!({ "window_id": id })),
        ("click", json!({ "window_id": id, "x": x, "y": y })),
        ("type_text", json!({ "window_id": id, "text": "\n" })),
        ("press_key", json!({ "window_id": id, "keys": ["return"] })),
        ("capture_window", json!({ "window_id": id, "target_width": 100, "target_height": 100 })),
        ("confirm_change", sap(5_000)),
        ("ask_user", json!({ "kind": "continue", "timeout_ms": 5_000 })),
    ];
    for (method, params) in attempts {
        let started = Instant::now();
        let sent = helper.send(method, params);
        let response = helper.wait(sent, Duration::from_secs(3));
        assert_eq!(response["ok"], false, "{method} ran while the window was open: {response}");
        assert_eq!(response["error"]["code"], "prompt_open", "{method}: {response}");
        assert!(started.elapsed() < Duration::from_secs(2), "{method} was not refused at once ({:?})", started.elapsed());
        assert!(unsafe { IsWindowVisible(window) }.as_bool(), "the window closed after {method}");
        assert!(!helper.answered(asked), "the window was answered by {method}");
    }
    assert_eq!(windows_of(helper.pid(), CONFIRM_CLASS).len(), 1, "a second confirm window opened");
    answer(helper.pid(), "deny");
    assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "cancel");

    let asked = helper.send("ask_user", json!({ "kind": "continue", "timeout_ms": 30_000 }));
    wait_for_window(helper.pid(), ASK_CLASS);
    let refused = helper.request("confirm_change", sap(5_000));
    assert_eq!(refused["error"]["code"], "prompt_open", "{refused}");
    assert_eq!(windows_of(helper.pid(), CONFIRM_CLASS), Vec::<isize>::new(), "a confirm window opened over the question");
    answer_class(helper.pid(), "deny", ASK_CLASS);
    assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "deny");
}

/// The window belongs to the helper process, so another helper (another server's, which does run
/// requests meanwhile) neither lists it, describes it, clicks it nor sends it keys: each is
/// `window_not_found`, and the window stays open and unanswered.
#[test]
fn another_helper_neither_lists_nor_targets_nor_types_into_the_window() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut helper = Helper::spawn();
    let mut other = Helper::spawn();
    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    wait_until(|| unsafe { GetForegroundWindow() } == window, "the window did not take the foreground");
    std::thread::sleep(Duration::from_millis(900)); // past the arming of "Allow"
    let id = window_id(window);
    let (x, y) = allow_offset(window);

    let listed = other.request("list_windows", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
    assert!(!listed["result"]["windows"].as_array().expect("windows").iter().any(|listed| listed["window_id"] == id.as_str()), "the window is listed: {listed}");
    for (method, params) in [
        ("get_window", json!({ "window_id": id })),
        ("activate_window", json!({ "window_id": id })),
        ("click", json!({ "window_id": id, "x": x, "y": y, "button": "left", "click_count": 1 })),
        ("type_text", json!({ "window_id": id, "text": "\n" })),
        ("press_key", json!({ "window_id": id, "keys": ["return"] })),
        ("press_key", json!({ "window_id": id, "keys": ["tab"] })),
        ("press_key", json!({ "window_id": id, "keys": ["space"] })),
    ] {
        let response = other.request(method, params);
        assert_eq!(response["error"]["code"], "window_not_found", "{method}: {response}");
        assert!(unsafe { IsWindowVisible(window) }.as_bool(), "the window closed after {method}");
        assert!(!helper.answered(asked), "the window was answered by another helper's {method}: {:?}", helper.responses.get(&asked));
    }
    answer(helper.pid(), "deny");
    assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "cancel");
}

/// The window is excluded from screen capture, so a screenshot of where it is shows what is behind
/// it: a magenta window of the test's own put right behind it.
#[test]
fn a_screen_capture_does_not_show_the_window() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut helper = Helper::spawn();
    let asked = helper.send("confirm_change", sap(30_000));
    let window = wait_for_window(helper.pid(), CONFIRM_CLASS);
    wait_until(|| unsafe { GetForegroundWindow() } == window, "the window did not take the foreground");
    let client = client_rect_on_screen(window);
    let backdrop = TestWindow::show("CUMCP_TEST_CONFIRM_BACKDROP", WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, WS_POPUP, client, COLORREF(0x00FF_00FF));
    std::thread::sleep(Duration::from_millis(500));
    assert!(unsafe { IsWindowVisible(window) }.as_bool(), "the window closed before the capture");
    let pixels = capture_screen(client);
    let magenta = pixels.iter().filter(|&&[r, g, b]| r > 230 && g < 25 && b > 230).count();
    let share = magenta as f64 / pixels.len().max(1) as f64;
    drop(backdrop);
    assert!(share > 0.95, "the capture shows the window: only {:.1}% of its client area is the window behind it", share * 100.0);
    answer(helper.pid(), "deny");
    assert_eq!(helper.wait(asked, Duration::from_secs(5))["result"]["answer"], "cancel");
}

/// Missing or bad parameters are `invalid_params`, naming the parameter, and show no window.
#[test]
fn confirm_change_refuses_bad_parameters_without_a_window() {
    let mut helper = Helper::spawn();
    let without = |name: &str| {
        let mut params = sap(5_000);
        params.as_object_mut().expect("params").remove(name);
        params
    };
    let with = |name: &str, value: Value| {
        let mut params = sap(5_000);
        params[name] = value;
        params
    };
    let cases = [
        (without("app_name"), "app_name"),
        (without("current_state"), "current_state"),
        (without("restored_state"), "restored_state"),
        (without("timeout_ms"), "timeout_ms"),
        (with("app_name", json!("  ")), "app_name"),
        (with("current_state", json!(3)), "current_state"),
        (with("timeout_ms", json!(999)), "timeout_ms"),
        (with("timeout_ms", json!(120_001)), "timeout_ms"),
        (with("timeout_ms", json!("60000")), "timeout_ms"),
    ];
    for (params, named) in cases {
        let response = helper.request("confirm_change", params.clone());
        assert_eq!(response["error"]["code"], "invalid_params", "{params}: {response}");
        let message = response["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains(named), "{params}: the error does not name {named}: {response}");
    }
    assert_eq!(windows_of(helper.pid(), CONFIRM_CLASS), Vec::<isize>::new());
    // Each refused request closed its question: the helper takes requests again.
    let listed = helper.request("list_windows", json!({}));
    assert_eq!(listed["ok"], true, "{listed}");
}

fn answer(helper_pid: u32, press: &str) -> Value {
    answer_class(helper_pid, press, CONFIRM_CLASS)
}

fn answer_class(helper_pid: u32, press: &str, class: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_prompt-answerer"))
        .args(["--pid", &helper_pid.to_string(), "--press", press, "--wait-ms", "10000", "--class", class])
        .output()
        .expect("prompt-answerer");
    assert!(output.status.success(), "prompt-answerer --press {press} failed: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("prompt-answerer output")
}

fn wait_for_window(pid: u32, class: &str) -> HWND {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(&found) = windows_of(pid, class).first() {
            return HWND(found as *mut core::ffi::c_void);
        }
        assert!(Instant::now() < deadline, "no {class} window appeared");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Visible top-level windows of `class` that belong to process `pid`.
fn windows_of(pid: u32, class: &str) -> Vec<isize> {
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    all.into_iter()
        .filter(|hwnd| {
            let mut owner = 0u32;
            unsafe {
                GetWindowThreadProcessId(*hwnd, Some(&mut owner));
            }
            let mut name = [0u16; 128];
            let length = unsafe { GetClassNameW(*hwnd, &mut name) };
            owner == pid && String::from_utf16_lossy(&name[..length.max(0) as usize]) == class && unsafe { IsWindowVisible(*hwnd) }.as_bool()
        })
        .map(|hwnd| hwnd.0 as isize)
        .collect()
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
    windows::Win32::Foundation::TRUE
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn control_text(window: HWND, id: i32) -> String {
    let control = unsafe { GetDlgItem(Some(window), id) }.unwrap_or_else(|_| panic!("control {id} missing"));
    let mut buffer = [0u16; 2048];
    let length = unsafe { GetWindowTextW(control, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

/// The window that has the keyboard focus on the window's thread.
fn focus_of(window: HWND) -> Option<HWND> {
    unsafe {
        let thread = GetWindowThreadProcessId(window, None);
        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        GetGUIThreadInfo(thread, &mut info).ok()?;
        (!info.hwndFocus.0.is_null()).then_some(info.hwndFocus)
    }
}

fn window_id(hwnd: HWND) -> String {
    (hwnd.0 as isize).to_string()
}

/// The middle of "Allow", relative to the window rectangle.
fn allow_offset(window: HWND) -> (i32, i32) {
    let button = unsafe { GetDlgItem(Some(window), ALLOW_ID) }.expect("allow button");
    let (mut outer, mut inner) = (RECT::default(), RECT::default());
    unsafe {
        GetWindowRect(window, &mut outer).expect("window rect");
        GetWindowRect(button, &mut inner).expect("button rect");
    }
    ((inner.left + inner.right) / 2 - outer.left, (inner.top + inner.bottom) / 2 - outer.top)
}

fn client_rect_on_screen(window: HWND) -> RECT {
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(window, &mut rect).expect("client rect");
        assert!(ClientToScreen(window, &mut origin).as_bool(), "client origin");
    }
    RECT { left: origin.x, top: origin.y, right: origin.x + rect.right, bottom: origin.y + rect.bottom }
}

/// Copies a rectangle of the screen the way a GDI screenshot does; one RGB triple per pixel.
fn capture_screen(rect: RECT) -> Vec<[u8; 3]> {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
        BitBlt(memory, 0, 0, width, height, Some(screen), rect.left, rect.top, SRCCOPY).expect("BitBlt");
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(memory, bitmap, 0, height as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        assert_eq!(lines, height, "GetDIBits");
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    bgra.chunks_exact(4).map(|pixel| [pixel[2], pixel[1], pixel[0]]).collect()
}

/// A plain coloured top-level window on a thread of its own, shown at the top of the ordinary
/// (not topmost) windows without activation. Closed when dropped.
struct TestWindow {
    thread: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestWindow {
    fn show(class: &'static str, ex_style: WINDOW_EX_STYLE, style: WINDOW_STYLE, rect: RECT, background: COLORREF) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(None).expect("module");
            let class: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
            let proc: WNDPROC = Some(plain_proc);
            let _ = RegisterClassExW(&WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: instance.into(),
                hbrBackground: CreateSolidBrush(background),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            });
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            let hwnd = CreateWindowExW(ex_style, PCWSTR(class.as_ptr()), w!("CUMCP test window"), style, rect.left, rect.top, width, height, None, None, Some(instance.into()), None)
                .expect("test window");
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), rect.left, rect.top, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            sender.send(GetCurrentThreadId()).expect("test window shown");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                DispatchMessageW(&message);
            }
            let _ = DestroyWindow(hwnd);
        });
        let thread = receiver.recv_timeout(Duration::from_secs(5)).expect("test window shown");
        Self { thread, handle: Some(handle) }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn plain_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
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

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else { return false };
        let _ = CloseDesktop(desktop);
        true
    }
}

/// A helper whose responses may arrive out of order: requests refused while the window is open
/// are answered before the request that opened it.
struct Helper {
    child: Child,
    lines: Receiver<Value>,
    responses: HashMap<u64, Value>,
    next_id: u64,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--lang", "zh-TW", "--server-pid", &std::process::id().to_string()])
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

    fn keep(&mut self, value: Value) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            self.responses.insert(id, value);
        }
    }

    fn answered(&mut self, id: u64) -> bool {
        while let Ok(value) = self.lines.try_recv() {
            self.keep(value);
        }
        self.responses.contains_key(&id)
    }

    fn wait(&mut self, id: u64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = self.responses.remove(&id) {
                return value;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(value) => self.keep(value),
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
