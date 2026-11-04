use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CombineRgn, CreateCompatibleDC, CreateFontW, CreateRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    FillRect, GetMonitorInfoW, GetStockObject, InvalidateRect, MonitorFromWindow, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, FW_SEMIBOLD, HFONT, HRGN, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULLREGION, OUT_DEFAULT_PRECIS, RGN_COPY,
    RGN_DIFF, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, KillTimer, GWL_EXSTYLE, GW_HWNDPREV,
    PeekMessageW, PostThreadMessageW, RegisterClassExW, SetLayeredWindowAttributes, SetTimer, SetWindowDisplayAffinity,
    SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, HWND_TOPMOST, LWA_ALPHA, LWA_COLORKEY, MSG, PM_NOREMOVE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WINDOW_DISPLAY_AFFINITY, WM_APP, WM_DESTROY, WM_PAINT,
    WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::strings::{self, Lang};
use crate::windows_list;

/// The border's own label, above the target (ticket 145): the stop key of the delivery, as the
/// corner notice has it, so the two never disagree.
fn label_text(foreground: bool) -> &'static str {
    label_text_in(strings::current(), foreground)
}

fn label_text_in(lang: Lang, foreground: bool) -> &'static str {
    strings::text_in(lang, if foreground { strings::LABEL_FOREGROUND } else { strings::LABEL_BACKGROUND })
}
const LABEL_HEIGHT: i32 = 28;
const BORDER: i32 = 6;
const MAGENTA: u32 = 0x00FF_00FF;
const GREEN: u32 = 0x0000_FF00;
const INK: u32 = 0x0010_1010;

enum Cmd {
    Show { window_id: String, notice: String, foreground: bool, done: mpsc::Sender<Result<(), String>> },
    Hide { done: Option<mpsc::Sender<()>> },
}

/// Ticket 144: the corner notice's text, "AI is operating ‹App› · stop key" (open-source ticket 03: in the
/// UI language): the app by the name its
/// approval showed (`app_name`, left out when none is given) and the stop key of the delivery,
/// Ctrl+Alt+Esc in the background (the one that stops at any time, so also when none is given),
/// Esc in the foreground.
fn notice_text(params: &Value) -> Result<String, (String, String)> {
    notice_text_in(strings::current(), params)
}

fn notice_text_in(lang: Lang, params: &Value) -> Result<String, (String, String)> {
    let key = if foreground_of(params)? { "Esc" } else { "Ctrl+Alt+Esc" };
    let app = params.get("app_name").and_then(|value| value.as_str()).map(str::trim).unwrap_or("");
    Ok(if app.is_empty() {
        strings::fill(strings::text_in(lang, strings::NOTICE), &[("key", key)])
    } else {
        strings::fill(strings::text_in(lang, strings::NOTICE_APP), &[("app", app), ("key", key)])
    })
}

/// Whether `overlay_show` shows the border of a foreground action: `delivery_mode` foreground. None
/// given is the background's (the server sends a read's mode as the overlay showing has it).
fn foreground_of(params: &Value) -> Result<bool, (String, String)> {
    match params.get("delivery_mode") {
        None | Some(Value::Null) => Ok(false),
        Some(value) => match value.as_str() {
            Some("background") => Ok(false),
            Some("foreground") => Ok(true),
            _ => Err(("invalid_params".to_string(), "delivery_mode must be background or foreground.".to_string())),
        },
    }
}

/// Ticket 145: the border showing is a foreground action's, so a physical single Esc stops (`esc`).
/// Written by the overlay thread when it shows the border (the mode of that `overlay_show`) and
/// when it hides it, whatever hid it; read by the keyboard hook. An atomic, so the hook takes no
/// lock; it is set before `overlay_show` is answered, so before the action that follows it.
static FOREGROUND_BORDER: AtomicBool = AtomicBool::new(false);

/// Whether the border of a foreground action shows now (ticket 145, the single Esc's rule).
pub(crate) fn foreground_border_shown() -> bool {
    FOREGROUND_BORDER.load(Ordering::SeqCst)
}

/// The corner notice (ticket 144): a second window of the overlay thread, shown, followed and
/// hidden with the border.
struct Notice {
    hwnd: HWND,
    text: String,
    /// The DPI of the monitor it was last placed on, and the font made for it.
    dpi: u32,
    font: Option<HFONT>,
    /// Where it was last placed, with the text it was sized for.
    placed: Option<(RECT, String)>,
}

struct ThreadSlot {
    id: u32,
    join: Option<JoinHandle<()>>,
}

struct Ui {
    hwnd: HWND,
    target: HWND,
    /// The clip the overlay window's region was last set to; `None` until one is set.
    clip: Option<Clip>,
    notice: Notice,
}

/// What of the overlay is drawn (ticket 143): the border shows only where the target can be
/// seen, the target's frame minus the visible top-level windows stacked above it.
#[derive(Clone, PartialEq, Debug)]
enum Clip {
    /// Nothing covers the target or its border (a target in front): no region, as before.
    Whole,
    /// The target is covered entirely: nothing is drawn.
    Nothing,
    /// The overlay (its size) minus these covering frames, in overlay window coordinates.
    Without { size: (i32, i32), covers: Vec<RECT> },
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui {
        hwnd: HWND::default(),
        target: HWND::default(),
        clip: None,
        notice: Notice { hwnd: HWND::default(), text: String::new(), dpi: 0, font: None, placed: None },
    });
}

static QUEUE: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
static THREAD: Mutex<Option<ThreadSlot>> = Mutex::new(None);

/// Test builds only (ticket 143): the overlay is not excluded from screen capture, so seam B tests
/// can see in a screen capture where the border is drawn.
#[cfg(debug_assertions)]
static CAPTURABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(debug_assertions)]
pub fn debug_capturable() -> Value {
    CAPTURABLE.store(true, std::sync::atomic::Ordering::SeqCst);
    json!({})
}

fn display_affinity() -> WINDOW_DISPLAY_AFFINITY {
    #[cfg(debug_assertions)]
    if CAPTURABLE.load(std::sync::atomic::Ordering::SeqCst) {
        return windows::Win32::UI::WindowsAndMessaging::WDA_NONE;
    }
    WDA_EXCLUDEFROMCAPTURE
}

pub fn show(params: &Value) -> Result<Value, (String, String)> {
    let window_id = params.get("window_id").and_then(|value| value.as_str()).unwrap_or("");
    if windows_list::parse_hwnd(window_id).is_none() {
        return Err(("invalid_params".to_string(), "window_id is not a window handle.".to_string()));
    }
    let notice = notice_text(params)?;
    let foreground = foreground_of(params)?;
    let thread_id = ensure_thread().map_err(|message| ("internal".to_string(), message))?;
    let (done, wait) = mpsc::channel();
    QUEUE.lock().unwrap_or_else(|poison| poison.into_inner()).push(Cmd::Show { window_id: window_id.to_string(), notice, foreground, done });
    wake(thread_id)?;
    match wait.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(json!({})),
        Ok(Err(message)) => Err(("window_not_found".to_string(), message)),
        Err(_) => Err(("internal".to_string(), "Overlay did not appear.".to_string())),
    }
}

pub fn hide() -> Result<Value, (String, String)> {
    let thread_id = {
        let slot = THREAD.lock().unwrap_or_else(|poison| poison.into_inner());
        slot.as_ref().map(|thread| thread.id)
    };
    let Some(thread_id) = thread_id else {
        return Ok(json!({}));
    };
    let (done, wait) = mpsc::channel();
    QUEUE.lock().unwrap_or_else(|poison| poison.into_inner()).push(Cmd::Hide { done: Some(done) });
    wake(thread_id)?;
    let _ = wait.recv_timeout(Duration::from_secs(2));
    Ok(json!({}))
}

fn wake(thread_id: u32) -> Result<(), (String, String)> {
    unsafe { PostThreadMessageW(thread_id, WM_APP, WPARAM(0), LPARAM(0)) }
        .map_err(|error| ("internal".to_string(), error.to_string()))
}

fn ensure_thread() -> Result<u32, String> {
    let mut slot = THREAD.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(existing) = slot.as_ref() {
        if existing.join.as_ref().is_some_and(|join| !join.is_finished()) {
            return Ok(existing.id);
        }
    }
    let (ready, wait) = mpsc::channel();
    let join = std::thread::spawn(move || thread_main(ready));
    let id = match wait.recv() {
        Ok(Ok(id)) => id,
        Ok(Err(message)) => {
            drop(slot);
            let _ = join.join();
            return Err(message);
        }
        Err(error) => {
            drop(slot);
            let _ = join.join();
            return Err(error.to_string());
        }
    };
    *slot = Some(ThreadSlot { id, join: Some(join) });
    Ok(id)
}

fn thread_main(ready: mpsc::Sender<Result<u32, String>>) {
    unsafe {
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        if ready.send(Ok(GetCurrentThreadId())).is_err() {
            return;
        }
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            if message.message == WM_APP {
                process_queue();
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn process_queue() {
    let commands: Vec<Cmd> = {
        let mut queue = QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
        std::mem::take(&mut *queue)
    };
    for command in commands {
        match command {
            Cmd::Show { window_id, notice, foreground, done } => {
                let _ = done.send(show_on_thread(&window_id, notice, foreground));
            }
            Cmd::Hide { done } => {
                hide_on_thread();
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
        }
    }
}

fn show_on_thread(window_id: &str, notice: String, foreground: bool) -> Result<(), String> {
    let Some(target) = windows_list::parse_hwnd(window_id) else {
        return Err("window_id is not a window handle.".to_string());
    };
    unsafe {
        if !flag(IsWindow(Some(target))) {
            return Err("The window is no longer open.".to_string());
        }
    }
    let needs_window = UI.with(|ui| ui.borrow().hwnd.0.is_null());
    if needs_window {
        let hwnd = create_overlay()?;
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.hwnd = hwnd;
            ui.target = target;
        });
    } else {
        UI.with(|ui| ui.borrow_mut().target = target);
    }
    let hwnd = UI.with(|ui| ui.borrow().hwnd);
    if UI.with(|ui| ui.borrow().notice.hwnd.0.is_null()) {
        let created = create_notice()?;
        UI.with(|ui| ui.borrow_mut().notice.hwnd = created);
    }
    let notice_hwnd = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.notice.text = notice;
        ui.notice.hwnd
    });
    // Before `place`, which writes the label of this mode.
    FOREGROUND_BORDER.store(foreground, Ordering::SeqCst);
    if let Err(message) = place(hwnd, target) {
        FOREGROUND_BORDER.store(false, Ordering::SeqCst);
        return Err(message);
    }
    place_notice(notice_hwnd, target);
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, display_affinity());
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = UpdateWindow(hwnd);
        let _ = SetWindowDisplayAffinity(notice_hwnd, display_affinity());
        let _ = ShowWindow(notice_hwnd, SW_SHOWNOACTIVATE);
        let _ = UpdateWindow(notice_hwnd);
        if SetTimer(Some(hwnd), 1, 50, None) == 0 {
            return Err("Could not follow the target window.".to_string());
        }
    }
    Ok(())
}

fn hide_on_thread() {
    FOREGROUND_BORDER.store(false, Ordering::SeqCst);
    let (hwnd, notice) = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.target = HWND::default();
        ui.clip = None;
        (ui.hwnd, ui.notice.hwnd)
    });
    // The corner notice hides with the border (ticket 144), whatever hid it: the server's idle
    // timer, a stop, the end of the session, or the target closing.
    if !notice.0.is_null() {
        unsafe {
            let _ = ShowWindow(notice, SW_HIDE);
        }
    }
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        let _ = KillTimer(Some(hwnd), 1);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

fn create_overlay() -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|error| error.to_string())?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_proc),
            hInstance: windows::Win32::Foundation::HINSTANCE(instance.0),
            lpszClassName: w!("DESKPASS_OVERLAY"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
            return Err("Could not register the overlay window.".to_string());
        }
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("DESKPASS_OVERLAY"),
            PCWSTR(wide_text(label_text(false)).as_ptr()),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(windows::Win32::Foundation::HINSTANCE(instance.0)),
            None,
        )
        .map_err(|error| error.to_string())
        .and_then(|hwnd| {
            SetLayeredWindowAttributes(hwnd, COLORREF(MAGENTA), 0, LWA_COLORKEY).map_err(|error| error.to_string())?;
            Ok(hwnd)
        })
    }
}

/// Places the overlay around the target's visible frame (`visible_frame`, the DWM extended frame
/// bounds that screenshots and clicks use): GetWindowRect also covers a framed window's invisible
/// resize borders, which put the border about 8 px outside the window (F22).
fn place(hwnd: HWND, target: HWND) -> Result<(), String> {
    let rect = windows_list::visible_frame(target)?;
    let mut label: Vec<u16> = label_text(foreground_border_shown()).encode_utf16().collect();
    label.push(0);
    let bounds = RECT { left: rect.left - BORDER, top: rect.top - BORDER - LABEL_HEIGHT, right: rect.right + BORDER, bottom: rect.bottom + BORDER };
    // The region is set before the window is shown or moved, so the border never shows over a
    // window covering the target, not even for a frame.
    clip_to_visible(hwnd, target, rect, bounds);
    unsafe {
        SetWindowTextW(hwnd, windows::core::PCWSTR(label.as_ptr())).map_err(|error| error.to_string())?;
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .map_err(|error| error.to_string())?;
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
    Ok(())
}

/// Ticket 143: sets the overlay's window region to the part of the target the user can see, the
/// target's `frame` minus the visible top-level windows stacked above it, when that changed since
/// the last call. `bounds` is where the overlay goes (screen pixels). The follow timer calls this
/// every 50 ms, so the border catches up with a cover moving or the z-order changing within that.
/// A target in front has nothing above it, so its overlay has no region, as before the ticket.
fn clip_to_visible(hwnd: HWND, target: HWND, frame: RECT, bounds: RECT) {
    let covers: Vec<RECT> = unsafe { covers_above(target) }.into_iter().filter(|cover| overlaps(cover, &bounds)).collect();
    let clip = if covers.is_empty() {
        Clip::Whole
    } else if fully_covered(frame, &covers) {
        Clip::Nothing
    } else {
        let relative = covers
            .iter()
            .map(|cover| RECT { left: cover.left - bounds.left, top: cover.top - bounds.top, right: cover.right - bounds.left, bottom: cover.bottom - bounds.top })
            .collect();
        Clip::Without { size: (bounds.right - bounds.left, bounds.bottom - bounds.top), covers: relative }
    };
    if UI.with(|ui| ui.borrow().clip.as_ref() == Some(&clip)) {
        return;
    }
    let applied = unsafe {
        match &clip {
            Clip::Whole => SetWindowRgn(hwnd, None, true) != 0,
            Clip::Nothing => set_region(hwnd, CreateRectRgn(0, 0, 0, 0)),
            Clip::Without { size, covers } => {
                let region = CreateRectRgn(0, 0, size.0, size.1);
                for cover in covers {
                    subtract(region, cover);
                }
                set_region(hwnd, region)
            }
        }
    };
    if applied {
        UI.with(|ui| ui.borrow_mut().clip = Some(clip));
    }
}

/// Gives `region` to `hwnd` as its window region; the system owns it once that succeeded.
unsafe fn set_region(hwnd: HWND, region: HRGN) -> bool {
    if region.is_invalid() {
        return false;
    }
    if SetWindowRgn(hwnd, Some(region), true) == 0 {
        let _ = DeleteObject(region.into());
        return false;
    }
    true
}

unsafe fn subtract(region: HRGN, rect: &RECT) {
    let cut = CreateRectRgn(rect.left, rect.top, rect.right, rect.bottom);
    if !cut.is_invalid() {
        let _ = CombineRgn(Some(region), Some(region), Some(cut), RGN_DIFF);
        let _ = DeleteObject(cut.into());
    }
}

/// Whether the `covers` together hide all of `frame`.
fn fully_covered(frame: RECT, covers: &[RECT]) -> bool {
    unsafe {
        let region = CreateRectRgn(frame.left, frame.top, frame.right, frame.bottom);
        if region.is_invalid() {
            return false;
        }
        for cover in covers {
            subtract(region, cover);
        }
        let empty = CombineRgn(Some(region), Some(region), None, RGN_COPY) == NULLREGION;
        let _ = DeleteObject(region.into());
        empty
    }
}

fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// The frames (screen pixels) of the top-level windows above `target` in the z-order that the
/// user sees over it: visible, not minimized, not cloaked (another virtual desktop, a suspended
/// UWP app). Topmost windows count like the others, since they hide the target just the same.
/// Left out: the helper's own windows (the border itself, any notice beside it) and click-through
/// layered windows (another Deskpass helper's border, other apps' see-through HUDs), which
/// neither hide the target from the user nor take its clicks.
unsafe fn covers_above(target: HWND) -> Vec<RECT> {
    let own = std::process::id();
    let mut covers = Vec::new();
    let mut current = target;
    for _ in 0..4096 {
        let Ok(above) = GetWindow(current, GW_HWNDPREV) else { break };
        if above.0.is_null() {
            break;
        }
        current = above;
        if !IsWindowVisible(above).as_bool() || IsIconic(above).as_bool() {
            continue;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(above, Some(&mut pid));
        if pid == own {
            continue;
        }
        let ex = GetWindowLongPtrW(above, GWL_EXSTYLE) as u32;
        if ex & WS_EX_LAYERED.0 != 0 && ex & WS_EX_TRANSPARENT.0 != 0 {
            continue;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(above, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32).is_ok() && cloaked != 0 {
            continue;
        }
        // The visible frame, as for the target: a framed window's invisible resize borders hide nothing.
        let mut frame = RECT::default();
        let framed = DwmGetWindowAttribute(above, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut RECT as *mut _, size_of::<RECT>() as u32).is_ok()
            && frame.right > frame.left
            && frame.bottom > frame.top;
        if !framed && GetWindowRect(above, &mut frame).is_err() {
            continue;
        }
        if frame.right > frame.left && frame.bottom > frame.top {
            covers.push(frame);
        }
    }
    covers
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            follow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn follow(hwnd: HWND) {
    let target = UI.with(|ui| ui.borrow().target);
    if target.0.is_null() {
        return;
    }
    unsafe {
        if !flag(IsWindow(Some(target))) {
            hide_on_thread();
            return;
        }
    }
    let _ = place(hwnd, target);
    let notice = UI.with(|ui| ui.borrow().notice.hwnd);
    if !notice.0.is_null() {
        place_notice(notice, target);
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.0.is_null() {
            return;
        }
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let magenta = CreateSolidBrush(COLORREF(MAGENTA));
        let green = CreateSolidBrush(COLORREF(GREEN));
        let ink = CreateSolidBrush(COLORREF(INK));
        let _ = FillRect(hdc, &client, magenta);
        let label = RECT { left: 0, top: 0, right: client.right, bottom: LABEL_HEIGHT };
        let _ = FillRect(hdc, &label, ink);
        let width = client.right;
        let height = client.bottom;
        let strips = [
            RECT { left: 0, top: LABEL_HEIGHT, right: BORDER, bottom: height },
            RECT { left: width - BORDER, top: LABEL_HEIGHT, right: width, bottom: height },
            RECT { left: 0, top: LABEL_HEIGHT, right: width, bottom: LABEL_HEIGHT + BORDER },
            RECT { left: 0, top: height - BORDER, right: width, bottom: height },
        ];
        for strip in &strips {
            let _ = FillRect(hdc, strip, green);
        }
        let stock = GetStockObject(DEFAULT_GUI_FONT);
        if !stock.is_invalid() {
            let _ = SelectObject(hdc, stock);
        }
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0x00FF_FFFF));
        let mut text: Vec<u16> = label_text(foreground_border_shown()).encode_utf16().collect();
        let mut text_rect = label;
        let _ = DrawTextW(hdc, &mut text, &mut text_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        let _ = DeleteObject(magenta.into());
        let _ = DeleteObject(green.into());
        let _ = DeleteObject(ink.into());
        let _ = EndPaint(hwnd, &paint);
    }
}

/// The corner notice's measures at 96 DPI: its gap from the work area's edges, the green accent
/// on its left (the border's colour, so the two read as one), its padding and its text size.
const NOTICE_MARGIN: i32 = 16;
const NOTICE_ACCENT: i32 = 6;
const NOTICE_PAD_LEFT: i32 = 16;
const NOTICE_PAD_RIGHT: i32 = 14;
const NOTICE_PAD_Y: i32 = 8;
const NOTICE_POINTS: i32 = 11;

fn scaled(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

/// The notice window: like the border, a click-through (layered, transparent), topmost tool window
/// that never activates; opaque, so its accent is exactly the border's green.
fn create_notice() -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|error| error.to_string())?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(notice_proc),
            hInstance: windows::Win32::Foundation::HINSTANCE(instance.0),
            lpszClassName: w!("DESKPASS_NOTICE"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
            return Err("Could not register the notice window.".to_string());
        }
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("DESKPASS_NOTICE"),
            PCWSTR(wide_text(strings::text(strings::NOTICE_TITLE)).as_ptr()),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(windows::Win32::Foundation::HINSTANCE(instance.0)),
            None,
        )
        .map_err(|error| error.to_string())
        .and_then(|hwnd| {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA).map_err(|error| error.to_string())?;
            Ok(hwnd)
        })
    }
}

/// Ticket 144: puts the notice in the bottom-right corner of the work area of the monitor the
/// target is on (the work area leaves out the taskbar wherever it is docked), sized to its text at
/// that monitor's DPI. The follow timer calls this every 50 ms, so the notice moves with a target
/// dragged to another monitor; it is moved and repainted only when its place or text changed.
fn place_notice(hwnd: HWND, target: HWND) {
    unsafe {
        let monitor = MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let (mut dpi, mut unused) = (96u32, 96u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused).is_err() || dpi == 0 {
            dpi = 96;
        }
        let (text, font) = UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            if ui.notice.font.is_none() || ui.notice.dpi != dpi {
                if let Some(old) = ui.notice.font.take() {
                    let _ = DeleteObject(old.into());
                }
                let font = CreateFontW(
                    -scaled(NOTICE_POINTS * 96 / 72, dpi),
                    0,
                    0,
                    0,
                    FW_SEMIBOLD.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    0,
                    w!("Microsoft JhengHei UI"),
                );
                ui.notice.font = (!font.is_invalid()).then_some(font);
                ui.notice.dpi = dpi;
            }
            (ui.notice.text.clone(), ui.notice.font)
        });
        let (width, height) = measure(&text, font);
        let width = width + scaled(NOTICE_PAD_LEFT + NOTICE_PAD_RIGHT, dpi);
        let height = height + scaled(2 * NOTICE_PAD_Y, dpi);
        let work = info.rcWork;
        let margin = scaled(NOTICE_MARGIN, dpi);
        let right = work.right - margin;
        let bottom = work.bottom - margin;
        let rect = RECT { left: (right - width).max(work.left), top: (bottom - height).max(work.top), right, bottom };
        let changed = UI.with(|ui| ui.borrow().notice.placed.as_ref() != Some(&(rect, text.clone())));
        if changed {
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            wide.push(0);
            let _ = SetWindowTextW(hwnd, windows::core::PCWSTR(wide.as_ptr()));
        }
        // Raised every tick like the border, so another topmost window does not stay above it.
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        if changed {
            let _ = InvalidateRect(Some(hwnd), None, true);
            UI.with(|ui| ui.borrow_mut().notice.placed = Some((rect, text)));
        }
    }
}

/// The size of `text` on one line in `font`.
fn measure(text: &str, font: Option<HFONT>) -> (i32, i32) {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return (0, 0);
        }
        let previous = font.map(|font| SelectObject(dc, font.into()));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT::default();
        let _ = DrawTextW(dc, &mut wide, &mut rect, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        if let Some(previous) = previous {
            let _ = SelectObject(dc, previous);
        }
        let _ = DeleteDC(dc);
        (rect.right - rect.left, rect.bottom - rect.top)
    }
}

unsafe extern "system" fn notice_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_PAINT => {
            paint_notice(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn paint_notice(hwnd: HWND) {
    let (text, font, dpi) = UI.with(|ui| {
        let ui = ui.borrow();
        (ui.notice.text.clone(), ui.notice.font, ui.notice.dpi.max(96))
    });
    unsafe {
        let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.0.is_null() {
            return;
        }
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let ink = CreateSolidBrush(COLORREF(INK));
        let green = CreateSolidBrush(COLORREF(GREEN));
        let _ = FillRect(hdc, &client, ink);
        let accent = RECT { left: 0, top: 0, right: scaled(NOTICE_ACCENT, dpi), bottom: client.bottom };
        let _ = FillRect(hdc, &accent, green);
        if let Some(font) = font {
            let _ = SelectObject(hdc, font.into());
        }
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0x00FF_FFFF));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut text_rect = RECT { left: scaled(NOTICE_PAD_LEFT, dpi), top: 0, right: client.right, bottom: client.bottom };
        let _ = DrawTextW(hdc, &mut wide, &mut text_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        let _ = DeleteObject(ink.into());
        let _ = DeleteObject(green.into());
        let _ = EndPaint(hwnd, &paint);
    }
}

/// `text` as a NUL-terminated UTF-16 string; it must outlive the call it is passed to.
fn wide_text(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn flag(value: windows::core::BOOL) -> bool {
    value.0 != 0
}

#[cfg(test)]
mod tests {
    use super::{label_text_in, notice_text_in};
    use crate::strings::Lang;
    use serde_json::json;

    fn notice(lang: Lang, params: serde_json::Value) -> String {
        notice_text_in(lang, &params).unwrap()
    }

    #[test]
    fn the_notice_names_the_app_and_the_stop_key_of_the_delivery() {
        let zh = Lang::ZhTw;
        assert_eq!(notice(zh, json!({ "app_name": "記事本", "delivery_mode": "background" })), "AI 正在操作 記事本 · Ctrl+Alt+Esc 停止");
        assert_eq!(notice(zh, json!({ "app_name": "記事本", "delivery_mode": "foreground" })), "AI 正在操作 記事本 · Esc 停止");
        // Without a mode, the key that stops at any time; without a name, none.
        assert_eq!(notice(zh, json!({ "app_name": "記事本" })), "AI 正在操作 記事本 · Ctrl+Alt+Esc 停止");
        assert_eq!(notice(zh, json!({ "delivery_mode": "foreground" })), "AI 正在操作 · Esc 停止");
        assert_eq!(notice_text_in(zh, &json!({ "delivery_mode": "sideways" })).unwrap_err().0, "invalid_params");
    }

    #[test]
    fn the_english_notice_names_the_app_and_the_stop_key_of_the_delivery() {
        let en = Lang::En;
        assert_eq!(notice(en, json!({ "app_name": "Notepad", "delivery_mode": "background" })), "AI is operating Notepad · Ctrl+Alt+Esc to stop");
        assert_eq!(notice(en, json!({ "app_name": "Notepad", "delivery_mode": "foreground" })), "AI is operating Notepad · Esc to stop");
        assert_eq!(notice(en, json!({})), "AI is operating · Ctrl+Alt+Esc to stop");
        assert_eq!(notice(en, json!({ "delivery_mode": "foreground" })), "AI is operating · Esc to stop");
        assert_eq!(notice_text_in(en, &json!({ "delivery_mode": "sideways" })).unwrap_err().0, "invalid_params");
    }

    #[test]
    fn the_border_label_names_the_stop_key_of_the_delivery() {
        assert_eq!(label_text_in(Lang::ZhTw, false), "AI 正在操作 · 按 Ctrl+Alt+Esc 停止");
        assert_eq!(label_text_in(Lang::ZhTw, true), "AI 正在操作 · 按 Esc 停止");
        assert_eq!(label_text_in(Lang::En, false), "AI is operating · press Ctrl+Alt+Esc to stop");
        assert_eq!(label_text_in(Lang::En, true), "AI is operating · press Esc to stop");
    }
}
