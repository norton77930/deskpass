//! Ticket 34: a UWP app's window belongs to ApplicationFrameHost.exe, and the app itself is the
//! process of the frame's `Windows.UI.Core.CoreWindow` child.
//!
//! The resolution code is compiled into this test binary from `src/windows_list.rs`, so a test can
//! name its own process as the frame host (`identify_as`). That switch exists only here: the
//! release helper always calls `identify`, which compares against the system's
//! ApplicationFrameHost.exe path and takes no argument, environment variable or file that could
//! change it. The windows here are never shown, so nothing on the desktop changes.

use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW,
    RegisterClassExW, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_QUIT, WNDCLASSEXW, WS_CHILD, WS_OVERLAPPEDWINDOW,
};

#[allow(dead_code)]
#[path = "../src/windows_list.rs"]
mod windows_list;

use windows_list::{identify, identify_as, is_frame_host, pick_core_window, process_path, Identity, CORE_WINDOW_CLASS};

const SYSTEM_FRAME_HOST: &str = r"C:\Windows\System32\ApplicationFrameHost.exe";

#[test]
fn the_frame_host_is_matched_by_its_whole_path_in_any_case() {
    assert!(is_frame_host(SYSTEM_FRAME_HOST, SYSTEM_FRAME_HOST));
    assert!(is_frame_host(r"c:\WINDOWS\system32\applicationframehost.exe", SYSTEM_FRAME_HOST));
    assert!(!is_frame_host(r"D:\Tools\ApplicationFrameHost.exe", SYSTEM_FRAME_HOST));
    assert!(!is_frame_host("ApplicationFrameHost.exe", SYSTEM_FRAME_HOST));
    assert!(!is_frame_host(r"C:\Windows\System32\ApplicationFrameHost.exe.bak", SYSTEM_FRAME_HOST));
    assert!(!is_frame_host(r"C:\Windows\System32\notepad.exe", SYSTEM_FRAME_HOST));
}

#[test]
fn the_app_is_the_single_core_window_child_of_another_process() {
    let child = |id: u32, class: &str, pid: u32| (id, class.to_string(), pid);
    let frame_pid = 10;
    // The title bar windows belong to the frame host; the CoreWindow to the app.
    let calculator = [child(1, "ApplicationFrameTitleBarWindow", 10), child(2, "ApplicationFrameTitleBarWindow", 10), child(3, CORE_WINDOW_CLASS, 77)];
    assert_eq!(pick_core_window(frame_pid, &calculator), Some((3, 77)));
    // Two CoreWindows of the same app: still that app, the first in z-order takes the keys.
    assert_eq!(pick_core_window(frame_pid, &[child(3, CORE_WINDOW_CLASS, 77), child(4, CORE_WINDOW_CLASS, 77)]), Some((3, 77)));
    // No CoreWindow (the app is starting or suspended), or only the frame host's own: unresolved.
    assert_eq!(pick_core_window(frame_pid, &[child(1, "ApplicationFrameTitleBarWindow", 10)]), None);
    assert_eq!(pick_core_window::<u32>(frame_pid, &[]), None);
    assert_eq!(pick_core_window(frame_pid, &[child(3, CORE_WINDOW_CLASS, 10)]), None);
    assert_eq!(pick_core_window(frame_pid, &[child(3, CORE_WINDOW_CLASS, 0)]), None);
    // A class that only looks alike is not the CoreWindow.
    assert_eq!(pick_core_window(frame_pid, &[child(3, "windows.ui.core.corewindow", 77)]), None);
    // CoreWindows of two different processes: which app it is cannot be told, so unresolved.
    assert_eq!(pick_core_window(frame_pid, &[child(3, CORE_WINDOW_CLASS, 77), child(4, CORE_WINDOW_CLASS, 88)]), None);
}

/// A frame like ApplicationFrameHost.exe's, with a CoreWindow child of a second process put into
/// it with SetParent, the way a UWP app's sits there. With this test process named as the frame
/// host the app is that second process, and its CoreWindow takes the keyboard input. Named as
/// anything else (the real frame host path), the frame is an ordinary window of this process.
/// Once that process is gone (the race: resolved at list time, gone at action time), unresolved.
#[test]
fn a_frame_resolves_to_the_process_of_its_core_window_child() {
    let own_pid = std::process::id();
    let own_path = process_path(own_pid).expect("own path");
    let frame = TestFrame::open(false);
    let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--core-window-child", &frame.id().to_string()]).spawn().expect("fixture"));
    let core = frame.wait_for_core_window(Duration::from_secs(5)).expect("the fixture's CoreWindow child did not appear in the frame");
    let mut core_pid = 0u32;
    unsafe { GetWindowThreadProcessId(core, Some(&mut core_pid)) };
    assert_eq!(core_pid, fixture.0.id());
    let fixture_path = process_path(core_pid).expect("fixture path");
    assert!(fixture_path.to_ascii_lowercase().ends_with("fixture-window.exe"), "{fixture_path}");

    assert_eq!(identify_as(frame.hwnd(), own_pid, &own_path), Some(Identity::Hosted { path: fixture_path, pid: core_pid, input: core }));
    assert_eq!(identify_as(frame.hwnd(), own_pid, &own_path.to_uppercase()), identify_as(frame.hwnd(), own_pid, &own_path));
    assert_eq!(identify(frame.hwnd(), own_pid), Some(Identity::Own { path: own_path.clone() }));

    drop(fixture);
    let deadline = Instant::now() + Duration::from_secs(5);
    while frame.core_window().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(identify_as(frame.hwnd(), own_pid, &own_path), Some(Identity::Unresolved { frame: own_path }));
}

#[test]
fn a_frame_without_a_core_window_of_another_process_is_unresolved() {
    let own_pid = std::process::id();
    let own_path = process_path(own_pid).expect("own path");
    let empty = TestFrame::open(false);
    assert_eq!(identify_as(empty.hwnd(), own_pid, &own_path), Some(Identity::Unresolved { frame: own_path.clone() }));
    // A CoreWindow child of the frame host's own process is not the app.
    let own_core = TestFrame::open(true);
    assert!(own_core.core_window().is_some());
    assert_eq!(identify_as(own_core.hwnd(), own_pid, &own_path), Some(Identity::Unresolved { frame: own_path }));
}

/// A hidden top-level window of this process, on a thread of its own that runs its messages.
struct TestFrame {
    hwnd: isize,
    thread: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl TestFrame {
    fn open(own_core_window: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        let join = std::thread::spawn(move || unsafe {
            register_classes();
            let instance = GetModuleHandleW(None).expect("module");
            let frame = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("CUMCP_TEST_FRAME"),
                w!("CUMCP_TEST_FRAME"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                300,
                200,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("frame");
            if own_core_window {
                CreateWindowExW(WINDOW_EX_STYLE::default(), w!("Windows.UI.Core.CoreWindow"), w!(""), WS_CHILD, 0, 0, 100, 100, Some(frame), None, Some(instance.into()), None)
                    .expect("own core window");
            }
            sender.send((frame.0 as isize, GetCurrentThreadId())).expect("send");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = DestroyWindow(frame);
        });
        let (hwnd, thread) = receiver.recv().expect("frame window");
        Self { hwnd, thread, join: Some(join) }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    fn id(&self) -> isize {
        self.hwnd
    }

    fn core_window(&self) -> Option<HWND> {
        unsafe { FindWindowExW(Some(self.hwnd()), None, w!("Windows.UI.Core.CoreWindow"), None) }.ok()
    }

    fn wait_for_core_window(&self, limit: Duration) -> Option<HWND> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(core) = self.core_window() {
                return Some(core);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for TestFrame {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn register_classes() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        for name in [w!("CUMCP_TEST_FRAME"), w!("Windows.UI.Core.CoreWindow")] {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(test_proc),
                hInstance: instance.into(),
                lpszClassName: name,
                ..Default::default()
            };
            assert_ne!(RegisterClassExW(&class), 0, "RegisterClassExW");
        }
    });
}

unsafe extern "system" fn test_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
