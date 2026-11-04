use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetLayeredWindowAttributes, GetWindow, GetWindowRect, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE,
    GW_OWNER, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::windows_list::top_level_windows;

/// A popup the target opened itself, as the screenshot includes it and pointer input may reach it.
pub(crate) struct Popup {
    pub hwnd: HWND,
    /// `menu`, `tooltip`, `dropdown`, or `popup`.
    pub kind: &'static str,
    /// Visible frame in physical screen pixels.
    pub frame: RECT,
}

/// What kind of popup of `target` the top-level `window` is, or None when it is not one. A target
/// popup is visible, not cloaked, and belongs to the target's own UI thread (so to its process,
/// never another app's and never the helper's: the overlay and the approval dialog are excluded),
/// and is a classic menu (`#32768`), a tooltip (`tooltips_class32`), a combo box's dropdown list
/// (`ComboLBox`), or an unowned `WS_POPUP` tool window (`WS_EX_TOOLWINDOW`), which is how apps draw
/// their own menus and flyouts. These are the windows the app opens over or next to the target and
/// expects the pointer on, often unowned (Explorer's context menu), so the owner chain that admits
/// dialogs does not find them.
pub(crate) unsafe fn popup_kind(window: HWND, target: HWND) -> Option<&'static str> {
    if window.0.is_null() || window == target || !IsWindowVisible(window).as_bool() {
        return None;
    }
    let (mut pid, mut target_pid) = (0u32, 0u32);
    let thread = GetWindowThreadProcessId(window, Some(&mut pid));
    let target_thread = GetWindowThreadProcessId(target, Some(&mut target_pid));
    if thread == 0 || thread != target_thread || pid != target_pid || pid == std::process::id() {
        return None;
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(window, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32).is_ok() && cloaked != 0 {
        return None;
    }
    let mut buffer = [0u16; 64];
    let length = GetClassNameW(window, &mut buffer).max(0) as usize;
    let class = String::from_utf16_lossy(&buffer[..length]);
    if class == "#32768" {
        return Some("menu");
    }
    if class.eq_ignore_ascii_case("tooltips_class32") {
        return Some("tooltip");
    }
    if class.eq_ignore_ascii_case("ComboLBox") {
        return Some("dropdown");
    }
    let style = GetWindowLongPtrW(window, GWL_STYLE) as u32;
    let ex_style = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
    let unowned = GetWindow(window, GW_OWNER).map_or(true, |owner| owner.0.is_null());
    (style & WS_POPUP.0 != 0 && ex_style & WS_EX_TOOLWINDOW.0 != 0 && unowned).then_some("popup")
}

/// What kind of popup of `target` the top-level `window` is, as `popup_kind` judges it, when it
/// is also above the target in the z-order; None otherwise. A popup behind the target (a palette
/// or floating toolbar the app keeps under its main window) is not in the screenshot, so pointer
/// input does not count it as the target's either.
pub(crate) unsafe fn popup_above(window: HWND, target: HWND) -> Option<&'static str> {
    let kind = popup_kind(window, target)?;
    let windows = top_level_windows().ok()?;
    // EnumWindows lists top-level windows from the top of the z-order down.
    windows.into_iter().take_while(|listed| *listed != target).any(|listed| listed == window).then_some(kind)
}

/// The target's popups above it in the z-order that overlap or touch its visible `frame`, front
/// to back: what the screenshot shows next to the target, and where pointer input outside the
/// target may go. Popups behind the target are left out: the target covers them where they
/// share pixels, so drawing them over it would show what is not there. So are generic popups
/// that show nothing (`shows_nothing`), which would only stretch the canvas.
pub(crate) fn target_popups(target: HWND, frame: RECT) -> Vec<Popup> {
    let Ok(windows) = top_level_windows() else { return Vec::new() };
    windows
        .into_iter()
        // EnumWindows lists top-level windows from the top of the z-order down.
        .take_while(|window| *window != target)
        .filter_map(|window| {
            let kind = unsafe { popup_kind(window, target) }?;
            if kind == "popup" && unsafe { shows_nothing(window) } {
                return None;
            }
            let popup = unsafe { popup_frame(window) }?;
            let touches = popup.right > popup.left
                && popup.bottom > popup.top
                && popup.left <= frame.right
                && popup.right >= frame.left
                && popup.top <= frame.bottom
                && popup.bottom >= frame.top;
            touches.then_some(Popup { hwnd: window, kind, frame: popup })
        })
        .collect()
}

/// Whether a generic popup is a layer that shows nothing, like an app's drag or full-screen
/// helper layer: click-through (`WS_EX_TRANSPARENT`), or layered with a constant alpha of 0. A
/// layered window drawn with `UpdateLayeredWindow` has no readable attributes and counts as
/// showing something. Menus, tooltips and dropdowns are not asked: they fade in from alpha 0.
unsafe fn shows_nothing(window: HWND) -> bool {
    let ex_style = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TRANSPARENT.0 != 0 {
        return true;
    }
    if ex_style & WS_EX_LAYERED.0 == 0 {
        return false;
    }
    let (mut key, mut alpha, mut flags) = (COLORREF(0), 0u8, LAYERED_WINDOW_ATTRIBUTES_FLAGS(0));
    GetLayeredWindowAttributes(window, Some(&mut key), Some(&mut alpha), Some(&mut flags)).is_ok() && flags.0 & LWA_ALPHA.0 != 0 && alpha == 0
}

/// A popup's visible frame: the DWM extended frame bounds, or its window rect when DWM has none
/// for it (menus and tooltips often have none; that is expected, so it is not logged).
unsafe fn popup_frame(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    if DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut RECT as *mut _, size_of::<RECT>() as u32).is_ok() && rect.right > rect.left && rect.bottom > rect.top {
        return Some(rect);
    }
    GetWindowRect(window, &mut rect).ok()?;
    Some(rect)
}
