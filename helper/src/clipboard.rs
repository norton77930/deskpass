//! The clipboard for `paste_text` (ticket 77): the user's clipboard is saved, the text to paste is
//! put there marked to stay out of clipboard monitors, the clipboard history and the cloud
//! clipboard, and the saved clipboard is put back afterwards, unless another program changed the
//! clipboard meanwhile (`Pasting`). What `paste_text` saves is put back as it was and never read for
//! anything else, shown or logged. Ticket 97's `read_clipboard` (`read_text`) is the one read the
//! helper answers with: the `CF_UNICODETEXT` text only, never a picture or a file list.
//!
//! Self-contained (std and the windows crate only), so the interactive tests include it to keep the
//! developer's clipboard around themselves.
//!
//! What is saved (`Saved`), format by format as `EnumClipboardFormats` lists them:
//! - Formats whose data is a global memory block (text, Unicode text, locale, HTML, RTF, PNG, DIB,
//!   `CF_HDROP`, and registered formats in general) are copied byte for byte and put back as they
//!   were, in the same order.
//! - GDI object formats (`CF_BITMAP`, `CF_PALETTE`, `CF_METAFILEPICT`, `CF_ENHMETAFILE`, the
//!   `CF_DSP*` ones that are GDI objects, the `CF_GDIOBJFIRST`..`CF_GDIOBJLAST` range), owner
//!   display, and the private range `CF_PRIVATEFIRST`..`CF_PRIVATELAST` (whose handles may be
//!   anything) are not saved: the clipboard is then not restored in full (`partial`). Except where
//!   Windows makes such a format from a memory format listed before it, which it does again once
//!   that format is back: a `CF_BITMAP` or `CF_PALETTE` listed after a `CF_DIB` or `CF_DIBV5`
//!   (Windows lists the formats it can make from others after the ones put there), so a copied
//!   picture (DIB, often with PNG beside it) comes back in full.
//! - Text formats Windows made from another text format (`CF_TEXT`, `CF_OEMTEXT` from
//!   `CF_UNICODETEXT`, and so on) and a `CF_DIB`/`CF_DIBV5` made from a `CF_BITMAP` are memory
//!   formats like any other and are saved and put back as such: their content is what a program
//!   reading them got. A `CF_BITMAP`-only clipboard therefore comes back as its DIB (from which
//!   Windows makes the bitmap again), but is reported `partial`, as the bitmap itself was not kept.
//! - A format whose data cannot be had now (delayed rendering the owner does not provide,
//!   `GetClipboardData` failing) is not saved (`partial`). A format whose data would take the
//!   saved total past `SAVE_LIMIT` is not saved either (`partial`).
//! - OLE's bookkeeping for the program that copied (`DataObject`, `Ole Private Data`,
//!   `OleClipboardPersistOnFlush`) is left out without counting as a loss: it describes that
//!   program's live data object, which the restored clipboard is not, and OLE reads the restored
//!   clipboard through its plain formats instead.

use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::IsHungAppWindow;

pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_DIB: u32 = 8;
pub const CF_PALETTE: u32 = 9;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
#[allow(dead_code)] // tests/paste_text.rs
pub const CF_HDROP: u32 = 15;
pub const CF_DIBV5: u32 = 17;
const CF_OWNERDISPLAY: u32 = 0x80;
const CF_DSPBITMAP: u32 = 0x82;
const CF_DSPMETAFILEPICT: u32 = 0x83;
const CF_DSPENHMETAFILE: u32 = 0x8E;
const CF_PRIVATEFIRST: u32 = 0x200;
const CF_GDIOBJLAST: u32 = 0x3FF;
/// Registered formats start here.
const REGISTERED_FIRST: u32 = 0xC000;

/// The registered formats written with the pasted text (spec-phase4a "`computer_paste_text`" 3):
/// clipboard monitors are asked to ignore it, and Windows keeps it out of the clipboard history
/// (Win+V) and the cloud clipboard.
pub const EXCLUDE_FROM_MONITORS: PCWSTR = w!("ExcludeClipboardContentFromMonitorProcessing");
pub const CAN_INCLUDE_IN_HISTORY: PCWSTR = w!("CanIncludeInClipboardHistory");
pub const CAN_UPLOAD_TO_CLOUD: PCWSTR = w!("CanUploadToCloudClipboard");

/// OLE's bookkeeping formats (see the module text).
const OLE_BOOKKEEPING: [&str; 3] = ["DataObject", "Ole Private Data", "OleClipboardPersistOnFlush"];

/// The most bytes saved in total; a format that would go past it is not saved.
const SAVE_LIMIT: usize = 256 * 1024 * 1024;

/// How often, and how far apart, opening the clipboard is tried before the paste gives up with
/// nothing sent (another program has it open).
pub const OPEN_ATTEMPTS: u32 = 5;
pub const OPEN_RETRY: Duration = Duration::from_millis(20);
/// The same for putting the user's clipboard back, which tries longer: giving up there leaves the
/// pasted text on the clipboard.
const RESTORE_ATTEMPTS: u32 = 25;

/// How the user's clipboard was put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restored {
    /// Every format it had is back (or made again by Windows from one that is).
    Full,
    /// Some formats could not be saved or put back (see the module text).
    Partial,
    /// Another program changed the clipboard after the text was put there, so it was left as that
    /// program made it.
    SkippedChanged,
    /// Another program kept the clipboard open (`RESTORE_ATTEMPTS`), so nothing was put back and
    /// the pasted text is still on it.
    NotRestored,
}

impl Restored {
    pub fn as_str(self) -> &'static str {
        match self {
            Restored::Full => "full",
            Restored::Partial => "partial",
            Restored::SkippedChanged => "skipped_changed",
            Restored::NotRestored => "not_restored",
        }
    }
}

/// The clipboard as it was: the formats saved, in order, with their bytes, and whether every format
/// it had is among them (or made again from one that is).
pub struct Saved {
    formats: Vec<(u32, Vec<u8>)>,
    complete: bool,
}

/// How `save` treats a format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A global memory block: copied.
    Memory,
    /// Anything else (a GDI object, owner display, a private handle): not saved.
    Other,
    /// OLE's bookkeeping: left out, not a loss.
    Bookkeeping,
}

fn kind(format: u32) -> Kind {
    match format {
        CF_BITMAP | CF_METAFILEPICT | CF_PALETTE | CF_ENHMETAFILE | CF_OWNERDISPLAY | CF_DSPBITMAP | CF_DSPMETAFILEPICT | CF_DSPENHMETAFILE => Kind::Other,
        CF_PRIVATEFIRST..=CF_GDIOBJLAST => Kind::Other,
        REGISTERED_FIRST.. if format_name(format).is_some_and(|name| OLE_BOOKKEEPING.contains(&name.as_str())) => Kind::Bookkeeping,
        _ => Kind::Memory,
    }
}

/// Whether Windows makes `format` again from a format saved before it (see the module text).
fn made_from_saved(format: u32, saved: &[(u32, Vec<u8>)]) -> bool {
    matches!(format, CF_BITMAP | CF_PALETTE) && saved.iter().any(|(saved, _)| matches!(*saved, CF_DIB | CF_DIBV5))
}

fn format_name(format: u32) -> Option<String> {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClipboardFormatNameW(format, &mut buffer) };
    (length > 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// The clipboard, opened by this thread (with no owner window: nothing here is rendered late, so
/// no window needs to answer for it); closed when dropped.
pub struct Open(());

impl Open {
    /// Tries `attempts` times, `OPEN_RETRY` apart; None when another program kept it open.
    pub fn new(attempts: u32) -> Option<Self> {
        for attempt in 0..attempts {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Open(()));
            }
            if attempt + 1 < attempts {
                std::thread::sleep(OPEN_RETRY);
            }
        }
        None
    }

    /// The clipboard as it is (see the module text for what is kept).
    pub fn save(&self) -> Saved {
        let mut formats: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut complete = true;
        let mut total = 0usize;
        let mut format = 0;
        loop {
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            match kind(format) {
                Kind::Bookkeeping => {}
                Kind::Other => complete &= made_from_saved(format, &formats),
                Kind::Memory => match copy(format) {
                    Some(bytes) if total + bytes.len() <= SAVE_LIMIT => {
                        total += bytes.len();
                        formats.push((format, bytes));
                    }
                    _ => complete = false,
                },
            }
        }
        Saved { formats, complete }
    }

    /// Whether the program that owns the clipboard is not answering (`IsHungAppWindow`): reading a
    /// format it renders late would wait for it.
    pub fn owner_hung(&self) -> bool {
        unsafe { GetClipboardOwner() }.is_ok_and(|owner| unsafe { IsHungAppWindow(owner) }.as_bool())
    }

    /// Ticket 97: the clipboard's `CF_UNICODETEXT` up to its first NUL (Windows makes it from
    /// `CF_TEXT` or `CF_OEMTEXT` when only those are there); None when there is no text or its data
    /// cannot be had. A lone surrogate becomes U+FFFD. No other format is read.
    pub fn unicode_text(&self) -> Option<String> {
        unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.ok()?;
        let bytes = copy(CF_UNICODETEXT)?;
        let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).take_while(|unit| *unit != 0).collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// Empties the clipboard; on failure it is as it was.
    pub fn empty(&self) -> Result<(), String> {
        unsafe { EmptyClipboard() }.map_err(|error| format!("EmptyClipboard failed ({error})"))
    }

    /// Puts `text` on the emptied clipboard (`empty`) as `CF_UNICODETEXT` (lone line feeds as
    /// CR LF, as Windows programs expect), with the three formats that keep it out of monitors, the
    /// history and the cloud clipboard (a DWORD 0 each) put there first.
    pub fn put_private_text(&self, text: &str) -> Result<(), String> {
        for name in [EXCLUDE_FROM_MONITORS, CAN_INCLUDE_IN_HISTORY, CAN_UPLOAD_TO_CLOUD] {
            let format = unsafe { RegisterClipboardFormatW(name) };
            if format == 0 {
                return Err("RegisterClipboardFormatW failed".to_string());
            }
            set(format, &0u32.to_le_bytes())?;
        }
        let bytes: Vec<u8> = crlf(text).encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        set(CF_UNICODETEXT, &bytes)
    }

    /// Empties the clipboard and puts the saved formats back, in order; false when any of them
    /// could not be put back.
    pub fn put_back(&self, saved: &Saved) -> bool {
        if unsafe { EmptyClipboard() }.is_err() {
            return false;
        }
        let mut all = true;
        for (format, bytes) in &saved.formats {
            all &= set(*format, bytes).is_ok();
        }
        all
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// `text` with every line feed not already after a carriage return made CR LF.
fn crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous = '\0';
    for character in text.chars() {
        if character == '\n' && previous != '\r' {
            out.push('\r');
        }
        out.push(character);
        previous = character;
    }
    out
}

/// The bytes of `format`'s global memory block, or None when it cannot be had.
fn copy(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let memory = HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        if size == 0 {
            return None;
        }
        let pointer = GlobalLock(memory) as *const u8;
        if pointer.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
        let _ = GlobalUnlock(memory);
        Some(bytes)
    }
}

/// Puts `bytes` on the open clipboard as `format`, in a new global memory block the clipboard then
/// owns (freed here when it is refused).
fn set(format: u32, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|error| format!("GlobalAlloc failed ({error})"))?;
        let pointer = GlobalLock(memory) as *mut u8;
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err("GlobalLock failed".to_string());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
        let _ = GlobalUnlock(memory);
        if let Err(error) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(format!("SetClipboardData failed ({error})"));
        }
        Ok(())
    }
}

/// Ticket 97: another program kept the clipboard open (`OPEN_ATTEMPTS`, as for a paste), or the
/// program that owns it is hung, so reading a format it renders late could wait for it.
pub struct Busy;

/// Ticket 97: the clipboard's text (`Open::unicode_text`), None without text.
pub fn read_text() -> Result<Option<String>, Busy> {
    let open = Open::new(OPEN_ATTEMPTS).ok_or(Busy)?;
    if open.owner_hung() {
        return Err(Busy);
    }
    Ok(open.unicode_text())
}

/// Why `Pasting::begin` changed nothing.
pub enum NotPut {
    /// Another program kept the clipboard open (`OPEN_ATTEMPTS`), or the program that owns it is
    /// hung (`Open::owner_hung`), so saving it could wait for that program.
    Busy,
    /// The text could not be written; the saved clipboard was put back at once (the message says
    /// when that failed too).
    Failed(String),
}

/// The text to paste on the clipboard in place of the user's (spec steps 2 and 3), put back by
/// `finish` (step 6), or when dropped, so every way out after the text was put there, a stop or a
/// failure included, puts the user's clipboard back.
pub struct Pasting {
    saved: Saved,
    /// The clipboard's sequence number once the text was there: any other number later means
    /// another program changed the clipboard since.
    ours: u32,
    restored: Option<Restored>,
}

impl Pasting {
    pub fn begin(text: &str) -> Result<Self, NotPut> {
        let open = Open::new(OPEN_ATTEMPTS).ok_or(NotPut::Busy)?;
        if open.owner_hung() {
            return Err(NotPut::Busy);
        }
        let saved = open.save();
        open.empty().map_err(NotPut::Failed)?;
        if let Err(error) = open.put_private_text(text) {
            if open.put_back(&saved) {
                return Err(NotPut::Failed(error));
            }
            return Err(NotPut::Failed(format!("{error}; the user's clipboard had been emptied and could not be put back in full")));
        }
        drop(open);
        // Read once the clipboard is closed: Windows counts the change only then (a number read
        // while it is open is not the one it has after). Another program that changes the
        // clipboard between the close and this read is taken for ours (review M1, open).
        Ok(Self { saved, ours: sequence(), restored: None })
    }

    /// Puts the user's clipboard back, once: unless another program changed it since the text was
    /// put there (checked with the clipboard open, so no change can come in between), in which case
    /// it is left as that program made it.
    pub fn finish(&mut self) -> Restored {
        if let Some(restored) = self.restored {
            return restored;
        }
        let restored = match Open::new(RESTORE_ATTEMPTS) {
            Some(_open) if sequence() != self.ours => Restored::SkippedChanged,
            Some(open) => {
                if open.put_back(&self.saved) && self.saved.complete { Restored::Full } else { Restored::Partial }
            }
            None if sequence() != self.ours => Restored::SkippedChanged,
            None => {
                let _ = writeln_stderr("clipboard: another program kept the clipboard open, so it could not be put back; the pasted text is still on it");
                Restored::NotRestored
            }
        };
        self.restored = Some(restored);
        restored
    }
}

impl Drop for Pasting {
    fn drop(&mut self) {
        self.finish();
    }
}

/// One line to stderr, ignoring a failed write (stderr may be gone when the server went away).
fn writeln_stderr(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    writeln!(std::io::stderr(), "{line}")
}
