//! Reference-gaps G4 (ticket 07): `read_text` reads an element's selection or whole text through
//! UI Automation without touching anything of the user's. The fixture's `--ui-text` Rich Edit holds
//! "ab😀cd needle xyz" with "😀cd" selected (a TextPattern); the `--ui-content` combo box has a value
//! and no text; the "Click me" button has neither; the `--ui-controls` password box is refused.
//!
//! A Rich Edit ends its text with a CR the tests do not rely on, so they compare the text without
//! trailing line breaks.
//!
//! Ticket 08: `offset`. The `--ui-text-long <segments>` fixture holds `long_text`, with surrogate pairs
//! where pieces end. The time of each read goes to stderr: run with `-- --nocapture` to see it, the
//! measured result the ticket asks for. No test asserts a time: a read over the budget is
//! `read_timeout`, which fails the normal-size test and is reported as a measurement by the large one.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetCursorPos, GetForegroundWindow, SendMessageW};

#[path = "interactive_lock.rs"]
mod interactive_lock;

const EM_GETSEL: u32 = 0x00B0;
/// The `--ui-controls` password box's text, which must be in no answer.
const PASSWORD_TEXT: &str = "CUMCP-secret-42";

struct Fixture {
    _fixture: KillOnDrop,
    helper: Helper,
    window_id: String,
    tree: Value,
}

impl Fixture {
    fn start() -> Self {
        Self::start_with(&[])
    }

    fn start_with(extra: &[&str]) -> Self {
        let fixture = KillOnDrop(Command::new(env!("CARGO_BIN_EXE_fixture-window")).args(["--ui-controls", "--ui-content", "--ui-text"]).args(extra).spawn().expect("fixture"));
        let mut helper = Helper::spawn();
        let window_id = wait_for_fixture(&mut helper);
        let read = helper.request("get_ui_tree", json!({ "window_id": window_id }));
        assert_eq!(read["ok"], true, "{read}");
        let tree = read["result"].clone();
        Self { _fixture: fixture, helper, window_id, tree }
    }

    fn index_of(&self, what: &str, matches: impl Fn(&Value) -> bool) -> u64 {
        let elements = self.tree["elements"].as_array().expect("elements");
        elements.iter().find(|element| matches(element)).unwrap_or_else(|| panic!("no {what} in {}", self.tree))["index"].as_u64().expect("index")
    }

    fn document(&self) -> u64 {
        self.index_of("document (the Rich Edit)", |element| element["role"] == "document")
    }

    fn combo(&self) -> u64 {
        self.index_of("combo box", |element| element["role"] == "combo box")
    }

    fn button(&self) -> u64 {
        self.index_of("\"Click me\" button", |element| element["role"] == "button" && element["name"] == "Click me")
    }

    fn password(&self) -> u64 {
        self.index_of("password box", |element| element["states"].as_array().is_some_and(|states| states.iter().any(|state| state == "password")))
    }

    fn read(&mut self, index: u64, extra: Value) -> Value {
        let mut params = json!({ "window_id": self.window_id, "tree_id": self.tree["tree_id"], "index": index });
        for (key, value) in extra.as_object().expect("object") {
            params[key] = value.clone();
        }
        self.helper.request("read_text", params)
    }
}

fn text_of(answer: &Value) -> String {
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"]["text"].as_str().expect("text").trim_end_matches(['\r', '\n']).to_string()
}

#[test]
fn a_text_pattern_gives_its_document_and_its_selection() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let rich = fixture.document();

    // The document is the default scope.
    let document = fixture.read(rich, json!({}));
    assert_eq!(text_of(&document), "ab😀cd needle xyz", "{document}");
    assert_eq!(document["result"]["source"], "text_pattern", "{document}");
    assert_eq!(document["result"]["more"], false, "{document}");
    assert!(document["result"].get("ranges").is_none(), "{document}");
    let explicit = fixture.read(rich, json!({ "scope": "document" }));
    assert_eq!(text_of(&explicit), "ab😀cd needle xyz", "{explicit}");

    // The selection the fixture made, one range.
    let selection = fixture.read(rich, json!({ "scope": "selection" }));
    assert_eq!(text_of(&selection), "😀cd", "{selection}");
    assert_eq!(selection["result"]["source"], "text_pattern", "{selection}");
    assert_eq!(selection["result"]["ranges"], 1, "{selection}");
    assert_eq!(selection["result"]["more"], false, "{selection}");
}

#[test]
fn text_past_max_chars_is_cut_on_code_points_and_says_more() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let rich = fixture.document();

    // "ab" then an emoji of two UTF-16 units: 3 characters keep it whole, 2 stop before it.
    let three = fixture.read(rich, json!({ "max_chars": 3 }));
    assert_eq!(text_of(&three), "ab😀", "{three}");
    assert_eq!(three["result"]["more"], true, "{three}");
    let two = fixture.read(rich, json!({ "max_chars": 2 }));
    assert_eq!(text_of(&two), "ab", "{two}");
    assert_eq!(two["result"]["more"], true, "{two}");
    // Long enough for all of it (the Rich Edit's own trailing CR may or may not count).
    let all = fixture.read(rich, json!({ "max_chars": 17 }));
    assert_eq!(text_of(&all), "ab😀cd needle xyz", "{all}");
    let roomy = fixture.read(rich, json!({ "max_chars": 20_000 }));
    assert_eq!(roomy["result"]["more"], false, "{roomy}");

    let selected = fixture.read(rich, json!({ "scope": "selection", "max_chars": 1 }));
    assert_eq!(text_of(&selected), "😀", "{selected}");
    assert_eq!(selected["result"]["more"], true, "{selected}");

    for bad in [json!({ "max_chars": 0 }), json!({ "max_chars": 20_001 }), json!({ "max_chars": 1.5 }), json!({ "max_chars": "5" }), json!({ "scope": "all" }), json!({ "scope": 1 })] {
        let refused = fixture.read(rich, bad.clone());
        assert_eq!(refused["error"]["code"], "invalid_params", "{bad}: {refused}");
    }
}

#[test]
fn a_value_is_the_document_of_an_element_without_a_text_pattern_but_never_its_selection() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let combo = fixture.combo();

    let document = fixture.read(combo, json!({ "scope": "document" }));
    assert_eq!(text_of(&document), "Red", "{document}");
    assert_eq!(document["result"]["source"], "value_pattern", "{document}");
    // A selection is read from the text pattern only: no fallback to the value.
    let selection = fixture.read(combo, json!({ "scope": "selection" }));
    assert_eq!(selection["ok"], false, "{selection}");
    assert_eq!(selection["error"]["code"], "not_supported", "{selection}");
}

#[test]
fn an_element_with_neither_pattern_is_not_supported() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let button = fixture.button();
    for scope in ["document", "selection"] {
        let read = fixture.read(button, json!({ "scope": scope }));
        assert_eq!(read["ok"], false, "{scope}: {read}");
        assert_eq!(read["error"]["code"], "not_supported", "{scope}: {read}");
    }
}

#[test]
fn a_password_field_is_refused_before_anything_is_read() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let password = fixture.password();
    for scope in ["document", "selection"] {
        let read = fixture.read(password, json!({ "scope": scope }));
        assert_eq!(read["ok"], false, "{scope}: {read}");
        assert_eq!(read["error"]["code"], "password_field", "{scope}: {read}");
        assert!(!read.to_string().contains(PASSWORD_TEXT), "{scope}: the answer carries the password: {read}");
    }
}

#[test]
fn a_read_leaves_the_clipboard_the_foreground_the_pointer_and_the_selection_as_they_were() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let rich = fixture.document();
    let combo = fixture.combo();
    let window = HWND(fixture.window_id.parse::<isize>().expect("window id") as *mut core::ffi::c_void);
    let rich_window = unsafe { FindWindowExW(Some(window), None, w!("RICHEDIT50W"), None) }.expect("the Rich Edit");
    // The fixture has just been created; let it settle so nothing it does itself shows up below.
    std::thread::sleep(Duration::from_millis(500));

    let state = || unsafe {
        let mut at = POINT::default();
        GetCursorPos(&mut at).expect("cursor");
        // No pointers: the start in the low word, the end in the high word.
        let selection = SendMessageW(rich_window, EM_GETSEL, Some(WPARAM(0)), Some(LPARAM(0))).0 as usize;
        (GetClipboardSequenceNumber(), GetForegroundWindow().0 as isize, (at.x, at.y), (selection & 0xFFFF, (selection >> 16) & 0xFFFF))
    };
    let before = state();
    assert_eq!(before.3, (2, 6), "the fixture's selection");
    for (index, scope) in [(rich, "document"), (rich, "selection"), (combo, "document"), (rich, "document")] {
        let read = fixture.read(index, json!({ "scope": scope }));
        assert_eq!(read["ok"], true, "{scope}: {read}");
    }
    assert_eq!(state(), before, "(clipboard sequence number, foreground window, pointer, selection) changed");
}

#[test]
fn the_checks_of_every_other_reading_tool_apply() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let rich = fixture.document();

    let unknown = fixture.helper.request("read_text", json!({ "window_id": fixture.window_id, "tree_id": "t999-zzzzz", "index": rich }));
    assert_eq!(unknown["error"]["code"], "no_ui_tree", "{unknown}");
    let past = fixture.read(10_000, json!({}));
    assert_eq!(past["error"]["code"], "invalid_params", "{past}");
    let other_app = fixture.read(rich, json!({ "expected_app": "C:\\Windows\\System32\\notepad.exe" }));
    assert_eq!(other_app["error"]["code"], "app_changed", "{other_app}");
    let no_window = fixture.helper.request("read_text", json!({ "window_id": "not-a-handle", "tree_id": fixture.tree["tree_id"], "index": rich }));
    assert_eq!(no_window["error"]["code"], "invalid_params", "{no_window}");

    // A stopped helper reads nothing, and reads again after `resume`.
    let stopped = fixture.helper.request("debug_stop", json!({}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    let read = fixture.read(rich, json!({}));
    assert_eq!(read["ok"], false, "{read}");
    assert_eq!(read["error"]["code"], "interrupted", "{read}");
    let resumed = fixture.helper.request("resume", json!({}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let read = fixture.read(rich, json!({}));
    assert_eq!(text_of(&read), "ab😀cd needle xyz", "{read}");
}

/// The fixture's `long_ui_text`: "x", then per segment three emoji and its number.
fn long_text(segments: usize) -> String {
    let mut text = String::from("x");
    for number in 0..segments {
        text.push_str(&format!("😀😀😀{:04}", number % 10_000));
    }
    text
}

/// Reads a whole document in order by `offset`, checking and printing each piece, and gives them.
fn read_in_order(fixture: &mut Fixture, index: u64, max_chars: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut offset = 0;
    loop {
        let started = Instant::now();
        let read = fixture.read(index, json!({ "offset": offset, "max_chars": max_chars }));
        eprintln!("read_text max_chars {max_chars} offset {offset}: {:?}", started.elapsed());
        assert_eq!(read["ok"], true, "offset {offset}: {}", read["error"]);
        let piece = read["result"]["text"].as_str().expect("text").to_string();
        let chars = piece.chars().count();
        assert!(chars <= max_chars, "offset {offset}: {chars} characters");
        // A pair cut in two reaches the answer as a replacement character.
        assert!(!piece.contains('\u{FFFD}'), "offset {offset}: half a surrogate pair");
        offset += chars;
        pieces.push(piece);
        if read["result"]["more"] == false {
            return pieces;
        }
        assert!(chars > 0, "offset {offset}: more with no text");
    }
}

#[test]
fn a_long_document_read_in_order_by_offset_joins_to_the_original() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start_with(&["--ui-text-long", "9000"]);
    let rich = fixture.document();
    let original = long_text(9_000);
    assert!(original.chars().count() > 20_000);

    // The default piece, and a size that puts the cuts elsewhere among the pairs.
    for (max_chars, least_pieces) in [(20_000, 4), (7_777, 9)] {
        let pieces = read_in_order(&mut fixture, rich, max_chars);
        assert!(pieces.len() >= least_pieces, "max_chars {max_chars}: {} pieces", pieces.len());
        let joined = pieces.concat();
        // Not assert_eq!: a failure should not print a whole document.
        assert!(joined.trim_end_matches(['\r', '\n']) == original, "max_chars {max_chars}: the pieces do not join to the original ({} and {} characters)", joined.chars().count(), original.chars().count());
    }

    // At the end and past it: a normal result with empty text.
    let length = read_in_order(&mut fixture, rich, 20_000).concat().chars().count();
    for offset in [length, length + 1, 1_000_000_000, 5_000_000_000_u64 as usize] {
        let past = fixture.read(rich, json!({ "offset": offset }));
        assert_eq!(past["ok"], true, "offset {offset}: {}", past["error"]);
        assert_eq!(past["result"]["text"], "", "offset {offset}");
        assert_eq!(past["result"]["more"], false, "offset {offset}");
    }
}

/// How the budget holds for later pieces of a large document: the ticket asks for the measured
/// result. Over the budget is reported, whole: `read_timeout` and no text.
#[test]
fn a_later_piece_of_a_large_document_is_whole_or_read_timeout_and_its_time_is_printed() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    // About 420,000 characters, which the fixture fills in about 14 seconds; the time grows faster
    // than the size, and 1.05 million did not fill within wait_for_fixture's 60 seconds.
    let segments = 60_000;
    let segments_arg = segments.to_string();
    let mut fixture = Fixture::start_with(&["--ui-text-long", &segments_arg]);
    let rich = fixture.document();
    let original: Vec<char> = long_text(segments).chars().collect();

    for offset in [0, original.len() / 2, original.len() - 10_000] {
        let started = Instant::now();
        let read = fixture.read(rich, json!({ "offset": offset }));
        let took = started.elapsed();
        if read["error"]["code"] == "read_timeout" {
            eprintln!("read_text of {} characters, offset {offset}: read_timeout after {took:?}", original.len());
            assert!(read.get("result").is_none(), "partial text with a timeout: offset {offset}");
            continue;
        }
        eprintln!("read_text of {} characters, offset {offset}: {took:?}", original.len());
        assert_eq!(read["ok"], true, "offset {offset}: {}", read["error"]);
        let expected: String = original.iter().skip(offset).take(20_000).collect();
        let piece = read["result"]["text"].as_str().expect("text");
        assert!(piece.trim_end_matches(['\r', '\n']) == expected, "offset {offset}: not the piece of the original");
    }
}

#[test]
fn an_offset_counts_code_points_in_a_document_a_value_and_a_joined_selection() {
    if !interactive_desktop() {
        eprintln!("SKIP interactive: no desktop");
        return;
    }
    let _interactive = interactive_lock::acquire();
    let mut fixture = Fixture::start();
    let rich = fixture.document();
    let combo = fixture.combo();
    let password = fixture.password();

    // The emoji is one character.
    let emoji = fixture.read(rich, json!({ "offset": 2, "max_chars": 1 }));
    assert_eq!(text_of(&emoji), "😀", "{emoji}");
    assert_eq!(emoji["result"]["more"], true, "{emoji}");
    let after = fixture.read(rich, json!({ "offset": 3, "max_chars": 2 }));
    assert_eq!(text_of(&after), "cd", "{after}");
    let past = fixture.read(rich, json!({ "offset": 1000 }));
    assert_eq!(past["result"]["text"], "", "{past}");
    assert_eq!(past["result"]["more"], false, "{past}");

    // A value can only be cut, by the same rule.
    let value = fixture.read(combo, json!({ "offset": 1 }));
    assert_eq!(text_of(&value), "ed", "{value}");
    assert_eq!(value["result"]["source"], "value_pattern", "{value}");
    let past_value = fixture.read(combo, json!({ "offset": 3 }));
    assert_eq!(past_value["result"]["text"], "", "{past_value}");
    assert_eq!(past_value["result"]["more"], false, "{past_value}");

    // A selection: the range still counts when the offset is past its text.
    let selected = fixture.read(rich, json!({ "scope": "selection", "offset": 1 }));
    assert_eq!(text_of(&selected), "cd", "{selected}");
    assert_eq!(selected["result"]["ranges"], 1, "{selected}");
    let past_selected = fixture.read(rich, json!({ "scope": "selection", "offset": 100 }));
    assert_eq!(past_selected["result"]["text"], "", "{past_selected}");
    assert_eq!(past_selected["result"]["more"], false, "{past_selected}");
    assert_eq!(past_selected["result"]["ranges"], 1, "{past_selected}");

    for bad in [json!(-1), json!(1.5), json!("1"), json!(null)] {
        let refused = fixture.read(rich, json!({ "offset": bad }));
        assert_eq!(refused["error"]["code"], "invalid_params", "{bad}: {refused}");
    }
    // A password field is still decided before anything is read.
    let refused = fixture.read(password, json!({ "offset": 1 }));
    assert_eq!(refused["error"]["code"], "password_field", "{refused}");
    assert!(!refused.to_string().contains(PASSWORD_TEXT), "{refused}");
}

fn wait_for_fixture(helper: &mut Helper) -> String {
    // Long: a fixture with a large document takes its time to fill it.
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let listed = helper.request("list_windows", json!({}));
        let found = listed["result"]["windows"].as_array().and_then(|windows| {
            windows.iter().find_map(|window| {
                window["title"].as_str().filter(|title| title.starts_with("CUMCP_FIXTURE")).map(|_| window["window_id"].as_str().unwrap_or("").to_string())
            })
        });
        if let Some(found) = found {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the fixture was not listed");
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

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
