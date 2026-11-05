import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const SERVER_NAME = "deskpass";
export const SERVER_VERSION = "0.1.0";
/**
 * Version 2 (ticket 59) adds `get_ui_tree`; version 3 (ticket 62) adds `click` on an `element`; version 4 (ticket 64) adds
 * `set_value`; version 5 (ticket 65) adds `element_action`; version 6 (tickets 77, 78) adds `paste_text` and
 * `list_apps`; version 7 (ticket 95) adds `ask_user`'s `allow_always` and its answer `allow_always`, and drops `grant_mac`'s
 * `key_path` (the helper keeps its key in its own folder, or the `--grants-dir` it is started with); version 8 (ticket 97)
 * adds `read_clipboard` and `ask_user`'s kind `read_clipboard`; version 9 adds `list_start_menu_apps` (ticket 116) and
 * `confirm_change` (ticket 114), which the settings server (ticket 115) needs; version 10 adds `app_identity`'s
 * `original_name`, without which a signed file is known by its path.
 */
export const PROTOCOL_VERSION = "10";

export const DEFAULT_REQUEST_TIMEOUT_MS = 10_000;
export const LAUNCH_APP_TIMEOUT_MS = 15_000;
/**
 * The helper stops starting characters 20 seconds into a type_text request and answers
 * input_failed with the count typed; its waits on the target end 5 seconds later, and restoring
 * the keyboard layout takes up to another half second. Timing out kills the helper before that
 * restore runs, so this stays clear of the helper's worst case (`TYPE_TEXT_BUDGET` in
 * `helper/src/input.rs`).
 */
export const TYPE_TEXT_TIMEOUT_MS = 30_000;
/**
 * Ticket 77: a paste_text request takes at most about 5 seconds for Ctrl+V (`PRESS_KEY_LIMIT` in
 * `helper/src/input.rs`), up to a second reading the focused element first, 0.3 s confirming, and
 * the clipboard's save and restore. Timing out kills the helper before it puts the user's
 * clipboard back, so this stays well clear of that (at least 7 s).
 */
export const PASTE_TEXT_TIMEOUT_MS = 10_000;
export const OVERLAY_IDLE_MS = 10_000;
/**
 * Ticket 135: a session holding the operator lock that has made no tool call for this long (counted
 * from the end of its last call; a call in progress is never cut short) ends its session as its exit
 * does and releases the lock. Its next call that needs the lock takes it again.
 */
export const LOCK_IDLE_MS = 120_000;
/** Ticket 76: the most steps one computer_batch call runs. */
export const BATCH_MAX_STEPS = 20;
/**
 * Ticket 76: how long a computer_batch call may run, from its first step. Checked only between
 * steps: a step in progress finishes under its own timeouts, and the next one is not started.
 */
export const BATCH_TIME_LIMIT_MS = 60_000;
/**
 * How long the server waits for the client to answer an approval question (elicitation). This is
 * the MCP SDK's default request timeout, stated here because it adds to the dialog's time when
 * elicitInput fails by timing out: at worst 60 s here, then 60 s in the dialog.
 */
export const ELICITATION_TIMEOUT_MS = 60_000;
/** How long the helper's approval dialog waits for the user before it closes as a refusal. */
export const ASK_USER_DIALOG_MS = 60_000;
/**
 * The server's wait for an `ask_user` answer. Longer than the dialog, which closes itself when it
 * times out, so waiting on the user never kills the helper.
 */
export const ASK_USER_TIMEOUT_MS = ASK_USER_DIALOG_MS + 15_000;
/**
 * Ticket 115: how long the confirm window of a loosening (`confirm_change`, the settings page's Restore default) waits
 * for the user before it closes as `timeout`; as long as the approval dialog's.
 */
export const CONFIRM_CHANGE_DIALOG_MS = 60_000;
/**
 * Ticket 115: a `confirm_change` is waited for this much longer than the `timeout_ms` it asks for, so the window
 * closes itself (`timeout`) before the server would give up on the helper and kill it.
 */
export const CONFIRM_CHANGE_MARGIN_MS = 15_000;

/** Ticket 89: the most characters (code points) of an `action_summary` the log keeps; the rest is cut. */
export const ACTION_SUMMARY_MAX_CHARS = 200;
/** Ticket 89: log files older than this many days are deleted. */
export const LOG_RETENTION_DAYS = 30;
/** Ticket 89: while the log files add up to more than this, the oldest are deleted. */
export const LOG_MAX_BYTES = 10 * 1024 * 1024;

export function defaultHelperPath(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  const candidates = [
    join(here, "deskpass-helper.exe"),
    join(here, "..", "dist", "deskpass-helper.exe"),
    join(here, "..", "helper", "target", "release", "deskpass-helper.exe"),
    join(here, "..", "helper", "target", "debug", "deskpass-helper.exe"),
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? candidates[0];
}
