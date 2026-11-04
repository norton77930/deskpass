import { randomUUID } from "node:crypto";
import { appendFile, mkdir, readdir, stat, unlink } from "node:fs/promises";
import path from "node:path";
import { ACTION_SUMMARY_MAX_CHARS, LOG_MAX_BYTES, LOG_RETENTION_DAYS } from "./config.ts";
import { defaultLockDir } from "./lock.ts";

/**
 * Ticket 89: the server's event log, one JSONL file a day under
 * `%LOCALAPPDATA%\deskpass\logs\YYYY-MM-DD.jsonl`, read by the control panel later.
 * An event never holds what the user's apps show or what was sent to them: no typed or pasted
 * text, no keys, no clipboard, screenshot or UI-tree content, no window titles; only the app
 * (ticket 97: of a clipboard read, only its character count).
 * A new event type is one more member of `LogEvent` (ticket 90's `approval`, then the grant
 * events), written through the same `record`.
 */

/** The app a step acted on: its path, its approval key, and its tier (null until ticket 91). */
export interface LoggedApp {
  path: string;
  key: string;
  tier: string | null;
}

export interface ToolCallEvent {
  event: "tool_call";
  /** The tool's full name (`computer_click`); for a batch step, the step's tool. */
  tool: string;
  /** The step's number in its batch, from 1; null for a tool called alone and for the batch itself. */
  step: number | null;
  app: LoggedApp | null;
  /** The model's `action_summary`, cleaned (`cleanActionSummary`), or null. */
  action_summary: string | null;
  /** `ok`, or the failure's code. */
  result: string;
  /** Ticket 138: how the step's action was delivered, for a step that decided it; absent otherwise. */
  delivery?: "background" | "foreground";
  /** Ticket 138: the reason of a `background_unavailable` result; absent otherwise. */
  reason?: string;
  /** Ticket 138: the helper gave the foreground back after a background action; absent otherwise. */
  foreground_restored?: true;
  /** Ticket 138: the helper could not give the foreground back after a background action; absent otherwise. */
  restore_failed?: true;
  /** Ticket 149: how long a background action waited for the user to be idle (0: no wait); absent for other steps. */
  waited_for_idle_ms?: number;
  /** Ticket 149: that wait reached its 3 s cap and the action went ahead anyway; absent otherwise. */
  idle_wait_capped?: true;
  duration_ms: number;
}

/**
 * Ticket 90: where an approval decision came from. `session` is this server session's memory (an
 * earlier allow, or a remembered refusal); `grant` is an always-allow grant (ticket 94).
 */
export type ApprovalChannel = "elicitation" | "dialog" | "grant" | "session";

/** Ticket 90: how an approval was decided; `allow_always` is an always-allow grant used (ticket 94) or given (ticket 95). */
export type ApprovalAnswer = "allow_once" | "allow_always" | "refuse" | "timeout" | "unavailable" | "refused_earlier" | "blocked";

/**
 * Ticket 90: one approval decision for an app: asked or not, through which channel, and the answer.
 * Ticket 97: reading the clipboard is approved about no app: `app` is null and `subject` is
 * `clipboard` (an app's approval has no `subject`).
 */
export interface ApprovalEvent {
  event: "approval";
  app: LoggedApp | null;
  subject?: "clipboard";
  /** Null when no channel was involved: a blocked app is refused before any question. */
  channel: ApprovalChannel | null;
  answer: ApprovalAnswer;
}

/**
 * Ticket 94: why an entry of grants.json (or the file itself) was not taken: `malformed` (not the
 * fields of an entry, or no MAC: added by hand), `bad_mac` (edited, or signed under another key),
 * `not_signed` (a grant for an app without a signer or package, which cannot be always allowed),
 * `not_higher` (a raise that would lower or keep the built-in tier), `unreadable` (the file is not
 * a version 1 grants file, or is over 1 MB), `rollback` (the file's generation is lower than one
 * this server accepted). For the whole file (`entry: "file"`), `bad_mac` is a `file_mac` that does
 * not verify (entries added, removed, reordered or re-signed) and `not_signed` a grants file, even
 * an empty one, without `file_mac`.
 */
export type GrantInvalidReason = "malformed" | "bad_mac" | "not_signed" | "not_higher" | "unreadable" | "rollback";

/** Ticket 94: an entry of grants.json that does not count, logged once per reading of the file. */
export interface GrantInvalidEvent {
  event: "grant_invalid";
  /** Which list the entry is in; `file` for the whole file. */
  entry: "grant" | "raised" | "file";
  /** The entry's app key (its first 300 characters), or null when it has none. */
  key: string | null;
  reason: GrantInvalidReason;
}

/** Ticket 94: the grants key could not be unlocked (or was gone): every entry, `entries` of them, is void. */
export interface GrantKeyLostEvent {
  event: "grant_key_lost";
  entries: number;
}

/**
 * Ticket 95: a verified always-allow grant given for this executable (by its path) is not used,
 * since the app's identity now is another (`current_key`): its signer changed, its signature no
 * longer verifies, or its identity could not be had. Logged when the user is asked instead.
 */
export interface GrantMismatchEvent {
  event: "grant_mismatch";
  /** The grant's key. */
  key: string;
  /** The app's identity key now. */
  current_key: string;
  /** The executable checked. */
  path: string;
}

export type GrantLogEvent = GrantInvalidEvent | GrantKeyLostEvent | GrantMismatchEvent;

/** Ticket 97: the clipboard's text was read: how many characters (code points) were returned, never the text. */
export interface ClipboardReadEvent {
  event: "clipboard_read";
  chars: number;
  /** Whether the clipboard held more than was returned. */
  truncated: boolean;
}

/**
 * Ticket 113: a change the user made on the settings page (session `settings`), named as the
 * native control panel named its own: an app raised to `tier` (`grant_removed`: its always-allow
 * went with it), a grant revoked, and (ticket 115) a raise removed. `path` is the app's executable
 * when the page knows one, else omitted.
 */
export interface TierRaisedEvent {
  event: "tier_raised";
  key: string;
  path?: string;
  tier: "high" | "blocked";
  grant_removed: boolean;
}

export interface GrantRevokedEvent {
  event: "grant_revoked";
  key: string;
  path?: string;
}

export interface TierRaiseRemovedEvent {
  event: "tier_raise_removed";
  key: string;
  path?: string;
}

export type SettingsChangeEvent = TierRaisedEvent | GrantRevokedEvent | TierRaiseRemovedEvent;

export type LogEvent = ToolCallEvent | ApprovalEvent | GrantLogEvent | ClipboardReadEvent | SettingsChangeEvent;

/** What every line has before the event's own fields. */
interface Stamped {
  time: string;
  session: string;
}

export function defaultLogDir(): string {
  return path.join(defaultLockDir(), "logs");
}

const DAY_FILE = /^(\d{4}-\d{2}-\d{2})\.jsonl$/;
const DAY_MS = 24 * 60 * 60 * 1000;

/** The local calendar day of `time`, as the file name has it. */
function dayOf(time: number): string {
  const date = new Date(time);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/** Line breaks and other control characters (C0, DEL, C1, and the Unicode line and paragraph separators). */
const CONTROL = /[\p{Cc}\u2028\u2029]/gu;

/**
 * Ticket 89: `action_summary` as the log keeps it: text without line breaks or control
 * characters, cut to its first 200 characters (code points); anything else, or nothing left, null.
 */
export function cleanActionSummary(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const cleaned = Array.from(value.replace(CONTROL, "")).slice(0, ACTION_SUMMARY_MAX_CHARS).join("");
  return cleaned.length > 0 ? cleaned : null;
}

export interface EventLogOptions {
  dir?: string;
  /** The clock event times and file days are taken from; for tests. */
  now?: () => number;
  /** Ticket 113: what every line's `session` is; a new random id (an MCP server session's) by default, `settings` for the settings server. */
  session?: string;
}

/**
 * Writes the events in order, each awaited by its caller, and never throws: a failure to write
 * (a full disk, no permission) is said once on stderr and the event is dropped. Old files are
 * cleared when the log opens and on the first write of each day after (`sweep`).
 */
export class EventLog {
  readonly session: string;
  private readonly dir: string;
  private readonly now: () => number;
  private queue: Promise<void>;
  private sweptDay: string;
  private reported = false;

  constructor(options: EventLogOptions = {}) {
    this.session = options.session ?? randomUUID();
    this.dir = options.dir ?? defaultLogDir();
    this.now = options.now ?? Date.now;
    this.sweptDay = dayOf(this.now());
    this.queue = this.sweep(this.now());
  }

  record(event: LogEvent): Promise<void> {
    const time = this.now();
    const line: Stamped & LogEvent = { time: new Date(time).toISOString(), session: this.session, ...event };
    this.queue = this.queue.then(() => this.write(time, `${JSON.stringify(line)}\n`));
    return this.queue;
  }

  /** Waits for the writes and the clearing already started. */
  flush(): Promise<void> {
    return this.queue;
  }

  private async write(time: number, line: string): Promise<void> {
    const day = dayOf(time);
    if (day !== this.sweptDay) {
      this.sweptDay = day;
      await this.sweep(time);
    }
    try {
      await mkdir(this.dir, { recursive: true });
      await appendFile(path.join(this.dir, `${day}.jsonl`), line, "utf8");
    } catch (error) {
      this.report(`could not write the tool log in ${this.dir}`, error);
    }
  }

  /**
   * Deletes the day files older than 30 days, then, while the files add up to more than 10 MB,
   * the oldest; today's file is kept. Only `YYYY-MM-DD.jsonl` files are touched.
   */
  private async sweep(time: number): Promise<void> {
    try {
      let names: string[];
      try {
        names = await readdir(this.dir);
      } catch (error) {
        if ((error as { code?: unknown }).code === "ENOENT") return;
        throw error;
      }
      const today = dayOf(time);
      const oldest = dayOf(time - LOG_RETENTION_DAYS * DAY_MS);
      const days = names.filter((name) => DAY_FILE.test(name)).sort();
      const kept: { name: string; size: number }[] = [];
      for (const name of days) {
        const file = path.join(this.dir, name);
        if (name.slice(0, 10) < oldest) await unlink(file);
        else kept.push({ name, size: (await stat(file)).size });
      }
      let total = kept.reduce((sum, file) => sum + file.size, 0);
      for (const file of kept) {
        if (total <= LOG_MAX_BYTES || file.name.slice(0, 10) === today) break;
        await unlink(path.join(this.dir, file.name));
        total -= file.size;
      }
    } catch (error) {
      this.report(`could not clear old tool logs in ${this.dir}`, error);
    }
  }

  private report(what: string, error: unknown): void {
    if (this.reported) return;
    this.reported = true;
    const reason = error instanceof Error ? error.message : String(error);
    console.error(`deskpass: ${what} (${reason}); tools keep working, and further log failures are not reported.`);
  }
}
