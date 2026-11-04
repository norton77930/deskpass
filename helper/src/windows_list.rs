use std::path::Path;
use std::sync::OnceLock;

use serde::Serialize;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HWND, LPARAM, RECT, STILL_ACTIVE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible,
    GWL_EXSTYLE, GW_CHILD, GW_HWNDNEXT, GW_OWNER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

#[derive(Debug, Serialize)]
pub struct ListedWindow {
    pub window_id: String,
    pub app: String,
    pub app_name: String,
    pub title: String,
    pub minimized: bool,
    pub elevated: bool,
    /// For an owned top-level window (an app's dialog): its nearest visible owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_window_id: Option<String>,
    /// A UWP app's frame whose app inside could not be identified (ticket 34): `app` and `app_name`
    /// are then the frame host's, and `get_window` and every input method refuse the window with
    /// `app_unresolved`. Only sent when true.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub app_unresolved: bool,
}

pub fn list_windows() -> Result<Vec<ListedWindow>, String> {
    let mut windows = Vec::new();
    for hwnd in top_level_windows()? {
        if let Ok(window) = describe(hwnd) {
            windows.push(window);
        }
    }
    Ok(windows)
}

/// All top-level windows, front to back.
pub(crate) fn top_level_windows() -> Result<Vec<HWND>, String> {
    let mut hwnds = Vec::new();
    unsafe {
        EnumWindows(Some(enum_proc), LPARAM(&mut hwnds as *mut Vec<HWND> as isize)).map_err(|error| error.to_string())?;
    }
    Ok(hwnds)
}

/// `(executable path, process id)` of the app behind each listed window (a UWP app's frame counts
/// as the app inside it), for `list_apps`' running flag.
pub(crate) fn listed_app_processes() -> Vec<(String, u32)> {
    let mut apps = Vec::new();
    for hwnd in top_level_windows().unwrap_or_default() {
        if !listed(hwnd) {
            continue;
        }
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        match identify(hwnd, pid) {
            Some(Identity::Own { path }) => apps.push((path, pid)),
            Some(Identity::Hosted { path, pid: app_pid, .. }) => apps.push((path, app_pid)),
            Some(Identity::Unresolved { .. }) | None => {}
        }
    }
    apps
}

pub fn get_window(window_id: &str) -> Result<serde_json::Value, (String, String)> {
    let hwnd = parse_hwnd(window_id).ok_or_else(|| ("invalid_params".to_string(), "window_id is not a window handle.".to_string()))?;
    unsafe {
        if !flag(IsWindow(Some(hwnd))) {
            return Err(("window_not_found".to_string(), "The window is no longer open.".to_string()));
        }
    }
    let described = match describe(hwnd) {
        Ok(described) => described,
        // It may have closed since the check above.
        Err(_) if unsafe { !flag(IsWindow(Some(hwnd))) } => {
            return Err(("window_not_found".to_string(), "The window is no longer open.".to_string()));
        }
        Err(unlisted) => return Err(unlisted.failure()),
    };
    // Never described by the frame host instead: the server would ask approval for it.
    if described.app_unresolved {
        return Err(app_unresolved());
    }
    let rect = visible_frame(hwnd).map_err(|message| ("internal".to_string(), message))?;
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    Ok(serde_json::json!({
        "window_id": described.window_id,
        "app": described.app,
        "app_name": described.app_name,
        "title": described.title,
        "minimized": described.minimized,
        "elevated": described.elevated,
        "owner_window_id": described.owner_window_id,
        "helper_elevated": process_elevated(std::process::id()),
        "pid": pid,
        "x": rect.left,
        "y": rect.top,
        "width": rect.right - rect.left,
        "height": rect.bottom - rect.top,
    }))
}

/// The window's visible frame in physical screen pixels: the DWM extended frame bounds,
/// which is what Windows Graphics Capture captures. GetWindowRect also covers the
/// invisible resize borders (and runs off-screen for maximized windows), so it is only a
/// fallback when DWM cannot answer.
pub(crate) fn visible_frame(hwnd: HWND) -> Result<RECT, String> {
    let mut rect = RECT::default();
    let dwm = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut RECT as *mut _, size_of::<RECT>() as u32)
    };
    match dwm {
        Ok(()) if rect.right > rect.left && rect.bottom > rect.top => return Ok(rect),
        Ok(()) => eprintln!("DWMWA_EXTENDED_FRAME_BOUNDS was empty for window {:?}; falling back to GetWindowRect", hwnd.0),
        Err(error) => eprintln!("DWMWA_EXTENDED_FRAME_BOUNDS failed for window {:?} ({error}); falling back to GetWindowRect", hwnd.0),
    }
    let mut fallback = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut fallback) }.map_err(|error| error.to_string())?;
    Ok(fallback)
}

pub(crate) fn parse_hwnd(window_id: &str) -> Option<HWND> {
    let value = window_id.parse::<isize>().ok()?;
    if value == 0 {
        return None;
    }
    Some(HWND(value as *mut core::ffi::c_void))
}

pub fn process_alive(pid: u32) -> bool {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        let _ = CloseHandle(handle);
        ok.is_ok() && code == STILL_ACTIVE.0 as u32
    }
}

/// `process_info`, for the server's lock check: whether `pid` is running and when it started, in
/// Unix milliseconds. No process with that pid (ERROR_INVALID_PARAMETER), or one that has exited,
/// is `alive: false`. A process that exists but cannot be opened or timed (another user's, for
/// example) is `alive: true` with `started_ms: null` and a `reason`.
pub fn process_info(params: &serde_json::Value) -> Result<serde_json::Value, (String, String)> {
    let pid = params
        .get("pid")
        .and_then(|value| value.as_u64())
        .filter(|pid| *pid > 0)
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| ("invalid_params".to_string(), "pid must be a positive whole number.".to_string()))?;
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => return Ok(serde_json::json!({ "alive": false })),
            Err(error) => {
                return Ok(serde_json::json!({ "alive": true, "started_ms": null, "reason": format!("the process could not be opened: {error}") }));
            }
        };
        let mut exit_code = 0u32;
        let exit_read = GetExitCodeProcess(handle, &mut exit_code);
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let timed = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(handle);
        if exit_read.is_ok() && exit_code != STILL_ACTIVE.0 as u32 {
            return Ok(serde_json::json!({ "alive": false }));
        }
        if let Err(error) = timed {
            return Ok(serde_json::json!({ "alive": true, "started_ms": null, "reason": format!("its start time could not be read: {error}") }));
        }
        // FILETIME counts 100 ns intervals since 1601-01-01, 11644473600 s before the Unix epoch.
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        let started_ms = (ticks / 10_000) as i64 - 11_644_473_600_000;
        Ok(serde_json::json!({ "alive": true, "started_ms": started_ms }))
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let list = &mut *(lparam.0 as *mut Vec<HWND>);
    list.push(hwnd);
    windows::Win32::Foundation::TRUE
}

/// Why an open window is not listed, and so not a target.
enum Unlisted {
    Hidden,
    Cloaked,
    Tool,
    Untitled,
    Own,
    UnknownApp,
}

impl Unlisted {
    /// The error for targeting such a window. It is open, so it is never "no longer open".
    fn failure(self) -> (String, String) {
        let message = match self {
            Unlisted::Own => return ("window_not_found".to_string(), "That window belongs to Deskpass itself and cannot be operated.".to_string()),
            Unlisted::Hidden => "The window is open but not visible, so it cannot be operated.",
            Unlisted::Cloaked => "The window is open but cloaked (on another virtual desktop, or hidden by its app), so it cannot be operated.",
            Unlisted::Tool => "The window is open but is a tool window (a palette, menu or tooltip), so it cannot be operated.",
            Unlisted::Untitled => "The window is open but has no title and no visible owner, so it cannot be operated.",
            Unlisted::UnknownApp => "The window is open but its app could not be identified, so it cannot be operated.",
        };
        ("window_not_operable".to_string(), message.to_string())
    }
}

/// Whether `hwnd` is a window `list_windows` lists and `get_window` describes.
pub(crate) fn listed(hwnd: HWND) -> bool {
    describe(hwnd).is_ok()
}

fn describe(hwnd: HWND) -> Result<ListedWindow, Unlisted> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        // The helper's own windows (the approval dialog, the overlay) are never targets, nor (ticket
        // 96) those of another helper process, such as another server's helper. This is checked first so
        // they are always `window_not_found`, whatever their visibility or style.
        if is_helper_process(pid) {
            return Err(Unlisted::Own);
        }
        if !flag(IsWindowVisible(hwnd)) {
            return Err(Unlisted::Hidden);
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32).is_ok() && cloaked != 0 {
            return Err(Unlisted::Cloaked);
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let tool = ex & WS_EX_TOOLWINDOW.0 != 0;
        let app_window = ex & WS_EX_APPWINDOW.0 != 0;
        if tool && !app_window {
            return Err(Unlisted::Tool);
        }
        // Owned windows are listed too: they are the app's dialogs (Save As, message boxes,
        // settings). An owner that is itself hidden (some frameworks own their main window
        // with an invisible one) is not reported.
        let owner = visible_owner(hwnd);
        // A dialog may have no title (Paint's and Office's save prompts); a window with neither
        // a title nor a visible owner is background noise.
        let title = window_text(hwnd);
        if title.is_empty() && !app_window && owner.is_none() {
            return Err(Unlisted::Untitled);
        }
        let identity = if pid == 0 { None } else { identify(hwnd, pid) }.ok_or(Unlisted::UnknownApp)?;
        let (app, elevated, app_unresolved) = match identity {
            Identity::Own { path } => (path, process_elevated(pid), false),
            // The frame host and the app inside: either running as administrator makes it one.
            Identity::Hosted { path, pid: app_pid, .. } => (path, process_elevated(pid) || process_elevated(app_pid), false),
            Identity::Unresolved { frame } => (frame, process_elevated(pid), true),
        };
        Ok(ListedWindow {
            window_id: (hwnd.0 as isize).to_string(),
            app: app.clone(),
            app_name: display_name(&app),
            title,
            minimized: flag(IsIconic(hwnd)),
            elevated,
            owner_window_id: owner.map(|owner| (owner.0 as isize).to_string()),
            app_unresolved,
        })
    }
}

/// The nearest visible window in `hwnd`'s owner chain, if any.
fn visible_owner(hwnd: HWND) -> Option<HWND> {
    let mut current = hwnd;
    for _ in 0..32 {
        current = unsafe { GetWindow(current, GW_OWNER) }.ok()?;
        if unsafe { flag(IsWindowVisible(current)) } {
            return Some(current);
        }
    }
    None
}

/// The executable path and display name of the app that owns `hwnd`, if its process is readable:
/// for a UWP app's frame the app inside it, as `list_windows` describes it. A frame whose app cannot
/// be identified is described as such (no path), never as the frame host (ticket 55).
pub(crate) fn window_app(hwnd: HWND) -> Option<(String, String)> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    let path = match identify(hwnd, pid)? {
        Identity::Own { path } | Identity::Hosted { path, .. } => path,
        Identity::Unresolved { .. } => return Some((String::new(), "an app that could not be identified".to_string())),
    };
    let name = display_name(&path);
    Some((path, name))
}

/// Ticket 34: the refusal for a UWP app's frame whose app inside could not be identified. Such a
/// window is never operated, and never approved, as the frame host.
pub(crate) fn app_unresolved() -> (String, String) {
    (
        "app_unresolved".to_string(),
        "The app inside this window could not be identified (it may still be starting or be suspended), so the window cannot be operated and no input was sent. Wait a moment, call computer_list_windows again, then retry.".to_string(),
    )
}

/// Ticket 55: the server sends the app it approved for a call as `expected_app` (the executable
/// path `get_window` gave it). The window must still belong to that app when the helper acts:
/// a handle that now names another app's window (a UWP frame hosting another app, or a reused
/// handle) gets `app_changed`, and a frame whose app cannot be identified `app_unresolved`, before
/// anything is sent or captured. A closed window is left to the caller's own `window_not_found`.
/// Without `expected_app` (the helper's own tests) nothing is compared.
pub(crate) fn check_expected_app(hwnd: HWND, params: &serde_json::Value) -> Result<(), (String, String)> {
    let Some(expected) = params.get("expected_app").and_then(|value| value.as_str()) else {
        return Ok(());
    };
    if unsafe { !flag(IsWindow(Some(hwnd))) } {
        return Ok(());
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    let current = match if pid == 0 { None } else { identify(hwnd, pid) } {
        Some(Identity::Own { path }) | Some(Identity::Hosted { path, .. }) => Some(path),
        Some(Identity::Unresolved { .. }) => return Err(app_unresolved()),
        None => None,
    };
    if current.as_deref().is_some_and(|path| same_app(path, expected)) {
        return Ok(());
    }
    Err(app_changed(expected, current.as_deref()))
}

/// Executable paths compared as Windows does, without regard to case.
pub(crate) fn same_app(path: &str, expected: &str) -> bool {
    path.to_lowercase() == expected.to_lowercase()
}

fn app_changed(expected: &str, current: Option<&str>) -> (String, String) {
    let now = match current {
        Some(path) => format!("now belongs to {} ({path})", display_name(path)),
        None => "now belongs to an app that could not be identified".to_string(),
    };
    (
        "app_changed".to_string(),
        format!(
            "The window {now}, not to {} ({expected}), which this call was approved for, so nothing was sent. Call computer_list_windows and choose the window again.",
            display_name(expected)
        ),
    )
}

pub(crate) fn window_text(hwnd: HWND) -> String {
    unsafe {
        let mut buffer = [0u16; 512];
        let length = GetWindowTextW(hwnd, &mut buffer);
        if length <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

/// Who a window's app is (ticket 34).
#[derive(Debug, PartialEq)]
pub(crate) enum Identity {
    /// An ordinary window: the app is the window's own process.
    Own { path: String },
    /// A UWP app's frame, a window of ApplicationFrameHost.exe: the app is the process of the
    /// frame's `Windows.UI.Core.CoreWindow` child (`input`), which also takes its keyboard input.
    Hosted { path: String, pid: u32, input: HWND },
    /// A frame whose app inside could not be identified; `frame` is the frame host's own path.
    Unresolved { frame: String },
}

/// `window`'s app; `pid` is the window's process. None when that process's path cannot be read.
pub(crate) fn identify(window: HWND, pid: u32) -> Option<Identity> {
    identify_as(window, pid, frame_host_path())
}

/// `identify` with `frame_host` as the path of ApplicationFrameHost.exe. The release helper only
/// reaches this through `identify`; the frame host tests call it to name a test process instead.
pub(crate) fn identify_as(window: HWND, pid: u32, frame_host: &str) -> Option<Identity> {
    let path = process_path(pid)?;
    if !is_frame_host(&path, frame_host) {
        return Some(Identity::Own { path });
    }
    let hosted = pick_core_window(pid, &direct_children(window)).and_then(|(input, app_pid)| {
        // The app's own process, never the frame host again.
        let app = process_path(app_pid).filter(|app| !is_frame_host(app, frame_host))?;
        Some(Identity::Hosted { path: app, pid: app_pid, input })
    });
    Some(hosted.unwrap_or(Identity::Unresolved { frame: path }))
}

/// ApplicationFrameHost.exe in the system directory (`C:\Windows\System32` on a default install),
/// the only executable whose windows are UWP frames. Not configurable: it decides which app a
/// window's approval is asked and remembered for.
pub(crate) fn frame_host_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut buffer = [0u16; 260];
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if length == 0 || length >= buffer.len() {
            return r"C:\Windows\System32\ApplicationFrameHost.exe".to_string();
        }
        format!(r"{}\ApplicationFrameHost.exe", String::from_utf16_lossy(&buffer[..length]).trim_end_matches('\\'))
    })
}

/// Whether `path` is `frame_host`: the whole path, compared without regard to case. An executable
/// elsewhere that is merely named ApplicationFrameHost.exe is not the frame host.
pub(crate) fn is_frame_host(path: &str, frame_host: &str) -> bool {
    path.to_lowercase() == frame_host.to_lowercase()
}

/// The class of the window inside a UWP app's frame that belongs to the app's own process.
pub(crate) const CORE_WINDOW_CLASS: &str = "Windows.UI.Core.CoreWindow";

/// Among a frame's direct children `(window, class, pid)`, in z-order, the app's CoreWindow and its
/// process: a child of class `Windows.UI.Core.CoreWindow` (exactly) that belongs to a process other
/// than the frame host's. None when there is no such child (the app is still starting, or is
/// suspended), or when such children belong to more than one process, so which app this is cannot
/// be told.
pub(crate) fn pick_core_window<W: Copy>(frame_pid: u32, children: &[(W, String, u32)]) -> Option<(W, u32)> {
    let mut apps = children.iter().filter(|(_, class, pid)| class == CORE_WINDOW_CLASS && *pid != 0 && *pid != frame_pid);
    let (window, pid) = apps.next().map(|(window, _, pid)| (*window, *pid))?;
    if apps.any(|(_, _, other)| *other != pid) {
        return None;
    }
    Some((window, pid))
}

/// `window`'s direct children `(child, class, pid)`, in z-order.
fn direct_children(window: HWND) -> Vec<(HWND, String, u32)> {
    let mut children = Vec::new();
    let mut child = unsafe { GetWindow(window, GW_CHILD) };
    // Bounded, in case the children change while they are walked.
    while let Ok(current) = child {
        if current.0.is_null() || children.len() >= 256 {
            break;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(current, Some(&mut pid)) };
        children.push((current, class_name(current), pid));
        child = unsafe { GetWindow(current, GW_HWNDNEXT) };
    }
    children
}

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

/// Ticket 96: whether `pid` is a Deskpass helper: this process, or another process of the
/// helper's executable, such as another server's helper. Their windows (approval dialogs,
/// confirmation windows, overlays) are never listed, described or targeted. Judged
/// by the process image's file name (`is_helper_image`); a process whose image cannot be read is
/// not taken for one (its windows then have no app either, and are not listed for that).
pub(crate) fn is_helper_process(pid: u32) -> bool {
    pid != 0 && (pid == std::process::id() || process_path(pid).is_some_and(|path| is_helper_image(&path)))
}

/// Ticket 96: whether an executable path is the helper's: the running helper's own file name,
/// `deskpass-helper.exe`, or a copy renamed aside while it ran (`deskpass-helper.old-<time>.exe`,
/// scripts/copy-helper.mjs), compared without regard to case. Ticket 110: also the helper of an
/// older install still running on the same desktop under the old name (`computer-use-helper.exe`
/// and its renamed-aside copies), whose dialogs and overlay are as much off limits.
pub(crate) fn is_helper_image(path: &str) -> bool {
    const PREFIXES: [&str; 2] = ["deskpass-helper.", "computer-use-helper."];
    static OWN: OnceLock<Option<String>> = OnceLock::new();
    let name = |path: &Path| path.file_name().and_then(|name| name.to_str()).map(str::to_ascii_lowercase);
    let Some(file) = name(Path::new(path)) else { return false };
    let own = OWN.get_or_init(|| std::env::current_exe().ok().and_then(|exe| name(&exe)));
    own.as_deref() == Some(file.as_str()) || (PREFIXES.iter().any(|prefix| file.starts_with(prefix)) && file.ends_with(".exe"))
}

pub(crate) fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let queried = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(handle);
        queried.ok()?;
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

fn process_elevated(pid: u32) -> bool {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut token = windows::Win32::Foundation::HANDLE::default();
        if OpenProcessToken(handle, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(handle);
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        let _ = CloseHandle(handle);
        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}

fn display_name(path: &str) -> String {
    file_description(path).unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(path)
            .to_string()
    })
}

fn file_description(path: &str) -> Option<String> {
    let path_wide = utf16(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(path_wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(path_wide.as_ptr()), None, size, data.as_mut_ptr() as *mut _).ok()?;
        let mut translation = std::ptr::null_mut();
        let mut translation_len = 0u32;
        if !flag(VerQueryValueW(data.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut translation, &mut translation_len))
            || translation_len < 4
        {
            return None;
        }
        let lang = *(translation as *const u16);
        let codepage = *(translation.add(2) as *const u16);
        let query = utf16(&format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"));
        let mut value = std::ptr::null_mut();
        let mut value_len = 0u32;
        if !flag(VerQueryValueW(data.as_ptr() as *const _, PCWSTR(query.as_ptr()), &mut value, &mut value_len)) || value_len == 0 {
            return None;
        }
        let chars = std::slice::from_raw_parts(value as *const u16, (value_len as usize).saturating_sub(1));
        let text = String::from_utf16_lossy(chars);
        if text.is_empty() { None } else { Some(text) }
    }
}

fn flag(value: windows::core::BOOL) -> bool {
    value.0 != 0
}

pub fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};

    /// Ticket 55: input and capture go to a window only while it belongs to the app the server
    /// approved for the call (`expected_app`, compared without regard to case); otherwise
    /// `app_changed`, before anything is sent. No `expected_app` compares nothing.
    #[test]
    fn a_window_of_another_app_than_the_approved_one_is_refused_with_app_changed() {
        unsafe {
            let window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("cumcp-unit"), WS_POPUP, 0, 0, 50, 50, None, None, None, None).expect("a window");
            let own = process_path(std::process::id()).expect("the test's own path");
            let check = |expected: Option<&str>| {
                let params = match expected {
                    Some(app) => serde_json::json!({ "expected_app": app }),
                    None => serde_json::json!({}),
                };
                check_expected_app(window, &params)
            };
            assert_eq!(check(None), Ok(()));
            assert_eq!(check(Some(&own)), Ok(()));
            assert_eq!(check(Some(&own.to_uppercase())), Ok(()));
            let changed = check(Some(r"C:\Windows\System32\notepad.exe")).expect_err("another app");
            assert_eq!(changed.0, "app_changed");
            assert!(changed.1.contains(&own) && changed.1.contains("notepad.exe"), "{}", changed.1);
            let _ = DestroyWindow(window);
            // A closed window is left to the caller's own window_not_found.
            assert_eq!(check(Some(r"C:\Windows\System32\notepad.exe")), Ok(()));
        }
    }
}
