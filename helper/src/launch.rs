use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER};
use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, AO_NOERRORUI};
use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW};

use crate::{apps, windows_list};

const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// `launch_app`: start `app` (an executable path), or with `aumid` (ticket 79) the packaged app of
/// that AppUserModelID, which must still run `app`, the executable the server approved. Waits up
/// to 10 seconds for the app's main window (`new_window`): the result is `{ pid, window, kind:
/// "main" }` as soon as one appears; when by then only a dialog of the app appeared, that window
/// with `kind: "dialog"`; otherwise `window: null` with a message.
pub fn launch_app(params: &Value) -> Result<Value, (String, String)> {
    let app = params.get("app").and_then(|value| value.as_str()).unwrap_or("");
    if app.is_empty() || !std::path::Path::new(app).exists() {
        return Err(("launch_failed".to_string(), "Could not find that executable.".to_string()));
    }
    let aumid = params.get("aumid").and_then(|value| value.as_str()).filter(|aumid| !aumid.is_empty());
    // Windows that exist now never count as the launch's result.
    let existing = existing_windows();
    let mut started = match aumid {
        Some(aumid) => {
            let runs = apps::uwp_executable(aumid)?;
            if !windows_list::same_app(&runs, app) {
                return Err((
                    "launch_failed".to_string(),
                    "That app does not run the executable that was approved, so it was not started. Call computer_list_apps again.".to_string(),
                ));
            }
            Started::Activated(activate(aumid)?)
        }
        None => Started::Child(spawn_app(app)?),
    };
    let pid = started.pid();
    let paths = launched_paths(pid, app);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exited = None;
    loop {
        if let Some(NewWindow::Main(window)) = new_window(pid, &paths, &existing) {
            return Ok(json!({ "pid": pid, "window": window, "kind": "main" }));
        }
        if exited.is_none() {
            if let Started::Child(child) = &mut started {
                match child.try_wait() {
                    Ok(status) => exited = status,
                    Err(error) => return Err(("launch_failed".to_string(), error.to_string())),
                }
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(started);
    match new_window(pid, &paths, &existing) {
        Some(NewWindow::Main(window)) => return Ok(json!({ "pid": pid, "window": window, "kind": "main" })),
        Some(NewWindow::Dialog(window)) => return Ok(json!({ "pid": pid, "window": window, "kind": "dialog" })),
        None => {}
    }
    // A launcher may hand its window to another process (a shell, an already-running app) and
    // exit with any code; only when no new window of the app shows up does the code decide.
    if let Some(status) = exited.filter(|status| !status.success()) {
        return Err(("launch_failed".to_string(), format!("The process exited with {status}, and no new window of that application appeared within 10 seconds.")));
    }
    Ok(json!({
        "pid": pid,
        "window": null,
        "message": "The application started, but no new window appeared within 10 seconds.",
    }))
}

/// What was started: a child process, or a packaged app Windows activated (its process id).
enum Started {
    Child(std::process::Child),
    Activated(u32),
}

impl Started {
    fn pid(&self) -> u32 {
        match self {
            Started::Child(child) => child.id(),
            Started::Activated(pid) => *pid,
        }
    }
}

fn spawn_app(app: &str) -> Result<std::process::Child, (String, String)> {
    let mut last_error = None;
    for flags in [CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP, CREATE_NEW_PROCESS_GROUP] {
        match Command::new(app).creation_flags(flags).spawn() {
            Ok(child) => return Ok(child),
            Err(error) => last_error = Some(error),
        }
    }
    Err((
        "launch_failed".to_string(),
        last_error.map(|error| error.to_string()).unwrap_or_else(|| "Could not start that executable.".to_string()),
    ))
}

/// Starts the packaged app `aumid` as the Start menu does, without arguments; its process id.
fn activate(aumid: &str) -> Result<u32, (String, String)> {
    let failed = |error: windows::core::Error| ("launch_failed".to_string(), format!("Windows could not start that app: {}", error.message()));
    unsafe {
        let manager: IApplicationActivationManager = CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER).map_err(failed)?;
        let wide = windows_list::utf16(aumid);
        manager.ActivateApplication(PCWSTR(wide.as_ptr()), PCWSTR::null(), AO_NOERRORUI).map_err(failed)
    }
}

/// Every top-level window handle open now, listed or not.
fn existing_windows() -> HashSet<isize> {
    windows_list::top_level_windows().unwrap_or_default().into_iter().map(|hwnd| hwnd.0 as isize).collect()
}

/// The executable path a window of the launched app reports as its `app`, lowercased: the running
/// process's own path when it can be read (the same spelling `list_windows` gives), and the path
/// that was asked for.
fn launched_paths(pid: u32, app: &str) -> Vec<String> {
    let mut paths: Vec<String> = windows_list::process_path(pid).into_iter().collect();
    paths.push(app.to_string());
    if let Ok(full) = std::fs::canonicalize(app) {
        paths.push(full.to_string_lossy().trim_start_matches(r"\\?\").to_string());
    }
    paths.into_iter().map(|path| path.to_lowercase()).collect()
}

/// A window that appeared since the launch, as `get_window` describes it.
enum NewWindow {
    /// Unowned, visible, not a tool window: the app's main window.
    Main(Value),
    /// Any other listed window of the app: a dialog (owned, even by a hidden window) or a palette.
    Dialog(Value),
}

/// The window that appeared since the launch, a main window rather than a dialog. A window counts
/// when its app's process is the launched one (for a UWP app's frame, the app inside it, ticket
/// 34) or runs the same executable; the launched process's own windows come first. Only listed
/// (visible) windows count; a main window is one with no owner at all, visible or hidden, and not
/// a tool window, so a dialog that an app shows before its main window, owned by a window it has
/// not shown yet, is never taken for it.
fn new_window(pid: u32, paths: &[String], existing: &HashSet<isize>) -> Option<NewWindow> {
    let mut mains: Vec<(u8, String)> = Vec::new();
    let mut dialogs: Vec<(u8, String)> = Vec::new();
    for window in windows_list::list_windows().ok()? {
        let Some(hwnd) = windows_list::parse_hwnd(&window.window_id) else {
            continue;
        };
        if existing.contains(&(hwnd.0 as isize)) || window.app_unresolved {
            continue;
        }
        let rank = if app_pid(hwnd) == Some(pid) {
            0
        } else if paths.contains(&window.app.to_lowercase()) {
            1
        } else {
            continue;
        };
        let owned = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|owner| !owner.0.is_null());
        let tool = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0;
        if owned || tool {
            dialogs.push((rank, window.window_id));
        } else {
            mains.push((rank, window.window_id));
        }
    }
    let described = |mut found: Vec<(u8, String)>| {
        found.sort_by_key(|(rank, _)| *rank);
        found.into_iter().find_map(|(_, window_id)| windows_list::get_window(&window_id).ok())
    };
    described(mains).map(NewWindow::Main).or_else(|| described(dialogs).map(NewWindow::Dialog))
}

/// The process of `hwnd`'s app: the window's own, or for a UWP app's frame the app inside it.
fn app_pid(hwnd: windows::Win32::Foundation::HWND) -> Option<u32> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    match windows_list::identify(hwnd, pid)? {
        windows_list::Identity::Hosted { pid: app, .. } => Some(app),
        _ => Some(pid),
    }
}
