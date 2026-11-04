//! Ticket 97: `read_clipboard` returns the clipboard's `CF_UNICODETEXT` and nothing else: text as
//! it is, `null` without text (empty, a picture, copied files), and `clipboard_busy` while another
//! program keeps the clipboard open. The tests use the developer's real clipboard and keep it
//! around themselves (best effort: what the helper's own saving keeps, `clipboard::Saved`; GDI-only
//! formats such as a bare `CF_BITMAP` are not put back).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};

#[path = "interactive_lock.rs"]
mod interactive_lock;

#[allow(dead_code)]
#[path = "../src/clipboard.rs"]
mod clipboard;

/// Unicode text comes back exactly, up to its NUL: CR LF, a character outside the BMP, Chinese.
#[test]
fn text_on_the_clipboard_is_read_as_it_is() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    let text = "剪貼簿 97 text 😀\r\nsecond line\ttab";
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide(text))]);
    let mut helper = Helper::spawn();
    let read = helper.request("read_clipboard", json!({}));
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["text"], text, "{read}");
}

/// No text is `text: null`, not an error: an empty clipboard, a picture, copied files.
#[test]
fn a_clipboard_without_text_reads_as_null() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    let mut helper = Helper::spawn();
    for (what, formats) in [
        ("empty", vec![]),
        ("a picture", vec![(clipboard::CF_DIB, small_dib())]),
        ("copied files", vec![(clipboard::CF_HDROP, drop_files(&["C:\\Users\\Public\\secret-97.txt"]))]),
    ] {
        put_clipboard(&formats);
        let read = helper.request("read_clipboard", json!({}));
        assert_eq!(read["ok"], true, "{what}: {read}");
        assert_eq!(read["result"]["text"], Value::Null, "{what}: {read}");
        assert!(!read.to_string().contains("secret-97"), "{what}: the answer names a copied file: {read}");
    }
}

/// While another program (here this test, through a window of its own: a clipboard opened without
/// a window does not keep another `OpenClipboard(NULL)` out) keeps the clipboard open, the helper
/// retries as `paste_text` does and then answers `clipboard_busy`; once it is closed the read works
/// again.
#[test]
fn a_clipboard_kept_open_by_another_program_is_clipboard_busy() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let _keep = KeepClipboard::new();
    put_clipboard(&[(clipboard::CF_UNICODETEXT, wide("busy 97"))]);
    let mut helper = Helper::spawn();
    let held = HeldClipboard::open();
    let read = helper.request("read_clipboard", json!({}));
    drop(held);
    assert_eq!(read["ok"], false, "{read}");
    assert_eq!(read["error"]["code"], "clipboard_busy", "{read}");
    let read = helper.request("read_clipboard", json!({}));
    assert_eq!(read["result"]["text"], "busy 97", "{read}");
}

/// The clipboard opened by a hidden window of this test's; closed (and the window destroyed) when dropped.
struct HeldClipboard(HWND);

impl HeldClipboard {
    fn open() -> Self {
        unsafe {
            let window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("read_clipboard busy"), WS_POPUP, 0, 0, 1, 1, None, None, None, None).expect("window");
            OpenClipboard(Some(window)).expect("open the clipboard with the window");
            Self(window)
        }
    }
}

impl Drop for HeldClipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
            let _ = DestroyWindow(self.0);
        }
    }
}

/// The developer's clipboard, saved as the helper saves one and put back when dropped.
struct KeepClipboard(Option<clipboard::Saved>);

impl KeepClipboard {
    fn new() -> Self {
        Self(clipboard::Open::new(50).map(|open| open.save()))
    }
}

impl Drop for KeepClipboard {
    fn drop(&mut self) {
        if let (Some(saved), Some(open)) = (self.0.as_ref(), clipboard::Open::new(50)) {
            open.put_back(saved);
        }
    }
}

/// `text` as `CF_UNICODETEXT` bytes, ending in NUL.
fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// Empties the clipboard and puts `formats` there, as global memory blocks.
fn put_clipboard(formats: &[(u32, Vec<u8>)]) {
    let _open = clipboard::Open::new(50).expect("open the clipboard");
    unsafe {
        EmptyClipboard().expect("empty the clipboard");
        for (format, bytes) in formats {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).expect("GlobalAlloc");
            let pointer = GlobalLock(memory) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            let _ = GlobalUnlock(memory);
            if SetClipboardData(*format, Some(HANDLE(memory.0))).is_err() {
                let _ = GlobalFree(Some(memory));
                panic!("SetClipboardData({format}) failed");
            }
        }
    }
}

/// A 2 x 2, 32-bit `CF_DIB`: a BITMAPINFOHEADER, then the pixels.
fn small_dib() -> Vec<u8> {
    let mut dib = Vec::new();
    for value in [40u32, 2, 2] {
        dib.extend(value.to_le_bytes());
    }
    dib.extend(1u16.to_le_bytes());
    dib.extend(32u16.to_le_bytes());
    for value in [0u32, 16, 0, 0, 0, 0] {
        dib.extend(value.to_le_bytes());
    }
    dib.extend([0x10, 0x20, 0x30, 0, 0x40, 0x50, 0x60, 0, 0x70, 0x80, 0x90, 0, 0xA0, 0xB0, 0xC0, 0]);
    dib
}

/// `CF_HDROP` for `paths`: a DROPFILES header (wide names start after its 20 bytes), then the names,
/// each ending in NUL, and one more NUL.
fn drop_files(paths: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [20u32, 0, 0, 0, 1] {
        bytes.extend(value.to_le_bytes());
    }
    for path in paths {
        bytes.extend(wide(path));
    }
    bytes.extend([0, 0]);
    bytes
}

fn interactive_desktop() -> bool {
    unsafe {
        let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS) else {
            return false;
        };
        let _ = CloseDesktop(desktop);
        true
    }
}

struct Helper {
    child: Child,
    incoming: Receiver<Value>,
    next_id: u64,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("helper");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let ready = incoming.recv_timeout(Duration::from_secs(10)).expect("ready");
        assert_eq!(ready["event"], "ready", "{ready}");
        Self { child, incoming, next_id: 1 }
    }

    /// The answer to the request; events are skipped.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut stdin = self.child.stdin.as_ref().expect("stdin");
        writeln!(stdin, "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        stdin.flush().unwrap();
        loop {
            let value = self.incoming.recv_timeout(Duration::from_secs(15)).expect("response");
            if value.get("event").is_some() || value["id"].as_u64() != Some(id) {
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
