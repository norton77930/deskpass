mod apps;
mod ask;
mod capture;
mod clipboard;
mod dpi;
mod esc;
mod grants;
mod identity;
mod idle;
mod input;
mod launch;
mod overlay;
mod popups;
mod protocol;
mod restore;
mod strings;
mod uia;
mod windows_list;

use std::io::{self, Write};
use std::time::Duration;

use serde_json::json;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows_list::process_alive;

use crate::protocol::{err, ok, Request};

/// Identifies the sources this helper was built from (see `build.rs`).
const BUILD_ID: &str = env!("HELPER_BUILD_ID");

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--build-id") {
        println!("{BUILD_ID}");
        return;
    }
    // Open-source ticket 03: the server's UI language (`--lang en|zh-TW`); the helper never detects it, and without one its texts are English.
    strings::set_from_args(std::env::args().skip(1));
    dpi::set_dpi_aware();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    esc::start_notifier();
    let server_pid = parse_server_pid();
    grants::set_dir_arg(parse_grants_dir());
    std::thread::spawn(move || watch_server(server_pid));
    let (incoming, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lines() {
            let Ok(line) = line else { break };
            // Acted on here, not when dispatched, so it stops the request being executed exactly
            // like a physical Esc does.
            if is_debug_stop(&line) {
                esc::stop();
            }
            // While the user is being asked, requests are refused here instead of queued, so none
            // runs during or after the question on the strength of having been sent during it.
            if let Some(refused) = ask::screen(&line) {
                write_stdout(&refused);
                continue;
            }
            if incoming.send(line).is_err() {
                break;
            }
        }
    });

    write_stdout(&json!({ "event": "ready" }));
    while let Ok(line) = requests.recv() {
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("bad request: {error}");
                continue;
            }
        };
        esc::begin_request(request.id);
        let asked = ask::shows_dialog(&request.method);
        let response = dispatch(request);
        if asked {
            ask::finished();
        }
        esc::finish_request(&response);
    }
    // stdin EOF: end a `press_key` still waiting in the background for its target (ticket 54),
    // which detaches and reopens the input method, then give a reopen `ImeGuard::drop` delayed to a
    // background thread a chance to land before the process exits (main returning does not wait
    // for other threads on its own).
    input::end_late_key_release();
    input::wait_for_pending_ime_reopens();
}

/// The protocol version `ping` reports, which the server's `PROTOCOL_VERSION` (src/config.ts)
/// must equal: 2 adds `get_ui_tree` (ticket 59), 3 `click` on an `element` (62), 4 `set_value`
/// (64), 5 `element_action` (65), 6 `list_apps` and `paste_text` (78, 77), and in the same
/// unreleased 6 `resolve_app_id`, `launch_app`'s `aumid` and its result's `kind` (79); 7
/// `ask_user`'s `allow_always` and its answer `allow_always`, and `grant_mac` without `key_path`
/// (the key in the helper's own folder, or `--grants-dir`) (95); 8 `read_clipboard` and
/// `ask_user`'s kind `read_clipboard` (97); 9 `list_start_menu_apps` (116) and `confirm_change`
/// (114), which the settings server uses (115); 10 `app_identity`'s `original_name`, without which a
/// signed file is known by its path.
const PROTOCOL_VERSION: &str = "10";

/// Methods that send input. While stopped they are answered `interrupted` without running.
/// `click` covers a click on an element too (ticket 62); `set_value` and `element_action` are the
/// element actions of tickets 64 and 65, listed ahead so they are stopped from the start;
/// `paste_text` is ticket 77's.
const INPUT_METHODS: [&str; 10] = ["activate_window", "click", "type_text", "press_key", "scroll", "drag", "move_mouse", "set_value", "element_action", "paste_text"];

/// Ticket 145: an input request delivered in the foreground, during which a physical single Esc
/// stops: every input method but a `click`, `set_value` or `element_action` whose `delivery_mode`
/// is background (one without it is foreground, `input::Delivery`).
fn is_foreground_input(method: &str, params: &serde_json::Value) -> bool {
    if !INPUT_METHODS.contains(&method) {
        return false;
    }
    let has_delivery = matches!(method, "click" | "set_value" | "element_action");
    !has_delivery || !matches!(input::Delivery::from_params(params), Ok(input::Delivery::Background))
}

fn dispatch(request: Request) -> protocol::Response {
    if INPUT_METHODS.contains(&request.method.as_str()) && esc::stopped() {
        return err(request.id, "interrupted", "Input is stopped until the server resumes it.");
    }
    let _foreground = is_foreground_input(&request.method, &request.params).then(esc::ForegroundAction::begin);
    match request.method.as_str() {
        "ping" => ok(request.id, json!({ "version": PROTOCOL_VERSION, "build": BUILD_ID })),
        "ask_user" => match ask::ask_user(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        // Ticket 114: the settings page's confirm window, the same dialog (see `ask`).
        "confirm_change" => match ask::confirm_change(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "resume" => match esc::resume(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "session_start" => match esc::session_start() {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "session_end" => {
            let _ = overlay::hide();
            match esc::session_end() {
                Ok(result) => ok(request.id, result),
                Err((code, message)) => err(request.id, &code, message),
            }
        }
        "overlay_show" => match overlay::show(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "overlay_hide" => match overlay::hide() {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "launch_app" => match launch::launch_app(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "list_windows" => match windows_list::list_windows() {
            Ok(windows) => ok(request.id, json!({ "windows": windows })),
            Err(message) => err(request.id, "internal", message),
        },
        "list_apps" => match apps::list_apps() {
            Ok(result) => ok(request.id, result),
            Err(message) => err(request.id, "internal", message),
        },
        // Ticket 116: every Start Menu app with its identity key, for the settings page's "All apps" view.
        "list_start_menu_apps" => ok(request.id, apps::list_start_menu_apps()),
        "resolve_app_id" => match apps::resolve_app_id(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        // Ticket 93: who an app is (its verified signer, or its package family), for its approval.
        "app_identity" => match identity::app_identity(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        // Ticket 94: the MACs of grants.json entries under the DPAPI-protected grants key, which never leaves the helper.
        "grant_mac" => match grants::grant_mac(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "process_info" => match windows_list::process_info(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "get_window" => {
            let window_id = request.params.get("window_id").and_then(|value| value.as_str()).unwrap_or("");
            match windows_list::get_window(window_id) {
                Ok(result) => ok(request.id, result),
                Err((code, message)) => err(request.id, &code, message),
            }
        }
        "activate_window" => match input::activate_window(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "type_text" => match input::type_text(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        // Ticket 97: the clipboard's text, `null` without text; never a picture or a file list.
        "read_clipboard" => match clipboard::read_text() {
            Ok(text) => ok(request.id, json!({ "text": text })),
            Err(clipboard::Busy) => err(
                request.id,
                "clipboard_busy",
                "Another program is using the clipboard (it kept it open, or is not responding), so it could not be read. Try again in a moment.",
            ),
        },
        "paste_text" => match input::paste_text(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "press_key" => match input::press_key(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "scroll" => match input::scroll(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "drag" => match input::drag(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "move_mouse" => match input::move_mouse(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        // The stdin reader already stopped input when it read this request.
        #[cfg(debug_assertions)]
        "debug_stop" => ok(request.id, json!({})),
        // Test builds only (ticket 82): holds up the next `ask_user` dialog's thread.
        #[cfg(debug_assertions)]
        "debug_hold_dialog" => match ask::debug_hold(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        // Test builds only (ticket 88): leaves the request thread's key state as ticket 82's
        // dialog did.
        #[cfg(debug_assertions)]
        "debug_attach_while_active" => match input::debug_attach_while_active() {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        // Test builds only (ticket 137): `input::prepare_background` on the window, then released.
        #[cfg(debug_assertions)]
        "debug_prepare_background" => match input::debug_prepare_background(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        // Test builds only (ticket 138): the next foreground restore fails.
        #[cfg(debug_assertions)]
        "debug_fail_next_restore" => ok(request.id, restore::debug_fail_next_restore()),
        // Test builds only (ticket 150): the first call of every later restore counts as refused.
        #[cfg(debug_assertions)]
        "debug_refuse_first_restore_call" => ok(request.id, restore::debug_refuse_first_restore_call()),
        // Test builds only (ticket 150): how many restores had their first call refused so far.
        #[cfg(debug_assertions)]
        "debug_refused_restores" => ok(request.id, restore::debug_refused_restores()),
        // Test builds only (ticket 143): the overlay is left out of screen capture no more, so a
        // test's screen capture shows the border as the user sees it.
        #[cfg(debug_assertions)]
        "debug_overlay_capturable" => ok(request.id, overlay::debug_capturable()),
        // Test builds only (ticket 149): marks physical typing or a held mouse button.
        #[cfg(debug_assertions)]
        "debug_user_input" => match idle::debug_user_input(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        "click" => match input::click(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "set_value" => match uia::set_value(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "element_action" => match uia::element_action(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "get_ui_tree" => match uia::get_ui_tree(&request.params) {
            Ok(result) => ok(request.id, result),
            Err(failure) => failure.into_response(request.id),
        },
        "capture_window" => match capture::capture_window(&request.params) {
            Ok(result) => ok(request.id, result),
            Err((code, message)) => err(request.id, &code, message),
        },
        _ => err(request.id, "invalid_params", "Unknown helper method."),
    }
}

fn write_stdout(value: &impl serde::Serialize) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{}", serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()));
    let _ = stdout.flush();
}

fn is_debug_stop(line: &str) -> bool {
    #[cfg(debug_assertions)]
    {
        serde_json::from_str::<Request>(line).is_ok_and(|request| request.method == "debug_stop")
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = line;
        false
    }
}

fn watch_server(pid: u32) {
    loop {
        std::thread::sleep(Duration::from_millis(500));
        if !process_alive(pid) {
            esc::stop();
            let started = std::time::Instant::now();
            while input::input_busy() && started.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(20));
            }
            // The server going away: `process::exit` below runs no destructors on other threads,
            // so end a `press_key` still waiting in the background (the stop already ends its
            // wait) and give a reopen `ImeGuard::drop` delayed to a background thread the same
            // chance.
            input::end_late_key_release();
            input::wait_for_pending_ime_reopens();
            std::process::exit(0);
        }
    }
}

/// Ticket 95: `--grants-dir <folder>`, where `grant_mac` keeps the grants key. Ticket 96: always
/// passed by the server (Node resolves the folder); without it `grant_mac` refuses. The flag with
/// no folder after it gives an empty path, which it refuses like any path that is not absolute.
fn parse_grants_dir() -> Option<std::path::PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--grants-dir" {
            return Some(args.next().map(std::path::PathBuf::from).unwrap_or_default());
        }
    }
    None
}

fn parse_server_pid() -> u32 {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--server-pid" {
            if let Some(value) = args.next() {
                if let Ok(pid) = value.parse() {
                    return pid;
                }
            }
        }
    }
    eprintln!("missing --server-pid");
    0
}

#[cfg(test)]
mod tests {
    use super::is_foreground_input;
    use serde_json::json;

    #[test]
    fn foreground_input_is_every_input_method_but_a_background_delivery() {
        assert!(is_foreground_input("type_text", &json!({})));
        assert!(is_foreground_input("click", &json!({ "x": 1, "y": 1 })), "a click without delivery_mode is foreground");
        assert!(is_foreground_input("set_value", &json!({ "delivery_mode": "foreground" })));
        assert!(!is_foreground_input("element_action", &json!({ "delivery_mode": "background" })));
        assert!(!is_foreground_input("click", &json!({ "delivery_mode": "background" })));
        assert!(!is_foreground_input("get_ui_tree", &json!({})), "a read is no input");
        assert!(!is_foreground_input("overlay_show", &json!({ "delivery_mode": "foreground" })));
    }
}
