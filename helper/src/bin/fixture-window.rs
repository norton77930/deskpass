use std::cell::RefCell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, ScreenToClient, COLOR_BTNFACE, HBRUSH,
    PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WM_TIMER};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, SetWindowsHookExW, CWPSTRUCT, WH_CALLWNDPROC, WM_IME_CONTROL};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetKeyState, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DialogBoxIndirectParamW, DispatchMessageW, EndDialog, GetDlgItem, GetMessageW, GetWindowTextLengthW, GetWindowTextW,
    DestroyWindow, PostQuitMessage, DLGTEMPLATE, ES_AUTOHSCROLL, WM_INITDIALOG, WS_TABSTOP,
    ChildWindowFromPoint, RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, HMENU, SW_MAXIMIZE, WINDOW_EX_STYLE, WINDOW_STYLE,
    WNDCLASSEXW, WNDPROC, GWLP_WNDPROC, MSG, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDBLCLK, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WM_CHAR, WM_CONTEXTMENU, WM_ENTERIDLE, WM_ENTERMENULOOP, AppendMenuW, CreatePopupMenu, DestroyMenu, GetMenuItemRect, TrackPopupMenu, MF_STRING, MSGF_MENU,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, GetForegroundWindow,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST, WM_RBUTTONDOWN, WS_EX_TOPMOST, SW_SHOWNOACTIVATE, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_PAINT, WM_RBUTTONUP, WM_SIZE, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN,
    WS_CAPTION, WS_EX_APPWINDOW, WS_MAXIMIZEBOX, WS_OVERLAPPED, WS_POPUP, WS_THICKFRAME, WS_VISIBLE, WS_VSCROLL,
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SW_SHOWNA, GetClientRect, WM_ERASEBKGND,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
use windows::Win32::UI::WindowsAndMessaging::{GetDlgCtrlID, GetParent};
use windows::Win32::UI::WindowsAndMessaging::{InSendMessage, WM_GETTEXT, WM_GETTEXTLENGTH, WM_SETFOCUS, WM_SETTEXT};
use windows::Win32::UI::WindowsAndMessaging::{SetParent, GWL_STYLE, MA_NOACTIVATE, WM_APP, WM_MOUSEACTIVATE, WM_INPUTLANGCHANGEREQUEST, WM_WINDOWPOSCHANGING};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::WindowsAndMessaging::{GetMessageTime, MsgWaitForMultipleObjects, PeekMessageW, PM_NOREMOVE, PM_QS_SENDMESSAGE, QS_SENDMESSAGE};
use windows::Win32::UI::WindowsAndMessaging::{
    SetLayeredWindowAttributes, SetWindowPos, HWND_TOP, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::core::Interface;
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::UI::WindowsAndMessaging::WS_EX_NOREDIRECTIONBITMAP;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

const EDIT_ID: isize = 1001;
const BUTTON_ID: isize = 1002;
const STATUS_ID: isize = 1003;
const DIALOG_BUTTON_ID: isize = 1005;
const COMBO_ID: isize = 1006;
const DIALOG_EDIT_ID: u16 = 2001;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CBN_SELCHANGE: usize = 1;
const CBN_DROPDOWN: usize = 7;
const IDOK: usize = 1;
const IDCANCEL: usize = 2;
const BN_CLICKED: usize = 0;
const EN_CHANGE: usize = 0x0300;
const VK_ESCAPE: usize = 0x1B;
const HTCLIENT: isize = 1;
/// Side length of each edge target in `--frame` mode, in physical pixels.
const TARGET: i32 = 8;

struct Fixture {
    hwnd: HWND,
    edit: HWND,
    status: HWND,
    button_proc: WNDPROC,
    edit_proc: WNDPROC,
    events: Vec<String>,
    scroll_pos: i32,
    drag_start: Option<(i32, i32)>,
    /// Modifiers held when the drag's button went down, as `held_modifiers` names them.
    drag_modifiers: String,
    /// Every vertical and horizontal wheel event the scroll area received, as counts rather than
    /// a list (`events` keeps only the last few).
    wheel_vertical: WheelStats,
    wheel_horizontal: WheelStats,
}

/// How many wheel events arrived on one axis, their total, and the smallest and largest delta.
#[derive(Clone, Copy, Default)]
struct WheelStats {
    count: u32,
    sum: i32,
    min: i32,
    max: i32,
}

impl WheelStats {
    fn add(&mut self, delta: i32) {
        if self.count == 0 {
            self.min = delta;
            self.max = delta;
        } else {
            self.min = self.min.min(delta);
            self.max = self.max.max(delta);
        }
        self.count += 1;
        self.sum += delta;
    }
}

thread_local! {
    static STATE: RefCell<Option<Fixture>> = const { RefCell::new(None) };
    static FRAME_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--slow-input <ms>`: the edit sleeps this long on every character it handles, like an
    /// app that is slow to process input (0: no delay).
    static SLOW_INPUT_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// `--slow-keys <ms>`: the edit sleeps this long after handling every key press, recording
    /// `slow:<vk>` (decimal virtual-key code) first, so a test can see which press it is busy
    /// with while the next ones wait in the queue (0: no delay).
    static SLOW_KEYS_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// `--slow-ime <ms>`: the thread sleeps this long before handling a request that opens or
    /// closes its input method, like an app busy when the IME request arrives (0: no delay).
    static SLOW_IME_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// `--untitled-dialog`: the Dialog button's dialog has no title, like the save prompts that
    /// Paint (`XAMLModalWindow`) and Office (`NUIDialog`) open, and a visible window with no title
    /// and no owner (class `CUMCP_STRAY`) stands beside the fixture, like an app's background
    /// window.
    static UNTITLED_DIALOG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--popup-menu`: a right-click on the fixture's background opens a classic `#32768` menu
    /// (`TrackPopupMenu`) that sticks out past the fixture's left and bottom edges.
    static POPUP_MENU: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--flyout`: the hidden flyout a right-click on the fixture's background shows (0 without
    /// `--flyout`), see `create_flyout`.
    static FLYOUT: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    /// The menu `--popup-menu` has open, and whether its last item's rect was recorded yet.
    static OPEN_MENU: std::cell::Cell<Option<(isize, bool)>> = const { std::cell::Cell::new(None) };
    /// `--click-handoff new|close`: what the fixture does when a left button is released on its
    /// own background (not on a child control), after recording `bg:up@<x>,<y>` (client pixels);
    /// each press there records `bg:down`.
    static CLICK_HANDOFF: std::cell::Cell<Handoff> = const { std::cell::Cell::new(Handoff::None) };
    /// The window `--click-handoff new` shows, created hidden at startup.
    static HANDOFF_WINDOW: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    /// `--slow-raise <ms>`: once armed by a sent `ARM_SLOW_RAISE`, the fixture window sleeps this
    /// long in the next WM_WINDOWPOSCHANGING it gets, like an app that does not answer a raise
    /// (`SetWindowPos` from another thread waits for that message); then it is disarmed again.
    static SLOW_RAISE_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SLOW_RAISE_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--close-on-char <c>` (ticket 52, F32): once the edit has handled character `<c>`, the
    /// fixture closes its window (a posted WM_CLOSE, taken before the next input), the only window
    /// of its thread; the thread then stays alive without any window for `CLOSED_LINGER` before
    /// the process exits, like an app whose last window the typing closed, still shutting down.
    static CLOSE_ON_CHAR: std::cell::Cell<Option<char>> = const { std::cell::Cell::new(None) };
    /// `--slow-pointer <ms>` (ticket 53, F35): the scroll area, and the background in
    /// `--click-handoff` mode, stay busy this long after handling every button press and wheel
    /// notch, recording `slow:down` or `slow:wheel` first, so the release or the next notch waits
    /// in the queue meanwhile (0: no delay). With `--slow-pointer-pumps` the delay answers sent
    /// messages (`busy_pump`), like an app that pumps them during its input handling.
    static SLOW_POINTER_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SLOW_POINTER_PUMPS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--ui-controls` (ticket 59): the fixture also has a check box "Remember me" and a password
    /// box labelled "Password:" holding `PASSWORD_TEXT`, below the scroll area and right of the
    /// Dialog button, for the UI Automation tree tests. Off by default, so the points other tests
    /// click on the background stay background.
    static UI_CONTROLS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--ui-content` (ticket 60): the fixture is 960 wide instead of 480 and its right half holds
    /// more controls for the UI Automation tree tests, see `create_ui_content`. The left half is
    /// as without it.
    static UI_CONTENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--ui-groups <n>` (ticket 60, with `--ui-content`): how many containers of
    /// `GROUP_BUTTONS` tiny buttons the right half has (1 without it).
    static UI_GROUPS: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    /// `--ui-lists <n>` (ticket 60, with `--ui-content`): how many list boxes of `TINY_ROWS`
    /// visible 2-pixel rows the right half has below the containers (0 without it). A list's rows
    /// are cheaper to read than child windows, so these make a tree past 500 lines quickly.
    static UI_LISTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// `--hang-name <ms>` (ticket 59): the first time another thread asks the check box for its
    /// text (as UI Automation does to read its name), the fixture's UI thread sleeps this long, like
    /// an app that hangs while its tree is read (0: never).
    static HANG_NAME_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// `--hang-nested <ms>` (ticket 69, with `--ui-section`): the first time another thread asks the
    /// section's button for its text, the UI thread sleeps this long, like `--hang-name` but for a
    /// control one level down: the section itself, and every control outside it, answer at once, so
    /// only the call that fetches the section's children hangs (0: never).
    static HANG_NESTED_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The section button's own window procedure, which `nested_subclass` passes messages on to.
    static NESTED_PROC: std::cell::Cell<WNDPROC> = const { std::cell::Cell::new(None) };
    /// The hang `ARM_SET_TEXT_HANG` armed (0: none).
    static SET_TEXT_HANG_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The hang `ARM_COMMAND_HANG` armed (0: none).
    static COMMAND_HANG_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The hang `ARM_QUERY_HANG` armed (0: none).
    static QUERY_HANG_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The check box's own window procedure, which `check_subclass` passes messages on to.
    static CHECK_PROC: std::cell::Cell<WNDPROC> = const { std::cell::Cell::new(None) };
    /// `--ui-section` (ticket 62): the fixture also has a section that closes when its button is
    /// pressed, see `create_ui_section`.
    static UI_SECTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--ui-read-only` (ticket 64): the fixture also has a read-only edit holding
    /// `READ_ONLY_TEXT`, see `create_read_only`.
    static UI_READ_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--ui-text` (reference-gaps G4, with `--ui-content`): the fixture also has a Rich Edit holding
    /// `UI_TEXT` with `UI_TEXT_SELECTION` selected, see `create_ui_text`.
    static UI_TEXT_BOX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--ui-text-long <segments>` (ticket 08, with `--ui-text`): how many segments of `long_ui_text`
    /// that Rich Edit holds instead of `UI_TEXT`; 0 without the flag.
    static UI_TEXT_LONG: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// `--ui-scroll-tree` (ticket 65, with `--ui-content`): the fixture also has a tree view whose
    /// fifth item shows only its top half, see `create_scroll_tree`.
    static UI_SCROLL_TREE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--paste-area` (ticket 77): the scroll area takes the keyboard focus when clicked and, on
    /// Ctrl+V, reads the clipboard's text as an app without a Value pattern would (`area_paste`).
    static PASTE_AREA: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--edit-keeps-focus` (ticket 138): activating the window puts the keyboard focus back in
    /// the edit, as an editor does, so every key typed while it is in front lands in the edit
    /// (its `len=` counts them). Without it, activation focuses the window itself.
    static EDIT_KEEPS_FOCUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `--list-ignores-wheel` (reference-gaps G5, with `--ui-content`): the list box swallows
    /// WM_MOUSEWHEEL and WM_MOUSEHWHEEL, like an app (Edge) that ignores a posted wheel message
    /// while its UI Automation ScrollPattern still scrolls it.
    static LIST_IGNORES_WHEEL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The list box's own window procedure, which `list_subclass` passes messages on to.
    static LIST_PROC: std::cell::Cell<WNDPROC> = const { std::cell::Cell::new(None) };
}

/// The text of the `--ui-controls` password box, which must never appear in a UI tree.
const PASSWORD_TEXT: &str = "CUMCP-secret-42";
const CHECK_ID: isize = 1007;
const PASSWORD_LABEL_ID: isize = 1008;
const PASSWORD_ID: isize = 1009;
const BS_AUTOCHECKBOX: u32 = 0x3;
const ES_PASSWORD: u32 = 0x20;
const ES_READONLY: u32 = 0x0800;
const READ_ONLY_ID: isize = 1010;
/// The text of the `--ui-read-only` edit.
const READ_ONLY_TEXT: &str = "Fixed text";
const DISABLED_EDIT_ID: isize = 1011;
/// The text of the `--ui-read-only` disabled edit.
const DISABLED_TEXT: &str = "Off text";
/// `--paste-moves-focus` (ticket 77): the edit that gets the focus, and the clipboard's text, when
/// Ctrl+V is pressed in the main edit box (`move_paste_focus`).
const MOVED_EDIT_ID: isize = 1012;

/// `--slow-pointer`: records `slow:<what>` and stays busy, after a button press or wheel notch
/// was handled.
fn slow_pointer(what: &str) {
    let ms = SLOW_POINTER_MS.with(|slow| slow.get());
    if ms == 0 {
        return;
    }
    record(&format!("slow:{what}"));
    if SLOW_POINTER_PUMPS.with(|pumps| pumps.get()) {
        busy_pump(ms);
    } else {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// How long `--close-on-char` keeps the fixture's thread alive after its window closed.
const CLOSED_LINGER: std::time::Duration = std::time::Duration::from_secs(5);

/// Sent by a test to arm `--slow-raise`, after the fixture was brought to the front normally.
const ARM_SLOW_RAISE: u32 = WM_APP + 1;
/// Ticket 64 (m5): posted by a test, the fixture's UI thread stops for `wparam` milliseconds when
/// it takes this message, answering nothing meanwhile, like an app that hangs.
const HANG_UI: u32 = WM_APP + 2;
/// Ticket 64: sent by a test, arms a hang of `wparam` milliseconds: the next WM_SETTEXT another
/// thread sends the main edit (as UI Automation's `SetValue` does) stops the UI thread that long
/// before the edit takes the text, recording `hang:settext` first. An app that hangs only once an
/// action reaches it, so the checks before the action (the foreground, a dialog) still pass.
const ARM_SET_TEXT_HANG: u32 = WM_APP + 3;
/// Ticket 65: sent by a test, arms a hang of `wparam` milliseconds: the next notification of a
/// button click (BN_CLICKED from any of the window's buttons or its check box, as UI Automation's
/// Invoke and Toggle cause) or of a combo box opening (CBN_DROPDOWN, as Expand causes) stops the
/// UI thread that long, recording `hang:command` first. Like `ARM_SET_TEXT_HANG`, the app hangs
/// only once the action reaches it. A Win32 button's Invoke and Toggle post their click, so their
/// call returns and the hang holds what comes next (the read-back); a combo box's Expand waits.
const ARM_COMMAND_HANG: u32 = WM_APP + 4;

/// Ticket 65 review: sent by a test, arms a hang of `wparam` milliseconds: the next time another
/// thread asks the check box for its check state (BM_GETCHECK or BM_GETSTATE, as UI Automation
/// reads a toggle state before toggling), the UI thread stops that long, recording `hang:query`
/// first. The app hangs before any action reaches it.
const ARM_QUERY_HANG: u32 = WM_APP + 5;
const BM_GETCHECK: u32 = 0x00F0;
const BM_GETSTATE: u32 = 0x00F2;

/// `--click-handoff`: `new` shows an unowned top-level window of this thread (class and title
/// `CUMCP_HANDOFF`, 200x100 at screen (640, 400), clear of the fixture) and gives it the
/// foreground, like Chrome's profile picker opening a browser window; `close` destroys the
/// fixture, so the process exits, like a picker closing itself. The window is created hidden at
/// startup so the hand-off is quick (creating it on the click took about 50 ms in a debug
/// build), and the trigger is the fixture's background rather than the "Click me" button (a
/// press on the button moves the focus off the edit and its input method, which delayed
/// handling the click by tens of ms): a double click's second click is due 40 ms after the first.
#[derive(Clone, Copy, PartialEq)]
enum Handoff {
    None,
    NewWindow,
    Close,
}

fn click_handoff_arg() -> Handoff {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().position(|arg| arg == "--click-handoff").and_then(|index| args.get(index + 1)).map(String::as_str) {
        Some("new") => Handoff::NewWindow,
        Some("close") => Handoff::Close,
        _ => Handoff::None,
    }
}

fn hand_off(fixture: HWND) {
    unsafe {
        match CLICK_HANDOFF.with(|handoff| handoff.get()) {
            Handoff::None => {}
            Handoff::Close => {
                let _ = DestroyWindow(fixture);
            }
            Handoff::NewWindow => {
                let window = HWND(HANDOFF_WINDOW.with(|cell| cell.get()) as *mut core::ffi::c_void);
                let _ = ShowWindow(window, SW_SHOWNA);
                let _ = SetForegroundWindow(window);
            }
        }
    }
}

fn handoff_mode() -> bool {
    CLICK_HANDOFF.with(|handoff| handoff.get()) != Handoff::None || HANDOFF_ON.with(|trigger| trigger.get()).is_some() || HANDED_OFF.with(|done| done.get())
}

fn create_handoff_window(instance: windows::Win32::Foundation::HMODULE) {
    unsafe {
        register_class(instance, w!("CUMCP_HANDOFF"), Some(stray_proc));
        let window = CreateWindowExW(
            WS_EX_APPWINDOW,
            w!("CUMCP_HANDOFF"),
            w!("CUMCP_HANDOFF"),
            WS_POPUP | WS_BORDER,
            640,
            400,
            200,
            100,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("hand-off window");
        HANDOFF_WINDOW.with(|cell| cell.set(window.0 as isize));
    }
}

/// Ticket 49: `--handoff-on <trigger>` shows a top-level window of another thread of this process
/// (class and title `CUMCP_HANDOFF`, 200x100 at screen (640, 400), clear of the fixture) and gives
/// it the foreground once, on the first input that matches `trigger`, after the fixture handled
/// that input: `down` a left press on the fixture's background; `wheel:<n>` the scroll area's
/// wheel event that brings its notches (deltas of 120, summed) to `n` or past it; `key:<vk>` a press of virtual key `<vk>` (decimal) the edit reads, here
/// before the edit handles it; `char:<c>` character `<c>` typed into the edit; `move` a pointer
/// move onto the background's right end (client x 440 or more). Being another thread's, the window takes the keys sent after
/// the hand-off (the fixture's thread never reads them), like a browser window of another process.
/// Its title reports the key presses (`key:<vk>`), characters (`char:<c>`) and wheel notches
/// (`wheel`) it received: `CUMCP_HANDOFF | ev=<events>`, plain `CUMCP_HANDOFF` until then. As
/// with `--click-handoff`, presses and releases on the background are recorded (`bg:down`,
/// `bg:up@<x>,<y>`).
#[derive(Clone, Copy, PartialEq)]
enum Trigger {
    Press,
    Wheel(u32),
    Key(usize),
    Char(char),
    Move,
}

thread_local! {
    static HANDOFF_ON: std::cell::Cell<Option<Trigger>> = const { std::cell::Cell::new(None) };
    /// Whether the `--handoff-on` trigger has fired.
    static HANDED_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The `CUMCP_HANDOFF` window's events, on its own thread.
    static OTHER_EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The `--handoff-on` window, created hidden on its own thread at startup.
static OTHER_THREAD_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn handoff_on_arg() -> Option<Trigger> {
    let args: Vec<String> = std::env::args().collect();
    let value = args.iter().position(|arg| arg == "--handoff-on").and_then(|index| args.get(index + 1))?;
    match value.split_once(':') {
        None if value == "down" => Some(Trigger::Press),
        None if value == "move" => Some(Trigger::Move),
        Some(("wheel", n)) => n.parse().ok().map(Trigger::Wheel),
        Some(("key", vk)) => vk.parse().ok().map(Trigger::Key),
        Some(("char", c)) => c.chars().next().map(Trigger::Char),
        _ => None,
    }
}

/// Hands the foreground to the `--handoff-on` window when `input` is its trigger, once.
fn hand_off_on(input: Trigger) {
    if HANDOFF_ON.with(|trigger| trigger.get()) != Some(input) {
        return;
    }
    HANDOFF_ON.with(|trigger| trigger.set(None));
    HANDED_OFF.with(|done| done.set(true));
    let window = HWND(OTHER_THREAD_WINDOW.load(std::sync::atomic::Ordering::SeqCst) as *mut core::ffi::c_void);
    unsafe {
        let _ = ShowWindow(window, SW_SHOWNA);
        let _ = SetForegroundWindow(window);
    }
}

fn open_other_thread_window() {
    std::thread::spawn(|| unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        register_class(instance, w!("CUMCP_HANDOFF"), Some(other_thread_proc));
        let window = CreateWindowExW(
            WS_EX_APPWINDOW,
            w!("CUMCP_HANDOFF"),
            w!("CUMCP_HANDOFF"),
            WS_POPUP | WS_BORDER,
            640,
            400,
            200,
            100,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("hand-off window");
        OTHER_THREAD_WINDOW.store(window.0 as isize, std::sync::atomic::Ordering::SeqCst);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
    while OTHER_THREAD_WINDOW.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

unsafe extern "system" fn other_thread_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let event = match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => format!("key:{}", wparam.0),
        WM_CHAR => format!("char:{}", char::from_u32(wparam.0 as u32).unwrap_or('\u{FFFD}')),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => "wheel".to_string(),
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    };
    let events = OTHER_EVENTS.with(|events| {
        let mut events = events.borrow_mut();
        events.push(event);
        events.join(",")
    });
    set_text(hwnd, &format!("CUMCP_HANDOFF | ev={events}"));
    LRESULT(0)
}

/// `--busy-pump <ms>` (ticket 49, F11): the edit stays busy this long on every character it
/// handles, after the character is in the edit, while still answering sent messages, like an app
/// that pumps sent messages during its input handling. The title then ends with
/// ` | ahead=<n>,<n>,...`: for each key press the edit read, in order, how many characters typed
/// before it were still being handled when that press was sent (its message time), that is, how
/// far the typing ran ahead of the edit.
fn busy_pump(ms: u64) {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        unsafe {
            let _ = MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_SENDMESSAGE);
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE);
        }
    }
}

thread_local! {
    static BUSY_PUMP_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// When each character's handling ended (`GetTickCount`), in order.
    static BUSY_DONE: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// `ahead` for each key press read, in order.
    static BUSY_AHEAD: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn busy_field() -> String {
    if BUSY_PUMP_MS.with(|busy| busy.get()) == 0 {
        return String::new();
    }
    let ahead: Vec<String> = BUSY_AHEAD.with(|ahead| ahead.borrow().iter().map(usize::to_string).collect());
    format!(" | ahead={}", ahead.join(","))
}

/// Items of the `--popup-menu` menu; the command of item `i` (0-based) is `MENU_FIRST_ID + i`.
const MENU_ITEMS: u32 = 8;
const MENU_FIRST_ID: u32 = 101;

/// Environment variables, for a launch that cannot pass arguments (`launch_app` starts the
/// executable bare, and the helper hands its environment on).
/// `CUMCP_FIXTURE_TRAMPOLINE=<code>`: act as a launcher, like `explorer.exe` handing its window
/// to the shell: start a copy of this executable in normal mode, wait until its window is
/// visible, then exit with `<code>`. With `CUMCP_FIXTURE_TRAMPOLINE_NO_SPAWN` set, exit with
/// `<code>` at once and open nothing.
/// `CUMCP_FIXTURE_OWNED_POPUP`: an untitled popup owned by the fixture window is in front of it
/// from the moment the fixture window becomes visible.
/// `CUMCP_FIXTURE_LAUNCH_DIALOG=<ms>|only` (ticket 79): the fixture window starts hidden and a
/// dialog it owns (title `CUMCP_LAUNCH_DIALOG`) shows first, like an app's sign-in or profile prompt
/// before its main window; the fixture window shows `<ms>` milliseconds later, or never with `only`.
/// While the owner is hidden, `list_windows` lists the dialog without an `owner_window_id`.
const TRAMPOLINE_VAR: &str = "CUMCP_FIXTURE_TRAMPOLINE";
const TRAMPOLINE_NO_SPAWN_VAR: &str = "CUMCP_FIXTURE_TRAMPOLINE_NO_SPAWN";
const OWNED_POPUP_VAR: &str = "CUMCP_FIXTURE_OWNED_POPUP";
const LAUNCH_DIALOG_VAR: &str = "CUMCP_FIXTURE_LAUNCH_DIALOG";

/// `CUMCP_FIXTURE_LAUNCH_DIALOG`: None without it, `Some(None)` for `only`, else the delay.
fn launch_dialog_var() -> Option<Option<u64>> {
    let value = std::env::var(LAUNCH_DIALOG_VAR).ok()?;
    if value == "only" {
        return Some(None);
    }
    value.parse().ok().map(Some)
}

/// Shows the `CUMCP_FIXTURE_LAUNCH_DIALOG` dialog owned by the hidden `fixture`, then the fixture
/// after `delay` (never without one).
fn open_launch_dialog(instance: windows::Win32::Foundation::HMODULE, fixture: HWND, delay: Option<u64>) {
    unsafe {
        register_class(instance, w!("CUMCP_LAUNCH_DIALOG"), Some(stray_proc));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("CUMCP_LAUNCH_DIALOG"),
            w!("CUMCP_LAUNCH_DIALOG"),
            WS_POPUP | WS_CAPTION | WS_BORDER | WS_VISIBLE,
            640,
            400,
            240,
            120,
            Some(fixture),
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("launch dialog");
    }
    let Some(delay) = delay else {
        return;
    };
    let fixture = fixture.0 as isize;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay));
        unsafe {
            let _ = ShowWindow(HWND(fixture as *mut core::ffi::c_void), SW_SHOWNA);
        }
    });
}

fn run_trampoline(code: i32) -> ! {
    if std::env::var_os(TRAMPOLINE_NO_SPAWN_VAR).is_none() {
        let child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .env_remove(TRAMPOLINE_VAR)
            .env_remove(TRAMPOLINE_NO_SPAWN_VAR)
            .spawn()
            .expect("spawn the fixture window");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline && !has_visible_window(child.id()) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    std::process::exit(code);
}

fn has_visible_window(pid: u32) -> bool {
    struct Search {
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        if owner == search.pid && IsWindowVisible(hwnd).as_bool() {
            search.found = true;
            return windows::core::BOOL(0);
        }
        windows::Win32::Foundation::TRUE
    }
    let mut search = Search { pid, found: false };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

fn main() {
    if let Some(code) = std::env::var(TRAMPOLINE_VAR).ok().and_then(|value| value.parse::<i32>().ok()) {
        run_trampoline(code);
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).expect("module");
        register_class(instance, w!("CUMCP_FIXTURE"), Some(fixture_proc));
        register_class(instance, w!("CUMCP_SCROLL"), Some(scroll_proc));
        if let Some(parent) = core_window_parent() {
            run_core_window_child(instance, parent);
            return;
        }
        if let Some(rect) = cover_rect() {
            let tool = std::env::args().any(|arg| arg == "--tool-window");
            run_cover(instance, rect, !std::env::args().any(|arg| arg == "--not-topmost"), tool);
            return;
        }
        if let Some(mode) = present_mode() {
            run_present(instance, mode);
            return;
        }
        let no_activate = std::env::args().any(|arg| arg == "--no-activate");
        let frame = std::env::args().any(|arg| arg == "--frame");
        let maximized = std::env::args().any(|arg| arg == "--maximized");
        FRAME_MODE.with(|mode| mode.set(frame));
        let untitled_dialog = std::env::args().any(|arg| arg == "--untitled-dialog");
        UNTITLED_DIALOG.with(|untitled| untitled.set(untitled_dialog));
        POPUP_MENU.with(|popup| popup.set(std::env::args().any(|arg| arg == "--popup-menu")));
        let handoff = click_handoff_arg();
        CLICK_HANDOFF.with(|cell| cell.set(handoff));
        if handoff == Handoff::NewWindow {
            create_handoff_window(instance);
        }
        let trigger = handoff_on_arg();
        HANDOFF_ON.with(|cell| cell.set(trigger));
        if trigger.is_some() {
            open_other_thread_window();
        }
        BUSY_PUMP_MS.with(|busy| busy.set(millis_arg("--busy-pump")));
        SLOW_INPUT_MS.with(|slow| slow.set(millis_arg("--slow-input")));
        SLOW_KEYS_MS.with(|slow| slow.set(millis_arg("--slow-keys")));
        SLOW_RAISE_MS.with(|slow| slow.set(millis_arg("--slow-raise")));
        SLOW_POINTER_MS.with(|slow| slow.set(millis_arg("--slow-pointer")));
        SLOW_POINTER_PUMPS.with(|pumps| pumps.set(std::env::args().any(|arg| arg == "--slow-pointer-pumps")));
        UI_CONTROLS.with(|controls| controls.set(std::env::args().any(|arg| arg == "--ui-controls")));
        let ui_content = std::env::args().any(|arg| arg == "--ui-content");
        UI_CONTENT.with(|content| content.set(ui_content));
        UI_GROUPS.with(|groups| groups.set(millis_arg("--ui-groups").max(1)));
        UI_LISTS.with(|lists| lists.set(millis_arg("--ui-lists")));
        if ui_content {
            register_class(instance, w!("CUMCP_MANY"), Some(many_proc));
        }
        let ui_section = std::env::args().any(|arg| arg == "--ui-section");
        UI_SECTION.with(|section| section.set(ui_section));
        UI_READ_ONLY.with(|read_only| read_only.set(std::env::args().any(|arg| arg == "--ui-read-only")));
        UI_SCROLL_TREE.with(|tree| tree.set(std::env::args().any(|arg| arg == "--ui-scroll-tree")));
        UI_TEXT_BOX.with(|text| text.set(std::env::args().any(|arg| arg == "--ui-text")));
        UI_TEXT_LONG.with(|long| long.set(std::env::args().skip_while(|arg| arg != "--ui-text-long").nth(1).and_then(|value| value.parse().ok()).unwrap_or(0)));
        PASTE_AREA.with(|area| area.set(std::env::args().any(|arg| arg == "--paste-area")));
        EDIT_KEEPS_FOCUS.with(|keeps| keeps.set(std::env::args().any(|arg| arg == "--edit-keeps-focus")));
        LIST_IGNORES_WHEEL.with(|ignores| ignores.set(std::env::args().any(|arg| arg == "--list-ignores-wheel")));
        if ui_section {
            register_class(instance, w!("CUMCP_SECTION"), Some(section_proc));
        }
        HANG_NAME_MS.with(|hang| hang.set(millis_arg("--hang-name")));
        HANG_NESTED_MS.with(|hang| hang.set(millis_arg("--hang-nested")));
        let close_on_char = std::env::args().skip_while(|arg| arg != "--close-on-char").nth(1).and_then(|value| value.chars().next());
        CLOSE_ON_CHAR.with(|close| close.set(close_on_char));
        let slow_ime = millis_arg("--slow-ime");
        SLOW_IME_MS.with(|slow| slow.set(slow_ime));
        if slow_ime > 0 {
            // A hook on this thread only: it sees the requests sent to the thread's IME window.
            let _ = SetWindowsHookExW(WH_CALLWNDPROC, Some(slow_ime_hook), None, GetCurrentThreadId());
        }
        // `--chromium-class top|child` (ticket 66): the fixture window itself, or a small child window
        // of it, has the class Chromium and Electron give their windows. The fixture has no web area
        // (no Document element), which is what a Chromium app that has not built its accessibility
        // tree yet looks like.
        let chromium_class = std::env::args().skip_while(|arg| arg != "--chromium-class").nth(1);
        let main_class = match chromium_class.as_deref() {
            Some("top") => {
                register_class(instance, w!("Chrome_WidgetWin_1"), Some(fixture_proc));
                w!("Chrome_WidgetWin_1")
            }
            Some("child") => {
                register_class(instance, w!("Chrome_WidgetWin_1"), Some(stray_proc));
                w!("CUMCP_FIXTURE")
            }
            _ => w!("CUMCP_FIXTURE"),
        };
        let mut ex_style = WS_EX_APPWINDOW;
        if no_activate {
            ex_style |= WS_EX_NOACTIVATE;
        }
        // `--frame` uses a standard captioned, resizable frame. Windows gives it invisible
        // resize borders, so GetWindowRect is larger than the visible frame. There is no
        // system menu, so no caption buttons: the system handles clicks on those itself, and
        // an edge target must never close the fixture. WS_MAXIMIZEBOX keeps a maximized
        // fixture inside the work area, like a normal app, instead of under the taskbar.
        let owned_popup = std::env::var_os(OWNED_POPUP_VAR).is_some();
        let (style, width, height) = if frame {
            (WS_OVERLAPPED | WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZEBOX | WS_VISIBLE | WS_CLIPCHILDREN, 640, 480)
        } else {
            (WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN, if ui_content { 960 } else { 480 }, 360)
        };
        // With the owned popup, the fixture window starts hidden: a popup is only listed once its
        // owner is visible, so showing the owner lists both at once, popup in front (owned
        // windows stay above their owner).
        let launch_dialog = launch_dialog_var();
        let style = if owned_popup || launch_dialog.is_some() { WINDOW_STYLE(style.0 & !WS_VISIBLE.0) } else { style };
        let _hwnd = CreateWindowExW(
            ex_style,
            main_class,
            w!("CUMCP_FIXTURE"),
            style,
            80,
            80,
            width,
            height,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("fixture window");
        if chromium_class.as_deref() == Some("child") {
            let _ = CreateWindowExW(WINDOW_EX_STYLE(0), w!("Chrome_WidgetWin_1"), w!(""), WS_CHILD | WS_VISIBLE, 0, 0, 20, 20, Some(_hwnd), None, Some(HINSTANCE(instance.0)), None);
        }
        if frame {
            // Windows 11 rounds the corners, which would hide the corner targets in screenshots.
            let square = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(
                _hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &square as *const _ as *const core::ffi::c_void,
                size_of_val(&square) as u32,
            );
        }
        if maximized {
            let _ = ShowWindow(_hwnd, SW_MAXIMIZE);
        }
        if untitled_dialog {
            register_class(instance, w!("CUMCP_STRAY"), Some(stray_proc));
            let stray = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("CUMCP_STRAY"),
                w!(""),
                WS_POPUP | WS_BORDER,
                80 + width + 40,
                80,
                120,
                40,
                None,
                None,
                Some(HINSTANCE(instance.0)),
                None,
            )
            .expect("stray window");
            let _ = ShowWindow(stray, SW_SHOWNOACTIVATE);
        }
        if owned_popup {
            register_class(instance, w!("CUMCP_STRAY"), Some(stray_proc));
            let popup = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("CUMCP_STRAY"),
                w!(""),
                WS_POPUP | WS_BORDER | WS_VISIBLE,
                80 + width + 40,
                80,
                120,
                40,
                Some(_hwnd),
                None,
                Some(HINSTANCE(instance.0)),
                None,
            )
            .expect("owned popup");
            let _ = popup;
            let _ = ShowWindow(_hwnd, SW_SHOWNA);
        }
        if let Some(delay) = launch_dialog {
            open_launch_dialog(instance, _hwnd, delay);
        }
        if std::env::args().any(|arg| arg == "--noactivate-popup") {
            open_noactivate_popup(instance, _hwnd, width);
        }
        if std::env::args().any(|arg| arg == "--flyout") {
            create_flyout(instance);
        }
        if std::env::args().any(|arg| arg == "--tooltip") {
            open_tooltip(instance);
        }
        if std::env::args().any(|arg| arg == "--combo") {
            add_combo(_hwnd, instance);
        }
        if std::env::args().any(|arg| arg == "--other-thread-tool-window") {
            open_other_thread_tool_window();
        }
        if std::env::args().any(|arg| arg == "--tool-window-behind") {
            open_tool_window_behind(instance, _hwnd);
        }
        if std::env::args().any(|arg| arg == "--invisible-layers") {
            open_invisible_layers(instance);
        }
        if !no_activate {
            let _ = SetForegroundWindow(_hwnd);
        }
        let moves_focus = std::env::args().any(|arg| arg == "--paste-moves-focus");
        // `--tab-to-password` (ticket 92, with `--ui-controls`): a Tab character typed in the main
        // edit moves the focus to the password box, as a login form's Tab order does.
        let tab_to_password = std::env::args().any(|arg| arg == "--tab-to-password");
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 != 0 {
            if tab_to_password && message.message == WM_CHAR && message.wParam.0 == 0x09 && GetDlgCtrlID(message.hwnd) as isize == EDIT_ID {
                if let Ok(password) = GetDlgItem(Some(_hwnd), PASSWORD_ID as i32) {
                    let _ = SetFocus(Some(password));
                }
                continue;
            }
            if moves_focus
                && message.message == WM_KEYDOWN
                && message.wParam.0 == b'V' as usize
                && GetDlgCtrlID(message.hwnd) as isize == EDIT_ID
                && (GetKeyState(VK_CONTROL.0 as i32) as u16) & 0x8000 != 0
            {
                move_paste_focus(message.hwnd);
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if close_on_char.is_some() {
            std::thread::sleep(CLOSED_LINGER);
        }
    }
}

/// `--noactivate-popup` (ticket 154): a visible, enabled, titled popup owned by the fixture
/// (class and title `CUMCP_NOACTIVATE_POPUP`, 120x40 right of the fixture) that cannot be
/// activated (`WS_EX_NOACTIVATE`, shown without activation), like the `Xaml_WindowedPopupClass`
/// host a XAML menu leaves behind. It is no dialog: it never takes the foreground or the input.
fn open_noactivate_popup(instance: windows::Win32::Foundation::HMODULE, fixture: HWND, width: i32) {
    unsafe {
        register_class(instance, w!("CUMCP_NOACTIVATE_POPUP"), Some(stray_proc));
        let popup = CreateWindowExW(
            WS_EX_NOACTIVATE,
            w!("CUMCP_NOACTIVATE_POPUP"),
            w!("CUMCP_NOACTIVATE_POPUP"),
            WS_POPUP | WS_CAPTION | WS_VISIBLE,
            80 + width + 40,
            80,
            120,
            40,
            Some(fixture),
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("no-activate popup");
        let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);
    }
}

/// `--tool-window-behind`: an unowned `WS_POPUP` tool window of the fixture's own thread (class
/// `CUMCP_BEHIND`, like an app's palette or floating toolbar) at screen (20, 200), 140x100, so it
/// sticks out 60 pixels past the fixture's left edge, placed right below the fixture in the
/// z-order: the fixture covers the part they share.
fn open_tool_window_behind(instance: windows::Win32::Foundation::HMODULE, fixture: HWND) {
    unsafe {
        register_class(instance, w!("CUMCP_BEHIND"), Some(stray_proc));
        let Ok(tool) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("CUMCP_BEHIND"),
            w!(""),
            WS_POPUP | WS_BORDER,
            20,
            200,
            140,
            100,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        ) else {
            return;
        };
        let _ = SetWindowPos(tool, Some(fixture), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// `--flyout` (ticket 51, F16): an unowned, not always-on-top `WS_POPUP` tool window of the
/// fixture's own thread that does not take the activation (class `CUMCP_FLYOUT`), like the
/// flyouts and menus apps draw themselves, created hidden at screen (20, 200), 140x100, so it
/// sticks out 60 pixels past the fixture's left edge. A right-click on the fixture's background
/// shows it above the fixture and records `flyout:open`; a left press on it records
/// `flyout:down`.
fn create_flyout(instance: windows::Win32::Foundation::HMODULE) {
    unsafe {
        register_class(instance, w!("CUMCP_FLYOUT"), Some(flyout_proc));
        if let Ok(flyout) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("CUMCP_FLYOUT"),
            w!(""),
            WS_POPUP | WS_BORDER,
            20,
            200,
            140,
            100,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        ) {
            FLYOUT.with(|cell| cell.set(flyout.0 as isize));
        }
    }
}

/// Shows the `--flyout` flyout above the fixture; false without `--flyout`.
fn show_flyout() -> bool {
    let flyout = FLYOUT.with(|cell| cell.get());
    if flyout == 0 {
        return false;
    }
    let flyout = HWND(flyout as *mut core::ffi::c_void);
    unsafe {
        let _ = SetWindowPos(flyout, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    record("flyout:open");
    true
}

unsafe extern "system" fn flyout_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // A click does not activate it, like a menu: the fixture keeps the foreground.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            record("flyout:down");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// `--tooltip` (ticket 51, F17): an unowned, always-on-top, non-activating `WS_POPUP` of the
/// fixture's own thread with the tooltip class name (`tooltips_class32`, registered here, as
/// comctl32 is not loaded) at screen (180, 50), 160x40: it overlaps the fixture's top 10 pixels
/// and sticks out 30 above it. A pointer move on it records `tooltip:move`.
fn open_tooltip(instance: windows::Win32::Foundation::HMODULE) {
    unsafe {
        register_class(instance, w!("tooltips_class32"), Some(tooltip_proc));
        if let Ok(tooltip) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("tooltips_class32"),
            w!(""),
            WS_POPUP | WS_BORDER,
            180,
            50,
            160,
            40,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        ) {
            let _ = ShowWindow(tooltip, SW_SHOWNOACTIVATE);
        }
    }
}

unsafe extern "system" fn tooltip_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            record("tooltip:move");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// `--combo` (ticket 51, F17): a drop-down list combo box at client (340, 300), 130 wide, with
/// ten items (`Item 1`..`Item 10`) and a list tall enough to stick out below the fixture's
/// bottom edge. Records `combo:open` when its list (`ComboLBox`) opens and `combo:sel:<index>`
/// when an item is chosen.
fn add_combo(fixture: HWND, instance: windows::Win32::Foundation::HMODULE) {
    unsafe {
        let style = WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST);
        let combo = child(fixture, instance, w!("ComboBox"), w!(""), style, 340, 300, 130, 240, COMBO_ID);
        for index in 1..=10 {
            let label: Vec<u16> = format!("Item {index}").encode_utf16().chain(std::iter::once(0)).collect();
            SendMessageW(combo, CB_ADDSTRING, None, Some(LPARAM(label.as_ptr() as isize)));
        }
    }
}

/// `--other-thread-tool-window` (ticket 51, F17): an unowned, always-on-top, non-activating
/// `WS_POPUP` tool window of another thread of this process (class `CUMCP_OTHER_THREAD`) at
/// screen (80, 120), 70x160, over the fixture's left edge and the left of its button. Same
/// process, but not the fixture's UI thread, so not one of its popups.
fn open_other_thread_tool_window() {
    std::thread::spawn(|| unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        register_class(instance, w!("CUMCP_OTHER_THREAD"), Some(stray_proc));
        let Ok(window) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("CUMCP_OTHER_THREAD"),
            w!(""),
            WS_POPUP | WS_BORDER,
            80,
            120,
            70,
            160,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        ) else {
            return;
        };
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
}

/// `--invisible-layers`: two unowned, always-on-top `WS_POPUP` tool windows of the fixture's own
/// thread that show nothing, like an app's drag or helper layer: `CUMCP_CLICKTHROUGH`, layered
/// and click-through (`WS_EX_TRANSPARENT`), over the whole fixture and well past it, and
/// `CUMCP_ALPHA0`, layered with alpha 0, overlapping the fixture's bottom-left corner below its
/// Dialog button.
fn open_invisible_layers(instance: windows::Win32::Foundation::HMODULE) {
    unsafe {
        for (class, ex_style, alpha, [x, y, width, height]) in [
            (w!("CUMCP_CLICKTHROUGH"), WS_EX_TRANSPARENT, 1u8, [20, 20, 700, 560]),
            (w!("CUMCP_ALPHA0"), WINDOW_EX_STYLE::default(), 0u8, [20, 420, 200, 120]),
        ] {
            register_class(instance, class, Some(stray_proc));
            let Ok(layer) = CreateWindowExW(
                ex_style | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class,
                w!(""),
                WS_POPUP,
                x,
                y,
                width,
                height,
                None,
                None,
                Some(HINSTANCE(instance.0)),
                None,
            ) else {
                continue;
            };
            let _ = SetLayeredWindowAttributes(layer, COLORREF(0), alpha, LWA_ALPHA);
            let _ = ShowWindow(layer, SW_SHOWNOACTIVATE);
        }
    }
}

/// `--cover x,y,width,height` (screen physical pixels): instead of the fixture, show an
/// always-on-top window there, like another app's picture-in-picture or call bar. It never
/// takes the foreground. Its title reports the pointer input it received:
/// `CUMCP_COVER | ev=<events>` (`cover:down`, `cover:wheel`). With `--not-topmost` it is an
/// ordinary window instead, like another app's window. Windows does not let it rise above the
/// window holding the foreground, so a test that wants the foreground window buried under it
/// moves that window below it (`SWP_NOACTIVATE`). With `--tool-window` it is also a tool window
/// (`WS_EX_TOOLWINDOW`), shaped like an app's own popup (menu, tooltip) but of another process.
fn cover_rect() -> Option<[i32; 4]> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--cover" {
            let values: Vec<i32> = args.next()?.split(',').filter_map(|part| part.trim().parse().ok()).collect();
            return <[i32; 4]>::try_from(values).ok();
        }
    }
    None
}

/// `--core-window-child <hwnd>` (decimal): instead of the fixture, put a window of class
/// `Windows.UI.Core.CoreWindow` (title `CUMCP_CORE`, 100x100) into window `<hwnd>` of another
/// process, the way a UWP app's CoreWindow sits inside its ApplicationFrameHost.exe frame: created
/// as a top-level popup, then made a child of that window with SetParent (ticket 34). It stays until
/// the process is killed or its parent is destroyed.
///
/// Ticket 44: once it has the keyboard focus, its title reports what it read:
/// `CUMCP_CORE | text=<characters> | keys=<events>`, Enter's character shown as `⏎` and each key
/// press and release (other than typed characters' `VK_PACKET`) as `d<vk>` / `u<vk>` in decimal.
/// A `9` it reads shows a top-level window of its own (class and title `CUMCP_STEAL`) and gives it
/// the foreground, like an app opening a window over a key (a digit: with the zh-TW input method
/// open, typed letters and some symbols are composed rather than read). With
/// `--core-window-hang <ms>` it stops reading for that long right after the first character it
/// reads, like an app that hangs; `--core-window-hang-keydown <ms>` and `--core-window-hang-keyup
/// <ms>` do the same right after the first key press or release (other than `VK_PACKET`) it reads,
/// recorded first. With `--core-window-keep-layout` it ignores every keyboard layout
/// request (`WM_INPUTLANGCHANGEREQUEST`), like an app that does not take the English layout, so
/// its thread stays on the layout it has.
fn core_window_hang_ms() -> u64 {
    millis_arg("--core-window-hang")
}

thread_local! {
    static CORE_TEXT: RefCell<String> = const { RefCell::new(String::new()) };
    static CORE_KEYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CORE_HANG_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static CORE_HANG_KEYDOWN_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static CORE_HANG_KEYUP_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static CORE_KEEP_LAYOUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The `CUMCP_STEAL` window, once a `9` created it, and the characters it read.
    static CORE_STEAL: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    static STEAL_TEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

unsafe extern "system" fn core_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    const VK_PACKET: usize = 0xE7;
    match msg {
        WM_CHAR => {
            let character = char::from_u32(wparam.0 as u32).unwrap_or('\u{FFFD}');
            CORE_TEXT.with(|text| text.borrow_mut().push(if character == '\r' { '⏎' } else { character }));
            publish_core(hwnd);
            if character == '9' {
                steal_foreground();
            }
            let hang = CORE_HANG_MS.with(|hang| hang.replace(0));
            if hang > 0 {
                std::thread::sleep(std::time::Duration::from_millis(hang));
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP if wparam.0 != VK_PACKET => {
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let event = if down { "d" } else { "u" };
            CORE_KEYS.with(|keys| keys.borrow_mut().push(format!("{event}{}", wparam.0)));
            publish_core(hwnd);
            let hang = if down { CORE_HANG_KEYDOWN_MS.with(|hang| hang.replace(0)) } else { CORE_HANG_KEYUP_MS.with(|hang| hang.replace(0)) };
            if hang > 0 {
                std::thread::sleep(std::time::Duration::from_millis(hang));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_INPUTLANGCHANGEREQUEST if CORE_KEEP_LAYOUT.with(|keep| keep.get()) => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The `CUMCP_STEAL` window's title reports the characters it read: `CUMCP_STEAL | text=<characters>`.
unsafe extern "system" fn steal_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg != WM_CHAR {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let text = STEAL_TEXT.with(|text| {
        let mut text = text.borrow_mut();
        text.push(char::from_u32(wparam.0 as u32).unwrap_or('\u{FFFD}'));
        text.clone()
    });
    set_text(hwnd, &format!("CUMCP_STEAL | text={text}"));
    LRESULT(0)
}

fn publish_core(hwnd: HWND) {
    let text = CORE_TEXT.with(|text| text.borrow().clone());
    let keys = CORE_KEYS.with(|keys| keys.borrow().join(","));
    set_text(hwnd, &format!("CUMCP_CORE | text={text} | keys={keys}"));
}

fn steal_foreground() {
    unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        let window = match CORE_STEAL.with(|steal| steal.get()) {
            0 => {
                register_class(instance, w!("CUMCP_STEAL"), Some(steal_proc));
                let Ok(window) = CreateWindowExW(
                    WS_EX_APPWINDOW,
                    w!("CUMCP_STEAL"),
                    w!("CUMCP_STEAL"),
                    WS_POPUP | WS_BORDER,
                    640,
                    400,
                    200,
                    100,
                    None,
                    None,
                    Some(HINSTANCE(instance.0)),
                    None,
                ) else {
                    return;
                };
                CORE_STEAL.with(|steal| steal.set(window.0 as isize));
                window
            }
            window => HWND(window as *mut core::ffi::c_void),
        };
        let _ = ShowWindow(window, SW_SHOWNA);
        let _ = SetForegroundWindow(window);
    }
}
fn core_window_parent() -> Option<HWND> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--core-window-child" {
            let value = args.next()?.parse::<isize>().ok().filter(|value| *value != 0)?;
            return Some(HWND(value as *mut core::ffi::c_void));
        }
    }
    None
}

fn run_core_window_child(instance: windows::Win32::Foundation::HMODULE, parent: HWND) {
    unsafe {
        register_class(instance, w!("Windows.UI.Core.CoreWindow"), Some(core_proc));
        CORE_HANG_MS.with(|hang| hang.set(core_window_hang_ms()));
        CORE_HANG_KEYDOWN_MS.with(|hang| hang.set(millis_arg("--core-window-hang-keydown")));
        CORE_HANG_KEYUP_MS.with(|hang| hang.set(millis_arg("--core-window-hang-keyup")));
        CORE_KEEP_LAYOUT.with(|keep| keep.set(std::env::args().any(|arg| arg == "--core-window-keep-layout")));
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("Windows.UI.Core.CoreWindow"),
            w!("CUMCP_CORE"),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("core window");
        SetWindowLongPtrW(window, GWL_STYLE, (WS_CHILD | WS_VISIBLE).0 as isize);
        SetParent(window, Some(parent)).expect("SetParent into the frame");
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// The milliseconds given after `name` (`--slow-input 10`), or 0.
fn millis_arg(name: &str) -> u64 {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next().and_then(|value| value.parse().ok()).unwrap_or(0);
        }
    }
    0
}

const IMC_SETOPENSTATUS: usize = 6;

/// `--slow-ime`: runs before the thread's windows handle a sent message; sleeps before a request
/// that opens or closes the input method.
unsafe extern "system" fn slow_ime_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let message = &*(lparam.0 as *const CWPSTRUCT);
        if message.message == WM_IME_CONTROL && message.wParam.0 == IMC_SETOPENSTATUS {
            std::thread::sleep(std::time::Duration::from_millis(SLOW_IME_MS.with(|slow| slow.get())));
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn run_cover(instance: windows::Win32::Foundation::HMODULE, [x, y, width, height]: [i32; 4], topmost: bool, tool: bool) {
    unsafe {
        register_class(instance, w!("CUMCP_COVER"), Some(cover_proc));
        let hwnd = CreateWindowExW(
            (if topmost { WS_EX_TOPMOST | WS_EX_NOACTIVATE } else { WS_EX_NOACTIVATE }) | if tool { WS_EX_TOOLWINDOW } else { WINDOW_EX_STYLE::default() },
            w!("CUMCP_COVER"),
            w!("CUMCP_COVER | ev="),
            WS_POPUP | WS_BORDER,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("cover window");
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// How `--present` puts its color on screen.
#[derive(Clone, Copy, PartialEq)]
enum Present {
    /// GDI, painted on WM_PAINT, like a classic Win32 app.
    Gdi,
    /// A D3D11 flip-model swap chain (DXGI) for the window.
    SwapChain,
    /// A D3D11 swap chain shown through DirectComposition in a window without a redirection
    /// bitmap (`WS_EX_NOREDIRECTIONBITMAP`), as Chromium and Electron present.
    DComp,
}

/// The colors `--present` alternates between, as RGB: red before the first key press, blue
/// after it, red after the second, and so on.
const PRESENT_COLORS: [[u8; 3]; 2] = [[255, 0, 0], [0, 0, 255]];

/// `--present gdi|swapchain`: instead of the fixture, a plain window (class `CUMCP_PRESENT`)
/// whose whole client area is one color, which changes on every key press (WM_KEYDOWN, not
/// repeats) to the next of `PRESENT_COLORS`. Its title is `CUMCP_PRESENT | n=<presses>`, set once
/// the new color has been painted (`gdi`) or presented (`swapchain`, `dcomp`).
fn present_mode() -> Option<Present> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--present" {
            return match args.next()?.as_str() {
                "gdi" => Some(Present::Gdi),
                "swapchain" => Some(Present::SwapChain),
                "dcomp" => Some(Present::DComp),
                _ => None,
            };
        }
    }
    None
}

struct Presenter {
    mode: Present,
    presses: u32,
    swap_chain: Option<(IDXGISwapChain1, ID3D11DeviceContext, ID3D11RenderTargetView)>,
    /// `dcomp`: kept alive so the swap chain stays on screen.
    _composition: Option<(IDCompositionDevice, IDCompositionTarget, IDCompositionVisual)>,
}

thread_local! {
    static PRESENTER: RefCell<Option<Presenter>> = const { RefCell::new(None) };
}

fn run_present(instance: windows::Win32::Foundation::HMODULE, mode: Present) {
    unsafe {
        register_class(instance, w!("CUMCP_PRESENT"), Some(present_proc));
        PRESENTER.with(|presenter| *presenter.borrow_mut() = Some(Presenter { mode, presses: 0, swap_chain: None, _composition: None }));
        let ex_style = if mode == Present::DComp { WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP } else { WS_EX_APPWINDOW };
        let hwnd = CreateWindowExW(
            ex_style,
            w!("CUMCP_PRESENT"),
            w!("CUMCP_PRESENT | n=0"),
            WS_POPUP,
            80,
            80,
            480,
            360,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )
        .expect("present window");
        if mode != Present::Gdi {
            let (swap_chain, composition) = create_swap_chain(hwnd, mode == Present::DComp);
            PRESENTER.with(|presenter| {
                let mut presenter = presenter.borrow_mut();
                let presenter = presenter.as_mut().unwrap();
                presenter.swap_chain = Some(swap_chain);
                presenter._composition = composition;
            });
        }
        let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
        show_presses(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[allow(clippy::type_complexity)]
unsafe fn create_swap_chain(
    hwnd: HWND,
    composition: bool,
) -> ((IDXGISwapChain1, ID3D11DeviceContext, ID3D11RenderTargetView), Option<(IDCompositionDevice, IDCompositionTarget, IDCompositionVisual)>) {
    let mut device: Option<ID3D11Device> = None;
    let mut context = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        windows::Win32::Foundation::HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        Some(&mut context),
    )
    .expect("D3D11 device");
    let device = device.expect("D3D11 device");
    let factory: IDXGIFactory2 = CreateDXGIFactory1().expect("DXGI factory");
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        // A composition swap chain needs its size; a window's takes the window's.
        Width: if composition { (client.right - client.left) as u32 } else { 0 },
        Height: if composition { (client.bottom - client.top) as u32 } else { 0 },
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let (swap_chain, composed) = if composition {
        let swap_chain = factory.CreateSwapChainForComposition(&device, &desc, None).expect("composition swap chain");
        let dxgi: IDXGIDevice = device.cast().expect("DXGI device");
        let composer: IDCompositionDevice = DCompositionCreateDevice(&dxgi).expect("DirectComposition device");
        let target = composer.CreateTargetForHwnd(hwnd, true).expect("composition target");
        let visual = composer.CreateVisual().expect("visual");
        visual.SetContent(&swap_chain).expect("visual content");
        target.SetRoot(&visual).expect("composition root");
        composer.Commit().expect("composition commit");
        (swap_chain, Some((composer, target, visual)))
    } else {
        (factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None).expect("swap chain"), None)
    };
    let back: ID3D11Texture2D = swap_chain.GetBuffer(0).expect("back buffer");
    let mut target = None;
    device.CreateRenderTargetView(&back, None, Some(&mut target)).expect("render target");
    ((swap_chain, context.expect("D3D11 context"), target.expect("render target")), composed)
}

/// Puts the color for the current press count on screen, then names the count in the title.
unsafe fn show_presses(hwnd: HWND) {
    let (mode, presses) = PRESENTER.with(|presenter| presenter.borrow().as_ref().map(|p| (p.mode, p.presses)).unwrap());
    let [red, green, blue] = PRESENT_COLORS[(presses % 2) as usize];
    match mode {
        Present::Gdi => {
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = UpdateWindow(hwnd);
        }
        Present::SwapChain | Present::DComp => PRESENTER.with(|presenter| {
            if let Some((swap_chain, context, target)) = presenter.borrow().as_ref().and_then(|p| p.swap_chain.as_ref()) {
                context.ClearRenderTargetView(target, &[red as f32 / 255.0, green as f32 / 255.0, blue as f32 / 255.0, 1.0]);
                let _ = swap_chain.Present(1, DXGI_PRESENT(0));
            }
        }),
    }
    set_text(hwnd, &format!("CUMCP_PRESENT | n={presses}"));
}

unsafe extern "system" fn present_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let (mode, presses) = PRESENTER.with(|presenter| presenter.borrow().as_ref().map(|p| (p.mode, p.presses)).unwrap_or((Present::Gdi, 0)));
            if mode == Present::Gdi {
                let [red, green, blue] = PRESENT_COLORS[(presses % 2) as usize];
                let brush = CreateSolidBrush(COLORREF(u32::from(red) | u32::from(green) << 8 | u32::from(blue) << 16));
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                FillRect(dc, &client, brush);
                let _ = DeleteObject(brush.into());
            }
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        // Bit 30 of lparam: the key was already down (auto-repeat).
        WM_KEYDOWN if lparam.0 & (1 << 30) == 0 => {
            PRESENTER.with(|presenter| presenter.borrow_mut().as_mut().unwrap().presses += 1);
            show_presses(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn stray_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// A `--ui-groups` container (ticket 63): a click on one of its buttons records `many:B<n>`.
unsafe extern "system" fn many_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COMMAND && (wparam.0 >> 16) & 0xFFFF == BN_CLICKED {
        let id = (wparam.0 & 0xFFFF) as isize;
        if id >= 2000 {
            record(&format!("many:B{}", id - 1999));
            return LRESULT(0);
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

thread_local! {
    static COVER_EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn cover_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let event = match msg {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => Some("cover:down"),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Some("cover:wheel"),
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => None,
    };
    let Some(event) = event else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    let events = COVER_EVENTS.with(|events| {
        let mut events = events.borrow_mut();
        events.push(event.to_string());
        events.join(",")
    });
    set_text(hwnd, &format!("CUMCP_COVER | ev={events}"));
    LRESULT(0)
}

fn register_class(instance: windows::Win32::Foundation::HMODULE, name: PCWSTR, proc: WNDPROC) {
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc,
            hInstance: HINSTANCE(instance.0),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as *mut core::ffi::c_void),
            lpszClassName: name,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            panic!("RegisterClassExW failed for a fixture class");
        }
    }
}

unsafe extern "system" fn fixture_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // WM_CLIPBOARDUPDATE, `--hang-on-clipboard-update <ms>` (ticket 77): the first clipboard change after the
        // fixture started (the text `paste_text` puts there) is marked `cliphang`, then the fixture
        // reads no input for that long, so a stop can come before Ctrl+V is sent.
        0x031D => {
            static HUNG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !HUNG.swap(true, std::sync::atomic::Ordering::SeqCst) {
                record("cliphang");
                std::thread::sleep(std::time::Duration::from_millis(millis_arg("--hang-on-clipboard-update")));
            }
            LRESULT(0)
        }
        WM_CREATE => {
            let instance = GetModuleHandleW(None).expect("module");
            // ES_AUTOHSCROLL: without it a full edit silently drops wide characters and keeps
            // narrow ones, so long typed text would not be a prefix of what was sent.
            let edit_style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
            let edit = child(hwnd, instance, w!("Edit"), w!(""), edit_style, 10, 10, 460, 28, EDIT_ID);
            let button = child(hwnd, instance, w!("Button"), w!("Click me"), WS_CHILD | WS_VISIBLE, 10, 48, 120, 28, BUTTON_ID);
            let status = child(hwnd, instance, w!("Static"), w!(""), WS_CHILD | WS_VISIBLE, 140, 48, 330, 28, STATUS_ID);
            let _dialog_button = child(hwnd, instance, w!("Button"), w!("Dialog"), WS_CHILD | WS_VISIBLE, 10, 300, 120, 28, DIALOG_BUTTON_ID);
            let _scroll = child(
                hwnd,
                instance,
                w!("CUMCP_SCROLL"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_VSCROLL,
                10,
                88,
                460,
                200,
                1004,
            );
            if UI_CONTROLS.with(|controls| controls.get()) {
                create_ui_controls(hwnd, instance);
            }
            if UI_CONTENT.with(|content| content.get()) {
                create_ui_content(hwnd, instance);
            }
            if UI_SECTION.with(|section| section.get()) {
                create_ui_section(hwnd, instance);
            }
            if UI_READ_ONLY.with(|read_only| read_only.get()) {
                create_read_only(hwnd, instance);
            }
            if UI_SCROLL_TREE.with(|tree| tree.get()) {
                create_scroll_tree(hwnd, instance);
            }
            if UI_TEXT_BOX.with(|text| text.get()) {
                create_ui_text(hwnd, instance);
            }
            if millis_arg("--hang-on-clipboard-update") > 0 {
                let _ = AddClipboardFormatListener(hwnd);
            }
            if std::env::args().any(|arg| arg == "--paste-moves-focus") {
                let _moved = child(hwnd, instance, w!("Edit"), w!(""), edit_style, 140, 300, 330, 28, MOVED_EDIT_ID);
            }
            if std::env::args().any(|arg| arg == "--web-documents") {
                match millis_arg("--web-documents-after") {
                    0 => create_web_documents(hwnd, instance),
                    after => {
                        SetTimer(Some(hwnd), WEB_TIMER_ID, after as u32, None);
                    }
                }
            }
            let button_proc = subclass(button, button_subclass);
            let edit_proc = subclass(edit, edit_subclass);
            STATE.with(|state| {
                *state.borrow_mut() = Some(Fixture {
                    hwnd,
                    edit,
                    status,
                    button_proc,
                    edit_proc,
                    events: Vec::new(),
                    scroll_pos: 0,
                    drag_start: None,
                    drag_modifiers: String::new(),
                    wheel_vertical: WheelStats::default(),
                    wheel_horizontal: WheelStats::default(),
                });
            });
            publish();
            let _ = SetFocus(Some(edit));
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == WEB_TIMER_ID => {
            let _ = KillTimer(Some(hwnd), WEB_TIMER_ID);
            create_web_documents(hwnd, GetModuleHandleW(None).expect("module"));
            LRESULT(0)
        }
        WM_SETFOCUS if EDIT_KEEPS_FOCUS.with(|keeps| keeps.get()) => {
            if let Some(edit) = STATE.with(|state| state.borrow().as_ref().map(|fixture| fixture.edit)) {
                let _ = SetFocus(Some(edit));
            }
            LRESULT(0)
        }
        // In `--frame` mode every point of the window is client area, so a click on the
        // caption or border is recorded instead of moving or sizing the window.
        WM_NCHITTEST if frame_mode() => LRESULT(HTCLIENT),
        WM_LBUTTONDOWN if frame_mode() => {
            let mut point = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = ClientToScreen(hwnd, &mut point);
            note_frame_hit(hwnd, point);
            LRESULT(0)
        }
        // A maximized window still gets caption clicks as non-client messages (screen
        // coordinates). Record them and do not let the caption drag or restore the window.
        WM_NCLBUTTONDOWN if frame_mode() => {
            let point = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            note_frame_hit(hwnd, point);
            LRESULT(0)
        }
        WM_NCLBUTTONDBLCLK if frame_mode() => LRESULT(0),
        WM_LBUTTONDOWN if handoff_mode() => {
            record("bg:down");
            hand_off_on(Trigger::Press);
            slow_pointer("down");
            LRESULT(0)
        }
        WM_LBUTTONUP if handoff_mode() => {
            let (x, y) = ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
            record(&format!("bg:up@{x},{y}"));
            hand_off(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE if handoff_mode() => {
            if (lparam.0 & 0xFFFF) as i16 >= 440 {
                hand_off_on(Trigger::Move);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SIZE if frame_mode() => {
            let _ = InvalidateRect(Some(hwnd), None, true);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_PAINT if frame_mode() => {
            paint_targets(hwnd);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let mut point = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = ScreenToClient(hwnd, &mut point);
            let target = ChildWindowFromPoint(hwnd, point);
            if target != hwnd && !target.0.is_null() {
                return SendMessageW(target, WM_MOUSEWHEEL, Some(wparam), Some(lparam));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SYSKEYDOWN | WM_SYSKEYUP | WM_KEYDOWN | WM_KEYUP => {
            note_system_key(msg, wparam);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_ENTERMENULOOP => {
            record("menu:enter");
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CONTEXTMENU if show_flyout() => LRESULT(0),
        WM_CONTEXTMENU if popup_menu() => {
            open_popup_menu(hwnd);
            LRESULT(0)
        }
        WM_ENTERIDLE if popup_menu() && wparam.0 == MSGF_MENU as usize => {
            note_menu_shown(hwnd, lparam);
            LRESULT(0)
        }
        // Characters reach the window itself only when it, not its edit, has the focus: after a
        // dialog it owns closes, activation gives the focus to the window. Recorded as
        // `char:<character>`, so text that should have gone nowhere shows up in the title.
        WM_CHAR => {
            let character = char::from_u32(wparam.0 as u32).map(|character| character.to_string()).unwrap_or_else(|| format!("{:#x}", wparam.0));
            record(&format!("char:{character}"));
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as isize;
            let notify = (wparam.0 >> 16) & 0xFFFF;
            if notify == BN_CLICKED || notify == CBN_DROPDOWN {
                let hang = COMMAND_HANG_MS.with(|hang| hang.replace(0));
                if hang > 0 {
                    record("hang:command");
                    std::thread::sleep(std::time::Duration::from_millis(hang));
                }
            }
            if id == BUTTON_ID && notify == BN_CLICKED {
                record("click:left");
            } else if id == DIALOG_BUTTON_ID && notify == BN_CLICKED {
                record("dialog:open");
                let closed = open_modal_dialog(hwnd);
                record(&format!("dialog:closed:{closed}"));
            } else if id == COMBO_ID && notify == CBN_DROPDOWN {
                record("combo:open");
            } else if id == COMBO_ID && notify == CBN_SELCHANGE {
                let selected = SendMessageW(HWND(lparam.0 as *mut core::ffi::c_void), CB_GETCURSEL, None, None).0;
                record(&format!("combo:sel:{selected}"));
            } else if id == EDIT_ID && notify == EN_CHANGE {
                publish();
            } else if id == PASSWORD_ID && notify == EN_CHANGE {
                // Its length only: the fixture never shows a password field's text.
                let length = window_text(HWND(lparam.0 as *mut core::ffi::c_void)).chars().count();
                record(&format!("pw:len={length}"));
            } else if id == CHECK_ID && notify == BN_CLICKED && std::env::args().any(|arg| arg == "--check-takes-foreground") {
                // `--check-takes-foreground` (ticket 153, with `--ui-controls`): a click on the
                // check box (a toggle included) puts the check box itself in the foreground, as a
                // control a proxy or a UWP app fronts inside its window does. A WS_CHILD window
                // cannot hold the foreground, so the style bit goes first; its parent (and so its
                // GA_ROOT) stays the fixture.
                let check = HWND(lparam.0 as *mut core::ffi::c_void);
                let _ = SetWindowLongPtrW(check, GWL_STYLE, (WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX)).0 as isize);
                let fronted = SetForegroundWindow(check).as_bool() && GetForegroundWindow() == check;
                record(&format!("check:front:{fronted}"));
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        ARM_SLOW_RAISE => {
            SLOW_RAISE_ARMED.with(|armed| armed.set(SLOW_RAISE_MS.with(|slow| slow.get()) > 0));
            LRESULT(1)
        }
        HANG_UI => {
            std::thread::sleep(std::time::Duration::from_millis(wparam.0 as u64));
            LRESULT(0)
        }
        ARM_SET_TEXT_HANG => {
            SET_TEXT_HANG_MS.with(|hang| hang.set(wparam.0 as u64));
            LRESULT(1)
        }
        ARM_COMMAND_HANG => {
            COMMAND_HANG_MS.with(|hang| hang.set(wparam.0 as u64));
            LRESULT(1)
        }
        ARM_QUERY_HANG => {
            QUERY_HANG_MS.with(|hang| hang.set(wparam.0 as u64));
            LRESULT(1)
        }
        WM_WINDOWPOSCHANGING if SLOW_RAISE_ARMED.with(|armed| armed.replace(false)) => {
            std::thread::sleep(std::time::Duration::from_millis(SLOW_RAISE_MS.with(|slow| slow.get())));
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// `--ui-controls`: the check box and the labelled password box (see `UI_CONTROLS`). They come
/// after the other children, so a UI Automation walk reaches them last.
unsafe fn create_ui_controls(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    let check_style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX);
    let check = child(hwnd, instance, w!("Button"), w!("Remember me"), check_style, 140, 300, 150, 28, CHECK_ID);
    let _label = child(hwnd, instance, w!("Static"), w!("Password:"), WS_CHILD | WS_VISIBLE, 300, 304, 70, 20, PASSWORD_LABEL_ID);
    let password_style = WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32 | ES_PASSWORD);
    let password = child(hwnd, instance, w!("Edit"), w!(""), password_style, 370, 302, 100, 24, PASSWORD_ID);
    let text: Vec<u16> = PASSWORD_TEXT.encode_utf16().chain(Some(0)).collect();
    let _ = SetWindowTextW(password, PCWSTR(text.as_ptr()));
    // For `--hang-name` and `ARM_QUERY_HANG`; it passes everything else on.
    let previous = subclass(check, check_subclass);
    CHECK_PROC.with(|proc| proc.set(previous));
}

/// `--ui-read-only` (ticket 64): a read-only edit holding `READ_ONLY_TEXT` at client (250, 332),
/// right of where `--ui-section` puts its section, and beside it a disabled (not read-only) edit
/// holding `DISABLED_TEXT`.
unsafe fn create_read_only(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32 | ES_READONLY);
    let read_only = child(hwnd, instance, w!("Edit"), w!(""), style, 250, 332, 100, 24, READ_ONLY_ID);
    set_text(read_only, READ_ONLY_TEXT);
    let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
    let disabled = child(hwnd, instance, w!("Edit"), w!(""), style, 360, 332, 100, 24, DISABLED_EDIT_ID);
    set_text(disabled, DISABLED_TEXT);
    let _ = EnableWindow(disabled, false);
}

const ES_MULTILINE: u32 = 0x4;

/// The text of the `--ui-text` Rich Edit: a code point outside the Basic Multilingual Plane (two
/// UTF-16 units), so a cut inside a character can be told from one between characters.
const UI_TEXT: &str = "ab\u{1F600}cd needle xyz";
/// What is selected in it (UTF-16 units, `EM_SETSEL`): the emoji and "cd". The tests hold both
/// texts as literals.
const UI_TEXT_SELECTION: (usize, usize) = (2, 6);
const UI_TEXT_ID: isize = 1700;
const EM_SETSEL: u32 = 0x00B1;
const EM_EXLIMITTEXT: u32 = 0x0435;

/// The text of `--ui-text-long <segments>` (ticket 08): "x", then per segment three emoji and its
/// number, 7 code points and 10 UTF-16 units. So pieces of 20,000 characters end among the emoji
/// from the second on, and the 2 * (offset + max_chars) + 2 units the helper asks for end inside a
/// pair. `helper/tests/read_text.rs` holds the same rule.
fn long_ui_text(segments: usize) -> String {
    let mut text = String::from("x");
    for number in 0..segments {
        text.push_str(&format!("\u{1F600}\u{1F600}\u{1F600}{:04}", number % 10_000));
    }
    text
}

/// `--ui-text` (reference-gaps G4, with `--ui-content`): a multi-line Rich Edit (UI Automation
/// shows a Document with a text pattern) at client (490, 230), 200x80, holding `UI_TEXT` with
/// `UI_TEXT_SELECTION` selected, to the right of the other controls. It does not take the focus.
/// With `--ui-text-long <segments>` (ticket 08) it holds `long_ui_text` instead.
unsafe fn create_ui_text(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    // The Rich Edit class (RICHEDIT50W) lives in this library.
    let _ = LoadLibraryW(w!("Msftedit.dll"));
    let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_MULTILINE);
    let rich = child(hwnd, instance, w!("RICHEDIT50W"), w!(""), style, 490, 230, 200, 80, UI_TEXT_ID);
    match UI_TEXT_LONG.with(|long| long.get()) {
        0 => set_text(rich, UI_TEXT),
        segments => {
            // A Rich Edit keeps 32,767 units unless told otherwise.
            SendMessageW(rich, EM_EXLIMITTEXT, Some(WPARAM(0)), Some(LPARAM(0x7FFF_FFFE)));
            set_text(rich, &long_ui_text(segments));
        }
    }
    SendMessageW(rich, EM_SETSEL, Some(WPARAM(UI_TEXT_SELECTION.0)), Some(LPARAM(UI_TEXT_SELECTION.1 as isize)));
}

const WEB_TIMER_ID: usize = 71;
const WEB_DOCUMENT_ID: isize = 1500;
const WEB_CONTENT_ID: isize = 1510;

/// `--web-documents <list>` (ticket 71): Document elements, which a Chromium app shows for its web
/// content. Each item of the comma-separated list is `large-full`, `large-empty`, `small-full` or
/// `small-empty`: a Rich Edit control (UI Automation shows it as a Document) at client (10, 88),
/// 320x200 (large), or at (340, 88), 130x100 (small); a full one holds a button "Inside" (what its
/// children are), an empty one holds nothing. They come after the other children. With
/// `--web-documents-after <ms>` they are created that long after the window, like web content a
/// Chromium app builds after its first query; without it, with the window.
unsafe fn create_web_documents(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    let list = std::env::args().skip_while(|arg| arg != "--web-documents").nth(1).unwrap_or_default();
    // The Rich Edit class (RICHEDIT50W) lives in this library.
    let _ = LoadLibraryW(w!("Msftedit.dll"));
    for (at, item) in list.split(',').filter(|item| !item.is_empty()).enumerate() {
        let (x, y, width, height) = if item.starts_with("large") { (10, 88, 320, 200) } else { (340, 88, 130, 100) };
        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE);
        let document = child(hwnd, instance, w!("RICHEDIT50W"), w!(""), style, x, y, width, height, WEB_DOCUMENT_ID + at as isize);
        if item.ends_with("full") {
            let _ = child(document, instance, w!("Button"), w!("Inside"), WS_CHILD | WS_VISIBLE, 4, 4, 80, 24, WEB_CONTENT_ID + at as isize);
        }
    }
}

/// Tiny buttons in each `--ui-groups` container: more than a parent's 60 children a UI tree shows.
const GROUP_BUTTONS: usize = 70;
const LIST_ROWS: usize = 20;
/// Rows of each `--ui-lists` list box, all visible: one more than a parent's 60 shown children.
const TINY_ROWS: usize = 61;
const LIST_ROW_HEIGHT: usize = 20;
const LBS_NOTIFY: u32 = 0x1;
const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
const LB_ADDSTRING: u32 = 0x180;
const LB_SETITEMHEIGHT: u32 = 0x1A0;
const CB_SETCURSEL: u32 = 0x14E;
const WS_DISABLED: u32 = 0x0800_0000;
const TVS_HASBUTTONS: u32 = 0x1;
const TVS_HASLINES: u32 = 0x2;
const TVS_LINESATROOT: u32 = 0x4;
const TVM_INSERTITEMW: u32 = 0x1132;
const TVM_EXPAND: u32 = 0x1102;
const TVE_EXPAND: usize = 0x2;
const TVIF_TEXT: u32 = 0x1;
const TVI_ROOT: isize = -0x10000;
const TVI_LAST: isize = -0xFFFE;
const ICC_TREEVIEW_CLASSES: u32 = 0x2;

#[repr(C)]
struct InitCommonControls {
    size: u32,
    classes: u32,
}

/// `TVITEMEXW`.
#[repr(C)]
struct TreeItem {
    mask: u32,
    item: isize,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_max: i32,
    image: i32,
    selected_image: i32,
    children: i32,
    lparam: isize,
    integral: i32,
    state_ex: u32,
    hwnd: isize,
    expanded_image: i32,
    reserved: i32,
}

/// `TVINSERTSTRUCTW`.
#[repr(C)]
struct TreeInsert {
    parent: isize,
    insert_after: isize,
    item: TreeItem,
}

#[link(name = "comctl32")]
extern "system" {
    fn InitCommonControlsEx(classes: *const InitCommonControls) -> i32;
}

/// `--ui-content` (ticket 60): right of the 480 pixels the fixture has without it (client pixels):
/// - a disabled button "Disabled" at (490, 10), 100x28;
/// - a label "Colour:" and a drop-down list combo box at (655, 10), 130 wide, with Red, Green and
///   Blue, Red chosen (with `--ui-long-colours`, also "Shade 1".. "Shade 30", so that its opened
///   list scrolls; that list lies over the right end of the list box below);
/// - a list box at (490, 50), 200x92 with a border, of `LIST_ROWS` rows "Row 1".. 20 pixels high:
///   rows 1 to 4 are whole, row 5 shows its top 10 pixels, and rows 6 on are scrolled out of it;
/// - a tree view at (700, 50), 250x120: "Fruits" expanded (Apple, Banana), "Vegetables"
///   collapsed (Carrot);
/// - `--ui-groups` containers (class `CUMCP_MANY`, 60x42, five per row from (490, 180) every 70
///   by 50 pixels), the first named "Many buttons", the others "Group <n>", each holding
///   `GROUP_BUTTONS` 4x4 buttons "B1".. in rows of ten;
/// - `--ui-lists` list boxes at (490 + 50 i, 230), 40 wide, each showing all its `TINY_ROWS`
///   2-pixel rows "L<list>-<row>".
unsafe fn create_ui_content(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    let classes = InitCommonControls { size: size_of::<InitCommonControls>() as u32, classes: ICC_TREEVIEW_CLASSES };
    InitCommonControlsEx(&classes);
    let _disabled = child(hwnd, instance, w!("Button"), w!("Disabled"), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(WS_DISABLED), 490, 10, 100, 28, 1101);
    let _label = child(hwnd, instance, w!("Static"), w!("Colour:"), WS_CHILD | WS_VISIBLE, 600, 14, 50, 20, 1102);
    let combo = child(hwnd, instance, w!("ComboBox"), w!(""), WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST), 655, 10, 130, 200, 1103);
    for colour in ["Red", "Green", "Blue"] {
        let label: Vec<u16> = colour.encode_utf16().chain(Some(0)).collect();
        SendMessageW(combo, CB_ADDSTRING, None, Some(LPARAM(label.as_ptr() as isize)));
    }
    if std::env::args().any(|arg| arg == "--ui-long-colours") {
        for shade in 1..=30 {
            let label: Vec<u16> = format!("Shade {shade}").encode_utf16().chain(Some(0)).collect();
            SendMessageW(combo, CB_ADDSTRING, None, Some(LPARAM(label.as_ptr() as isize)));
        }
    }
    SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(0)), None);
    let list_style = WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | WS_TABSTOP | WINDOW_STYLE(LBS_NOTIFY | LBS_NOINTEGRALHEIGHT);
    let list = child(hwnd, instance, w!("ListBox"), w!(""), list_style, 490, 50, 200, 92, 1104);
    if LIST_IGNORES_WHEEL.with(|ignores| ignores.get()) {
        LIST_PROC.with(|proc| proc.set(subclass(list, list_subclass)));
    }
    SendMessageW(list, LB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(LIST_ROW_HEIGHT as isize)));
    for row in 1..=LIST_ROWS {
        let label: Vec<u16> = format!("Row {row}").encode_utf16().chain(Some(0)).collect();
        SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(label.as_ptr() as isize)));
    }
    let tree_style = WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE(TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT);
    let tree = child(hwnd, instance, w!("SysTreeView32"), w!(""), tree_style, 700, 50, 250, 120, 1105);
    let insert = |parent: isize, text: &str| -> isize {
        let mut label: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let item = TreeItem {
            mask: TVIF_TEXT,
            item: 0,
            state: 0,
            state_mask: 0,
            text: label.as_mut_ptr(),
            text_max: label.len() as i32,
            image: 0,
            selected_image: 0,
            children: 0,
            lparam: 0,
            integral: 0,
            state_ex: 0,
            hwnd: 0,
            expanded_image: 0,
            reserved: 0,
        };
        let insert = TreeInsert { parent, insert_after: TVI_LAST, item };
        SendMessageW(tree, TVM_INSERTITEMW, None, Some(LPARAM(&insert as *const TreeInsert as isize))).0
    };
    let fruits = insert(TVI_ROOT, "Fruits");
    insert(fruits, "Apple");
    insert(fruits, "Banana");
    let vegetables = insert(TVI_ROOT, "Vegetables");
    insert(vegetables, "Carrot");
    SendMessageW(tree, TVM_EXPAND, Some(WPARAM(TVE_EXPAND)), Some(LPARAM(fruits)));
    for group in 0..UI_GROUPS.with(|groups| groups.get()) as usize {
        let name: Vec<u16> = if group == 0 { "Many buttons".to_string() } else { format!("Group {}", group + 1) }.encode_utf16().chain(Some(0)).collect();
        let (x, y) = (490 + (group % 5) as i32 * 70, 180 + (group / 5) as i32 * 50);
        let container = child(hwnd, instance, w!("CUMCP_MANY"), PCWSTR(name.as_ptr()), WS_CHILD | WS_VISIBLE, x, y, 60, 42, 1200 + group as isize);
        for button in 0..GROUP_BUTTONS {
            let label: Vec<u16> = format!("B{}", button + 1).encode_utf16().chain(Some(0)).collect();
            let (column, row) = ((button % 10) as i32, (button / 10) as i32);
            child(container, instance, w!("Button"), PCWSTR(label.as_ptr()), WS_CHILD | WS_VISIBLE, column * 6, row * 6, 4, 4, 2000 + button as isize);
        }
    }
    for list in 0..UI_LISTS.with(|lists| lists.get()) as usize {
        let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(LBS_NOINTEGRALHEIGHT);
        let height = (TINY_ROWS * 2 + 2) as i32;
        let tiny = child(hwnd, instance, w!("ListBox"), w!(""), style, 490 + list as i32 * 50, 230, 40, height, 1300 + list as isize);
        SendMessageW(tiny, LB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(2)));
        for row in 1..=TINY_ROWS {
            let label: Vec<u16> = format!("L{}-{row}", list + 1).encode_utf16().chain(Some(0)).collect();
            SendMessageW(tiny, LB_ADDSTRING, None, Some(LPARAM(label.as_ptr() as isize)));
        }
    }
}

/// `--list-ignores-wheel`: the list box's procedure; the wheel messages are taken and dropped.
unsafe extern "system" fn list_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL {
        return LRESULT(0);
    }
    CallWindowProcW(LIST_PROC.with(|proc| proc.get()), hwnd, msg, wparam, lparam)
}

const TVM_SETITEMHEIGHT: u32 = 0x111B;

/// `--ui-scroll-tree` (ticket 65, with `--ui-content`): a tree view at client (560, 180), 200x92
/// with a border, of ten items "Item 1".. 20 pixels high: items 1 to 4 are whole, item 5 shows its
/// top 10 pixels, and items 6 on are scrolled out of it. Unlike the list box's rows (whose UI
/// Automation ScrollIntoView leaves a partly visible row as it is), a tree item scrolled into view
/// shows whole. Clear of the `--ui-groups` and `--ui-lists` controls only without those.
unsafe fn create_scroll_tree(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    let style = WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE(TVS_HASLINES | TVS_LINESATROOT);
    let tree = child(hwnd, instance, w!("SysTreeView32"), w!(""), style, 560, 180, 200, 92, 1106);
    SendMessageW(tree, TVM_SETITEMHEIGHT, Some(WPARAM(LIST_ROW_HEIGHT)), None);
    for number in 1..=10 {
        let mut label: Vec<u16> = format!("Item {number}").encode_utf16().chain(Some(0)).collect();
        let item = TreeItem {
            mask: TVIF_TEXT,
            item: 0,
            state: 0,
            state_mask: 0,
            text: label.as_mut_ptr(),
            text_max: label.len() as i32,
            image: 0,
            selected_image: 0,
            children: 0,
            lparam: 0,
            integral: 0,
            state_ex: 0,
            hwnd: 0,
            expanded_image: 0,
            reserved: 0,
        };
        let insert = TreeInsert { parent: TVI_ROOT, insert_after: TVI_LAST, item };
        SendMessageW(tree, TVM_INSERTITEMW, None, Some(LPARAM(&insert as *const TreeInsert as isize)));
    }
}

const SECTION_BUTTON_ID: isize = 1401;

/// `--ui-section` (ticket 62): a section "Section" (class `CUMCP_SECTION`) at client (10, 332),
/// 220x26, below the Dialog button, holding a button "Close section" (110x26) and a label
/// "Section text". Pressing the button records `section:closed` and closes the section with
/// everything in it, like a dialog pane or an info bar that a button dismisses.
unsafe fn create_ui_section(hwnd: HWND, instance: windows::Win32::Foundation::HMODULE) {
    let section = child(hwnd, instance, w!("CUMCP_SECTION"), w!("Section"), WS_CHILD | WS_VISIBLE, 10, 332, 220, 26, 1400);
    let close = child(section, instance, w!("Button"), w!("Close section"), WS_CHILD | WS_VISIBLE | WS_TABSTOP, 0, 0, 110, 26, SECTION_BUTTON_ID);
    child(section, instance, w!("Static"), w!("Section text"), WS_CHILD | WS_VISIBLE, 115, 4, 100, 18, 1402);
    if HANG_NESTED_MS.with(|hang| hang.get()) > 0 {
        NESTED_PROC.with(|proc| proc.set(subclass(close, nested_subclass)));
    }
}

/// `--hang-nested`: sleeps once, on the first text request another thread sends the section's
/// button (a tree read asks for it with the section's other children); passes everything on.
unsafe extern "system" fn nested_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if (msg == WM_GETTEXT || msg == WM_GETTEXTLENGTH) && InSendMessage().as_bool() {
        let hang = HANG_NESTED_MS.with(|hang| hang.replace(0));
        if hang > 0 {
            std::thread::sleep(std::time::Duration::from_millis(hang));
        }
    }
    CallWindowProcW(NESTED_PROC.with(|proc| proc.get()), hwnd, msg, wparam, lparam)
}

/// `--ui-section`: the section's own procedure. Its button's click closes it, after the button
/// has finished handling the click (a posted WM_CLOSE, which a child window answers by destroying
/// itself).
unsafe extern "system" fn section_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COMMAND && (wparam.0 & 0xFFFF) as isize == SECTION_BUTTON_ID && (wparam.0 >> 16) & 0xFFFF == BN_CLICKED {
        record("section:closed");
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// `--hang-name`: sleeps once, on the first text request another thread sends the check box;
/// `ARM_QUERY_HANG`: once, on the first check-state request after it was armed.
unsafe extern "system" fn check_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if (msg == WM_GETTEXT || msg == WM_GETTEXTLENGTH) && InSendMessage().as_bool() {
        let hang = HANG_NAME_MS.with(|hang| hang.replace(0));
        if hang > 0 {
            std::thread::sleep(std::time::Duration::from_millis(hang));
        }
    }
    if (msg == BM_GETCHECK || msg == BM_GETSTATE) && InSendMessage().as_bool() {
        let hang = QUERY_HANG_MS.with(|hang| hang.replace(0));
        if hang > 0 {
            record("hang:query");
            std::thread::sleep(std::time::Duration::from_millis(hang));
        }
    }
    CallWindowProcW(CHECK_PROC.with(|proc| proc.get()), hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn button_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_RBUTTONUP => record("click:right"),
        WM_LBUTTONDBLCLK => record("click:double"),
        _ => note_system_key(msg, wparam),
    }
    call_previous(hwnd, msg, wparam, lparam, |state| state.button_proc)
}

/// Records the keys that can start an app's menu or accelerator mode, whichever fixture window
/// has the focus: any key sent as a system key (`syskey:down:<vk>`, `syskey:up:<vk>`, decimal
/// virtual-key code; ALT is 18) and ALT itself as an ordinary key (`key:alt:down`, `key:alt:up`).
/// Typed text never produces these, so a target that only receives text records none of them.
fn note_system_key(msg: u32, wparam: WPARAM) {
    match msg {
        WM_SYSKEYDOWN => record(&format!("syskey:down:{}", wparam.0)),
        WM_SYSKEYUP => record(&format!("syskey:up:{}", wparam.0)),
        WM_KEYDOWN if wparam.0 == VK_MENU.0 as usize => record("key:alt:down"),
        WM_KEYUP if wparam.0 == VK_MENU.0 as usize => record("key:alt:up"),
        _ => {}
    }
}

unsafe extern "system" fn edit_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_KEYDOWN && wparam.0 == VK_ESCAPE {
        record("key:escape");
    }
    // Ticket 153: the release too (`keyup:escape`), so a test can tell an Esc press the app got
    // without its release (or a release without its press).
    if msg == WM_KEYUP && wparam.0 == VK_ESCAPE {
        record("keyup:escape");
    }
    if msg == WM_SETTEXT && InSendMessage().as_bool() {
        let hang = SET_TEXT_HANG_MS.with(|hang| hang.replace(0));
        if hang > 0 {
            record("hang:settext");
            std::thread::sleep(std::time::Duration::from_millis(hang));
        }
    }
    if msg == WM_CHAR {
        let slow = SLOW_INPUT_MS.with(|slow| slow.get());
        if slow > 0 {
            std::thread::sleep(std::time::Duration::from_millis(slow));
        }
    }
    // A letter press with the modifiers this thread's key state held as it was read:
    // `key:a:ctrl` is Ctrl+A as the app sees it, `key:a:none` a plain A.
    if msg == WM_KEYDOWN && (b'A' as usize..=b'Z' as usize).contains(&wparam.0) {
        record(&format!("key:{}:{}", (wparam.0 as u8).to_ascii_lowercase() as char, held_modifiers()));
    }
    note_system_key(msg, wparam);
    if msg == WM_KEYDOWN && BUSY_PUMP_MS.with(|busy| busy.get()) > 0 {
        let sent = GetMessageTime() as u32;
        let ahead = BUSY_DONE.with(|done| done.borrow().iter().filter(|done| **done > sent).count());
        BUSY_AHEAD.with(|list| list.borrow_mut().push(ahead));
    }
    if msg == WM_KEYDOWN {
        hand_off_on(Trigger::Key(wparam.0));
    }
    let result = if msg == WM_KEYDOWN && (wparam.0 == b'A' as usize || wparam.0 == b'a' as usize) && (GetKeyState(VK_CONTROL.0 as i32) as u16) & 0x8000 != 0 {
        let _ = SendMessageW(hwnd, 0x00B1, Some(WPARAM(0)), Some(LPARAM(-1)));
        LRESULT(0)
    } else {
        call_previous(hwnd, msg, wparam, lparam, |state| state.edit_proc)
    };
    if msg == WM_CHAR {
        let busy = BUSY_PUMP_MS.with(|busy| busy.get());
        if busy > 0 {
            busy_pump(busy);
            BUSY_DONE.with(|done| done.borrow_mut().push(GetTickCount()));
            publish();
        }
        if let Some(character) = char::from_u32(wparam.0 as u32) {
            hand_off_on(Trigger::Char(character));
            if CLOSE_ON_CHAR.with(|close| close.get()) == Some(character) {
                let fixture = STATE.with(|state| state.borrow().as_ref().map(|fixture| fixture.hwnd));
                if let Some(fixture) = fixture {
                    let _ = PostMessageW(Some(fixture), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
    // `--slow-keys`: the press is handled at once, as an app reacts to it; the delay comes after,
    // while the next keys wait in the queue.
    if msg == WM_KEYDOWN {
        let slow = SLOW_KEYS_MS.with(|slow| slow.get());
        if slow > 0 {
            record(&format!("slow:{}", wparam.0));
            std::thread::sleep(std::time::Duration::from_millis(slow));
        }
    }
    result
}

unsafe extern "system" fn scroll_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let notches = delta / 120;
            STATE.with(|state| {
                if let Some(fixture) = state.borrow_mut().as_mut() {
                    fixture.scroll_pos += -notches * 30;
                    fixture.wheel_vertical.add(delta);
                }
            });
            let pos = STATE.with(|state| state.borrow().as_ref().map(|fixture| fixture.scroll_pos).unwrap_or(0));
            record(&format!("scroll:{pos}"));
            // Counted in notches, not messages: Windows merges a wheel event into one the
            // fixture has not read yet (one message of -240 for two notches), ticket 51 (F39).
            let total = STATE.with(|state| state.borrow().as_ref().map(|fixture| fixture.wheel_vertical.sum.unsigned_abs() / 120).unwrap_or(0));
            if let Some(Trigger::Wheel(n)) = HANDOFF_ON.with(|trigger| trigger.get()) {
                if n <= total {
                    hand_off_on(Trigger::Wheel(n));
                }
            }
            slow_pointer("wheel");
            LRESULT(0)
        }
        WM_MOUSEHWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            STATE.with(|state| {
                if let Some(fixture) = state.borrow_mut().as_mut() {
                    fixture.wheel_horizontal.add(delta);
                }
            });
            record("hscroll");
            LRESULT(0)
        }
        WM_KEYDOWN if PASTE_AREA.with(|area| area.get()) && wparam.0 == b'V' as usize && (GetKeyState(VK_CONTROL.0 as i32) as u16) & 0x8000 != 0 => {
            // `--hang-on-paste <ms>` (ticket 77): the area has read the V press but takes that long
            // before it reads the clipboard (marked `pastehang`), so it has not read the releases
            // yet when a stop comes.
            let hang = millis_arg("--hang-on-paste");
            if hang > 0 {
                record("pastehang");
                std::thread::sleep(std::time::Duration::from_millis(hang));
            }
            area_paste(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if PASTE_AREA.with(|area| area.get()) {
                let _ = SetFocus(Some(hwnd));
            }
            let point = parent_point(hwnd, lparam);
            let modifiers = held_modifiers();
            STATE.with(|state| {
                if let Some(fixture) = state.borrow_mut().as_mut() {
                    fixture.drag_start = Some(point);
                    fixture.drag_modifiers = modifiers;
                }
            });
            slow_pointer("down");
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let end = parent_point(hwnd, lparam);
            let (start, down_modifiers) = STATE.with(|state| {
                state.borrow().as_ref().map(|fixture| (fixture.drag_start, fixture.drag_modifiers.clone())).unwrap_or_default()
            });
            STATE.with(|state| {
                if let Some(fixture) = state.borrow_mut().as_mut() {
                    fixture.drag_start = None;
                }
            });
            if let Some((x1, y1)) = start {
                record(&format!("drag:{x1},{y1}-{},{}", end.0, end.1));
                // A separate event, so `drag:` keeps its plain coordinate format. Recorded only
                // when a modifier was held as the button went down or came up.
                let up_modifiers = held_modifiers();
                if down_modifiers != "none" || up_modifiers != "none" {
                    record(&format!("dragmods:{down_modifiers}/{up_modifiers}"));
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = parent_point(hwnd, lparam);
            record_hover(x, y);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// `--paste-area`: Ctrl+V in the scroll area reads the clipboard's text, as `paste:<text>` (or
/// `paste:none` when there is none, `paste:unopened` when it could not be opened).
/// Before that, `fmt:ex<0|1>/hist<value>/cloud<value>` says what the clipboard held of the formats
/// that keep text out of monitors (present or not), the history and the cloud clipboard (their
/// DWORD, `x` when absent).
fn area_paste(hwnd: HWND) {
    record(&format!("fmt:{}", private_formats(hwnd)));
    let text = match read_clipboard_text(hwnd) {
        Ok(text) => text.unwrap_or_else(|| "none".to_string()),
        Err(()) => "unopened".to_string(),
    };
    record(&format!("paste:{text}"));
    if std::env::args().any(|arg| arg == "--paste-overwrite") {
        overwrite_clipboard(hwnd);
    }
    let hold = millis_arg("--hold-clipboard-after-paste");
    if hold > 0 {
        hold_clipboard(hold);
    }
}

/// `--paste-moves-focus` (ticket 77): Ctrl+V in the main edit box is not pasted there; instead
/// the clipboard's text goes into the second edit, which takes the focus (marked `moved`), as an
/// app whose focus moves to another field that already shows the text.
fn move_paste_focus(edit: HWND) {
    unsafe {
        let Ok(parent) = GetParent(edit) else { return };
        let Ok(moved) = GetDlgItem(Some(parent), MOVED_EDIT_ID as i32) else { return };
        let text: Vec<u16> = clipboard_text(parent).unwrap_or_default().encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetWindowTextW(moved, PCWSTR(text.as_ptr()));
        let _ = SetFocus(Some(moved));
    }
    record("moved");
}

/// `--hold-clipboard-after-paste <ms>` (ticket 77): after pasting, another thread of the fixture
/// opens the clipboard and keeps it open that long (marked `cliphold` once it is open), as a
/// program that keeps the clipboard open while the helper would put the user's back.
fn hold_clipboard(millis: u64) {
    let (opened, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WS_POPUP, 0, 0, 1, 1, None, None, None, None).ok();
        let open = window.is_some() && OpenClipboard(window).is_ok();
        let _ = opened.send(open);
        if open {
            std::thread::sleep(std::time::Duration::from_millis(millis));
            let _ = CloseClipboard();
        }
        if let Some(window) = window {
            let _ = DestroyWindow(window);
        }
    });
    record(if wait.recv().unwrap_or(false) { "cliphold" } else { "cliphold:unopened" });
}

/// `--paste-overwrite` (ticket 77): after pasting, the fixture copies "fixture overwrite" as
/// another program would during the paste.
fn overwrite_clipboard(owner: HWND) {
    let text: Vec<u16> = "fixture overwrite".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            record("overwrite:unopened");
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2) {
            let pointer = GlobalLock(memory) as *mut u16;
            if !pointer.is_null() {
                std::ptr::copy_nonoverlapping(text.as_ptr(), pointer, text.len());
                let _ = GlobalUnlock(memory);
                if SetClipboardData(13, Some(HANDLE(memory.0))).is_err() {
                    let _ = GlobalFree(Some(memory));
                }
            }
        }
        let _ = CloseClipboard();
    }
}

fn private_formats(owner: HWND) -> String {
    unsafe {
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        let cloud = RegisterClipboardFormatW(w!("CanUploadToCloudClipboard"));
        if !open_clipboard(owner) {
            return "unopened".to_string();
        }
        let dword = |format: u32| -> String {
            let Ok(handle) = GetClipboardData(format) else { return "x".to_string() };
            let memory = HGLOBAL(handle.0);
            let pointer = GlobalLock(memory) as *const u8;
            if pointer.is_null() || GlobalSize(memory) < 4 {
                return "?".to_string();
            }
            let value = u32::from_le_bytes(std::slice::from_raw_parts(pointer, 4).try_into().unwrap_or([0; 4]));
            let _ = GlobalUnlock(memory);
            value.to_string()
        };
        let present = u8::from(IsClipboardFormatAvailable(exclude).is_ok());
        let found = format!("ex{present}/hist{}/cloud{}", dword(history), dword(cloud));
        let _ = CloseClipboard();
        found
    }
}

/// Opens the clipboard as `owner`, as a careful app does: programs that watch the clipboard (File
/// Explorer among them) open it for a moment after every change, so one failed try is retried, for
/// up to `CLIPBOARD_OPEN_TRIES` x 5 ms (ticket 84).
fn open_clipboard(owner: HWND) -> bool {
    for _ in 0..CLIPBOARD_OPEN_TRIES {
        if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

const CLIPBOARD_OPEN_TRIES: u32 = 40;

/// The clipboard's Unicode text, opened as `owner`; None when there is none or it could not be
/// opened.
fn clipboard_text(owner: HWND) -> Option<String> {
    read_clipboard_text(owner).ok().flatten()
}

/// The clipboard's Unicode text (None when there is none), opened as `owner`; Err when it could not
/// be opened (`open_clipboard`).
fn read_clipboard_text(owner: HWND) -> Result<Option<String>, ()> {
    unsafe {
        if !open_clipboard(owner) {
            return Err(());
        }
        let text = GetClipboardData(13).ok().and_then(|handle| {
            let memory = HGLOBAL(handle.0);
            let pointer = GlobalLock(memory) as *const u16;
            if pointer.is_null() {
                return None;
            }
            let units = GlobalSize(memory) / 2;
            let text: Vec<u16> = std::slice::from_raw_parts(pointer, units).iter().copied().take_while(|unit| *unit != 0).collect();
            let _ = GlobalUnlock(memory);
            Some(String::from_utf16_lossy(&text))
        });
        let _ = CloseClipboard();
        Ok(text)
    }
}

/// The modifiers down in this thread's input state for the message being handled, as
/// `shift+ctrl+alt` (in that order), or `none`.
fn held_modifiers() -> String {
    let held: Vec<&str> = [("shift", VK_SHIFT), ("ctrl", VK_CONTROL), ("alt", VK_MENU)]
        .into_iter()
        .filter(|(_, key)| unsafe { GetKeyState(key.0 as i32) as u16 } & 0x8000 != 0)
        .map(|(name, _)| name)
        .collect();
    if held.is_empty() { "none".to_string() } else { held.join("+") }
}

fn record(event: &str) {
    STATE.with(|state| {
        if let Some(fixture) = state.borrow_mut().as_mut() {
            fixture.events.push(event.to_string());
            if fixture.events.len() > 8 {
                fixture.events.remove(0);
            }
        }
    });
    publish();
}

fn record_hover(x: i32, y: i32) {
    STATE.with(|state| {
        if let Some(fixture) = state.borrow_mut().as_mut() {
            if fixture.events.last().is_some_and(|last| last.starts_with("hover:")) {
                fixture.events.pop();
            }
            fixture.events.push(format!("hover:{x},{y}"));
            if fixture.events.len() > 8 {
                fixture.events.remove(0);
            }
        }
    });
    publish();
}

fn publish() {
    let Some((hwnd, status, edit, events, wheel)) = STATE.with(|state| {
        let borrowed = state.borrow();
        let fixture = borrowed.as_ref()?;
        Some((fixture.hwnd, fixture.status, fixture.edit, fixture.events.join(","), wheel_field(fixture)))
    }) else {
        return;
    };
    let full = window_text(edit);
    let edit_text = truncate(&full, 80);
    let title = format!("CUMCP_FIXTURE{wheel} | edit={edit_text} | len={} | ev={events}{}", full.chars().count(), busy_field());
    set_text(hwnd, &title);
    set_text(status, &title);
}

/// ` | wheel=v:n=<count>,sum=<total>,min=<delta>,max=<delta>;h:...` for the axes that received a
/// wheel event, right after the class name so the parts after it keep their place; empty when
/// no wheel event arrived.
fn wheel_field(fixture: &Fixture) -> String {
    let axis = |name: &str, stats: &WheelStats| {
        (stats.count > 0).then(|| format!("{name}:n={},sum={},min={},max={}", stats.count, stats.sum, stats.min, stats.max))
    };
    let parts: Vec<String> = [axis("v", &fixture.wheel_vertical), axis("h", &fixture.wheel_horizontal)].into_iter().flatten().collect();
    if parts.is_empty() { String::new() } else { format!(" | wheel={}", parts.join(";")) }
}

fn parent_point(child: HWND, lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    let parent = STATE.with(|state| state.borrow().as_ref().map(|fixture| fixture.hwnd).unwrap_or_default());
    unsafe {
        let mut point = POINT { x, y };
        let _ = ClientToScreen(child, &mut point);
        let _ = ScreenToClient(parent, &mut point);
        (point.x, point.y)
    }
}

fn window_text(hwnd: HWND) -> String {
    unsafe {
        // Sized to the text, so `len=` counts long typed text in full.
        let mut buffer = vec![0u16; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let length = GetWindowTextW(hwnd, &mut buffer);
        if length <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

fn set_text(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

unsafe fn child(
    parent: HWND,
    instance: windows::Win32::Foundation::HMODULE,
    class: PCWSTR,
    title: PCWSTR,
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: isize,
) -> HWND {
    CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class,
        title,
        style,
        x,
        y,
        width,
        height,
        Some(parent),
        Some(HMENU(id as *mut core::ffi::c_void)),
        Some(HINSTANCE(instance.0)),
        None,
    )
    .expect("child window")
}

unsafe fn subclass(hwnd: HWND, proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) -> WNDPROC {
    let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as isize);
    if previous == 0 {
        None
    } else {
        Some(std::mem::transmute::<isize, unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>(previous))
    }
}

unsafe fn call_previous(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    previous: impl FnOnce(&Fixture) -> WNDPROC,
) -> LRESULT {
    let proc = STATE.with(|state| state.borrow().as_ref().and_then(|fixture| previous(fixture)));
    CallWindowProcW(proc, hwnd, msg, wparam, lparam)
}

fn popup_menu() -> bool {
    POPUP_MENU.with(|popup| popup.get())
}

/// `--popup-menu`: opens a classic menu (`TrackPopupMenu`, class `#32768`) whose top-left corner
/// is 60 pixels left of the fixture's visible frame and 40 pixels above its bottom edge, so it
/// sticks out past both, like Explorer's context menu past its window. Records `menu:open`, then
/// the chosen command as `menu:cmd:<id>` (101 for the first item) or `menu:none`.
fn open_popup_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            record("menu:failed");
            return;
        };
        for index in 0..MENU_ITEMS {
            let label: Vec<u16> = format!("Menu item {}", index + 1).encode_utf16().chain(std::iter::once(0)).collect();
            let _ = AppendMenuW(menu, MF_STRING, (MENU_FIRST_ID + index) as usize, PCWSTR(label.as_ptr()));
        }
        let frame = visible_frame(hwnd);
        record("menu:open");
        OPEN_MENU.with(|open| open.set(Some((menu.0 as isize, false))));
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, frame.left - 60, frame.bottom - 40, None, hwnd, None).0;
        OPEN_MENU.with(|open| open.set(None));
        let _ = DestroyMenu(menu);
        if chosen == 0 { record("menu:none") } else { record(&format!("menu:cmd:{chosen}")) }
    }
}

/// Once the menu is on screen: records its window and the screen rect of its last item, which
/// lies below the fixture, as `menuwin:<hwnd>` and `menulast:<left>,<top>,<right>,<bottom>`.
fn note_menu_shown(hwnd: HWND, menu_window: LPARAM) {
    let Some((menu, false)) = OPEN_MENU.with(|open| open.get()) else { return };
    let mut rect = RECT::default();
    if unsafe { GetMenuItemRect(Some(hwnd), HMENU(menu as *mut core::ffi::c_void), MENU_ITEMS - 1, &mut rect) }.is_err() {
        return;
    }
    OPEN_MENU.with(|open| open.set(Some((menu, true))));
    record(&format!("menuwin:{}", menu_window.0));
    record(&format!("menulast:{},{},{},{}", rect.left, rect.top, rect.right, rect.bottom));
}

fn untitled_dialog() -> bool {
    UNTITLED_DIALOG.with(|untitled| untitled.get())
}

fn frame_mode() -> bool {
    FRAME_MODE.with(|mode| mode.get())
}

/// The visible frame (DWM extended frame bounds) in screen pixels.
fn visible_frame(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut RECT as *mut _, size_of::<RECT>() as u32);
    }
    rect
}

/// Eight edge targets: the corners and edge midpoints of the visible frame. Each is a
/// `TARGET`-pixel square flush with the frame edge, in frame-relative pixels, with the
/// color it is painted in (0x00BBGGRR).
fn frame_targets(width: i32, height: i32) -> [(&'static str, RECT, u32); 8] {
    let square = |left: i32, top: i32| RECT { left, top, right: left + TARGET, bottom: top + TARGET };
    let mid_x = width / 2 - TARGET / 2;
    let mid_y = height / 2 - TARGET / 2;
    let right = width - TARGET;
    let bottom = height - TARGET;
    [
        ("tl", square(0, 0), 0x0000_00FF),
        ("tm", square(mid_x, 0), 0x00FF_0080),
        ("tr", square(right, 0), 0x00FF_0000),
        ("ml", square(0, mid_y), 0x0000_FFFF),
        ("mr", square(right, mid_y), 0x00FF_00FF),
        ("bl", square(0, bottom), 0x00FF_FF00),
        ("bm", square(mid_x, bottom), 0x0000_80FF),
        ("br", square(right, bottom), 0x0000_FF00),
    ]
}

/// Records which edge target a click landed on, with its frame-relative position.
fn note_frame_hit(hwnd: HWND, point: POINT) {
    let frame = visible_frame(hwnd);
    let x = point.x - frame.left;
    let y = point.y - frame.top;
    let hit = frame_targets(frame.right - frame.left, frame.bottom - frame.top)
        .into_iter()
        .find(|(_, rect, _)| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom);
    match hit {
        Some((name, _, _)) => record(&format!("hit:{name}@{x}x{y}")),
        None => record(&format!("miss@{x}x{y}")),
    }
}

/// Paints the parts of the edge targets that fall inside the client area. Targets on the
/// caption are not painted (the system draws the caption) but still record clicks.
fn paint_targets(hwnd: HWND) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut paint);
        let frame = visible_frame(hwnd);
        let mut origin = POINT::default();
        let _ = ClientToScreen(hwnd, &mut origin);
        let dx = frame.left - origin.x;
        let dy = frame.top - origin.y;
        for (_, rect, color) in frame_targets(frame.right - frame.left, frame.bottom - frame.top) {
            let client = RECT { left: rect.left + dx, top: rect.top + dy, right: rect.right + dx, bottom: rect.bottom + dy };
            let brush = CreateSolidBrush(COLORREF(color));
            FillRect(dc, &client, brush);
            let _ = DeleteObject(brush.into());
        }
        let _ = EndPaint(hwnd, &paint);
    }
}

thread_local! {
    static DIALOG_EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Opens a modal dialog owned by `owner`, like an app's Save As or settings dialog, and returns
/// how it closed: 1 (IDOK: Enter or the OK button) or 2 (IDCANCEL: Esc). The dialog has an edit
/// control and a default OK button. Its title reports what it received:
/// `CUMCP_DIALOG | edit=<text> | ev=<events>`, where a click on its background records `dlg:click`.
/// With `--untitled-dialog` its title stays empty. It has no system menu, so no caption buttons.
fn open_modal_dialog(owner: HWND) -> isize {
    let template = dialog_template();
    unsafe {
        let instance = GetModuleHandleW(None).expect("module");
        DialogBoxIndirectParamW(Some(HINSTANCE(instance.0)), template.as_ptr() as *const DLGTEMPLATE, Some(owner), Some(dialog_proc), LPARAM(0))
    }
}

/// An in-memory DLGTEMPLATE (dialog units) with the edit control and the default OK button.
/// The template must be DWORD aligned, so it is returned as `u32`s.
fn dialog_template() -> Vec<u32> {
    const DS_MODALFRAME: u32 = 0x80;
    const DS_CENTER: u32 = 0x800;
    const ES_AUTOHSCROLL: u32 = 0x80;
    const BS_DEFPUSHBUTTON: u32 = 0x1;
    const BUTTON_ATOM: u16 = 0x0080;
    const EDIT_ATOM: u16 = 0x0081;
    fn push_u32(words: &mut Vec<u16>, value: u32) {
        words.push(value as u16);
        words.push((value >> 16) as u16);
    }
    fn push_text(words: &mut Vec<u16>, text: &str) {
        words.extend(text.encode_utf16());
        words.push(0);
    }
    fn item(words: &mut Vec<u16>, style: u32, rect: [i16; 4], id: u16, atom: u16, text: &str) {
        if words.len() % 2 == 1 {
            words.push(0);
        }
        push_u32(words, style | WS_CHILD.0 | WS_VISIBLE.0);
        push_u32(words, 0);
        words.extend(rect.map(|value| value as u16));
        words.push(id);
        words.extend([0xFFFF, atom]);
        push_text(words, text);
        words.push(0);
    }
    let mut words = Vec::new();
    push_u32(&mut words, DS_MODALFRAME | DS_CENTER | WS_POPUP.0 | WS_CAPTION.0);
    push_u32(&mut words, 0);
    words.push(2);
    words.extend([0i16, 0, 180, 90].map(|value| value as u16));
    words.extend([0, 0]);
    push_text(&mut words, if untitled_dialog() { "" } else { "CUMCP_DIALOG" });
    item(&mut words, WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL, [7, 7, 120, 12], DIALOG_EDIT_ID, EDIT_ATOM, "");
    item(&mut words, WS_TABSTOP.0 | BS_DEFPUSHBUTTON, [7, 68, 50, 14], IDOK as u16, BUTTON_ATOM, "OK");
    if words.len() % 2 == 1 {
        words.push(0);
    }
    words.chunks(2).map(|pair| pair[0] as u32 | (pair[1] as u32) << 16).collect()
}

unsafe extern "system" fn dialog_proc(dialog: HWND, msg: u32, wparam: WPARAM, _lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            DIALOG_EVENTS.with(|events| events.borrow_mut().clear());
            publish_dialog(dialog);
            1
        }
        WM_LBUTTONDOWN => {
            DIALOG_EVENTS.with(|events| events.borrow_mut().push("dlg:click".to_string()));
            publish_dialog(dialog);
            1
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            let notify = (wparam.0 >> 16) & 0xFFFF;
            if id == IDOK || id == IDCANCEL {
                let _ = EndDialog(dialog, id as isize);
            } else if id == DIALOG_EDIT_ID as usize && notify == EN_CHANGE {
                publish_dialog(dialog);
            }
            1
        }
        _ => 0,
    }
}

fn publish_dialog(dialog: HWND) {
    if untitled_dialog() {
        return;
    }
    let edit = unsafe { GetDlgItem(Some(dialog), DIALOG_EDIT_ID as i32) }.map(window_text).unwrap_or_default();
    let events = DIALOG_EVENTS.with(|events| events.borrow().join(","));
    set_text(dialog, &format!("CUMCP_DIALOG | edit={} | ev={events}", truncate(&edit, 40)));
}
