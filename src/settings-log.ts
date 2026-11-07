import { open, readdir } from "node:fs/promises";
import path from "node:path";
import { LOG_MAX_BYTES, LOG_RETENTION_DAYS } from "./config.ts";
import { fill, SETTINGS_TEXT, type SettingsText } from "./settings-strings.ts";

/**
 * Ticket 117 (Node side of the settings page's Log tab): reads the log folder
 * (`<dataDir>\logs\YYYY-MM-DD.jsonl`, written by `EventLog` in log.ts and by the settings page) and
 * turns a day into groups of sentences in the interface language (zh-TW or English, open-source ticket 02), as the native panel's log tab did (ticket 105;
 * the retired helper panel.rs, `describe_event`). A result is a separate `tag`, not words of the sentence.
 * Only the events' fixed fields are read: a row never holds typed text (the log has none).
 */

/** The words of the log tab are `logRows` of the settings table of the interface language (settings-strings.ts). */
export type LogRowText = SettingsText["logRows"];

/** How many days before today the tab offers a day file of. */
export const LOG_PAGE_DAYS = 7;
/** The most rows of a day shown (the newest), as in the native panel. */
export const LOG_ROWS_MAX = 2000;

/** The pseudo app key of the clipboard's approval, which is about no app. */
const CLIPBOARD_KEY = "subject:clipboard";
const DAY_FILE = /^(\d{4}-\d{2}-\d{2})\.jsonl$/;
const DAY = /^\d{4}-\d{2}-\d{2}$/;
const DAY_MS = 24 * 60 * 60 * 1000;

/** One line of a group: `time` is local HH:MM, `tag` the result as a label (none for a change you made). */
export interface LogRow {
  time: string;
  sentence: string;
  tag?: string;
  /** Ticket 146: how the action was delivered, and the mark (Background or Foreground) the row shows; none for an event that decided no delivery. */
  delivery?: "background" | "foreground";
  mark?: string;
}

/** Consecutive rows about one app; `key` is its identity key (null: no app), `first` and `last` are the first and last row's time. */
export interface LogGroup {
  key: string | null;
  app: string;
  count: number;
  first: string;
  last: string;
  rows: LogRow[];
}

/** An entry of the app filter: the key to pass as `options.app`, and the name to show. */
export interface LogApp {
  key: string;
  name: string;
}

export interface LogDay {
  day: string;
  /** In the order the events were logged. */
  groups: LogGroup[];
  /** The apps that appear that day (never narrowed by the filter), by name. */
  apps: LogApp[];
  /** True when the day had more than `LOG_ROWS_MAX` rows and only the newest are in `groups`. */
  truncated: boolean;
}

export interface LogReadOptions {
  /** Keep only this app's groups (a `LogApp.key`). */
  app?: string;
  /** An ISO UTC time as the local `HH:MM`; the machine's zone by default. For tests. */
  localTime?: (iso: string) => string;
  /** The name of an app by its key and executable (the Start Menu's names, the retired helper panel.rs `app_name`); its file name by default. */
  appName?: (key: string, path: string | null) => string;
  /** The words of the sentences and tags; zh-TW by default. */
  text?: LogRowText;
}

/** The day files' days (`YYYY-MM-DD`) in `dataDir\logs`, oldest first; none when the folder is not there. */
async function dayFiles(dataDir: string): Promise<string[]> {
  try {
    return (await readdir(path.join(dataDir, "logs")))
      .map((name) => DAY_FILE.exec(name)?.[1])
      .filter((day): day is string => day !== undefined)
      .sort();
  } catch {
    return [];
  }
}

/** The calendar day `n` days before `day`. */
function daysBefore(day: string, n: number): string {
  const [year, month, date] = day.split("-").map(Number) as [number, number, number];
  return new Date(Date.UTC(year, month - 1, date) - n * DAY_MS).toISOString().slice(0, 10);
}

/** The days to pick from: `today` (always first, even with no file), then the day files of the 7 days before it, newest first. */
export async function logDays(dataDir: string, today: string): Promise<string[]> {
  if (!DAY.test(today)) return [today];
  const oldest = daysBefore(today, LOG_PAGE_DAYS);
  const others = (await dayFiles(dataDir)).filter((day) => day >= oldest && day < today).reverse();
  return [today, ...others];
}

/** A day file's text, up to the log's size limit; "" when it cannot be read. */
async function readCapped(file: string): Promise<string> {
  try {
    const handle = await open(file, "r");
    try {
      const buffer = Buffer.alloc(Math.min((await handle.stat()).size, LOG_MAX_BYTES));
      const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
      return buffer.subarray(0, bytesRead).toString("utf8");
    } finally {
      await handle.close();
    }
  } catch {
    return "";
  }
}

/** A day's rows, in file order, each with the app it is about. */
interface Described {
  /** The app's key (or the clipboard's pseudo key); null for an event about no app. */
  key: string | null;
  app: string;
  row: LogRow;
}

type Fields = Record<string, unknown>;

const str = (value: unknown): string => (typeof value === "string" ? value : "");

/** The local `HH:MM` of an ISO UTC time (the machine's zone), or `noTime`. */
function localClock(iso: string, noTime: string): string {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return noTime;
  const date = new Date(ms);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

function baseName(file: string): string {
  return path.win32.basename(file.replaceAll("/", "\\"));
}

/**
 * An app named without the Start Menu (the end of the retired helper panel.rs `app_name`): its executable's
 * file name, else its signer key's file name, else its key's, else a package's name; never a signer or family.
 */
function fileName(key: string, appPath: string | null): string {
  const keyFile = key.startsWith("signer:") ? key.slice(key.lastIndexOf("|") + 1) : key.startsWith("package:") || key.length === 0 ? "" : baseName(key);
  const file = [appPath ? baseName(appPath) : "", keyFile].find((name) => name.length > 0);
  if (file) return file;
  return key.startsWith("package:") ? (key.slice("package:".length).split("_")[0] ?? "") : "";
}

/**
 * One logged line as a row about an app, or null for an event this tab does not know (or a line
 * that is not an event). Tags are the result words of the native panel's sentence.
 */
function describe(value: Fields, options: Required<Pick<LogReadOptions, "localTime" | "appName" | "text">>): Described | null {
  const LOG_TEXT = options.text;
  const event = value.event;
  if (typeof event !== "string") return null;
  const appFields = typeof value.app === "object" && value.app !== null && !Array.isArray(value.app) ? (value.app as Fields) : value;
  const key = str(appFields.key);
  const appPath = str(appFields.path);
  let app = "";
  let appKey: string | null = null;
  if (key.length > 0 || appPath.length > 0) {
    app = options.appName(key, appPath.length > 0 ? appPath : null);
    appKey = key.length > 0 ? key : null;
  } else if (value.subject === "clipboard") {
    app = LOG_TEXT.clipboard;
    appKey = CLIPBOARD_KEY;
  }
  const shown = app.length > 0 ? app : LOG_TEXT.noApp;
  const time = typeof value.time === "string" ? options.localTime(value.time) : LOG_TEXT.noTime;
  const tier = LOG_TEXT.tiers[str(value.tier)] ?? str(value.tier);
  const removed = value.grant_removed === true ? LOG_TEXT.grantRemoved : "";
  const row = (sentence: string, tag?: string, delivery?: "background" | "foreground"): Described => ({
    key: appKey,
    app: app.length > 0 ? app : LOG_TEXT.noApp,
    row: { time, sentence, ...(tag ? { tag } : {}), ...(delivery ? { delivery, mark: LOG_TEXT.delivery[delivery] } : {}) },
  });

  // What the settings page (or the native panel) wrote is what the user did there.
  const session = value.session === "settings" || value.session === "panel" ? value.session : null;
  if (session !== null) {
    const done = LOG_TEXT.done;
    const where = done[session].where;
    if (event === "tier_raised") return row(fill(done.tierRaised, { where, app: shown, tier, removed }));
    if (event === "tier_raise_removed") return row(fill(done.tierRaiseRemoved, { where, app: shown }));
    if (event === "grant_revoked") return row(fill(done.grantRevoked, { where, app: shown }));
  }

  const channel = typeof value.channel === "string" ? value.channel : null;
  switch (event) {
    case "tool_call": {
      const tool = str(value.tool);
      const result = str(value.result);
      const what = tool.length === 0 ? LOG_TEXT.toolUnnamed : (LOG_TEXT.tools[tool] ?? tool);
      const summary = str(value.action_summary);
      const sentence = summary.trim().length > 0 ? `${what} · ${LOG_TEXT.summary}${summary}` : what;
      const delivery = value.delivery === "background" || value.delivery === "foreground" ? value.delivery : undefined;
      return row(sentence, LOG_TEXT.results[result] ?? result, delivery);
    }
    case "approval": {
      const answer = str(value.answer);
      const text = LOG_TEXT.approval;
      const what =
        value.subject === "system_key"
          ? fill(channel === "dialog" ? text.systemKeyAsked : text.systemKey, { combination: str(value.combination) })
          : channel === "dialog"
            ? text.dialog
            : channel === "elicitation"
              ? text.elicitation
              : text.other;
      const tag =
        channel === "grant" && answer === "allow_always"
          ? text.grantAllowAlways
          : channel === "session" && answer === "allow_once"
            ? text.sessionAllowOnce
            : (text.answers[answer] ?? answer);
      return row(what, tag);
    }
    case "grant_invalid": {
      const reason = str(value.reason);
      return row(LOG_TEXT.grantInvalid.what[str(value.entry)] ?? LOG_TEXT.grantInvalid.what.file!, LOG_TEXT.grantInvalid.reasons[reason] ?? reason);
    }
    case "grant_key_lost": {
      const lost = typeof value.entries === "number" ? value.entries : 0;
      return row(LOG_TEXT.keyLost.what, fill(lost === 1 ? LOG_TEXT.keyLost.resultOne : LOG_TEXT.keyLost.result, { n: lost }));
    }
    case "grant_mismatch":
      return row(LOG_TEXT.mismatch.what, LOG_TEXT.mismatch.result);
    case "grant_revoked":
      return row(LOG_TEXT.grantRevoked);
    case "tier_raised":
      return row(LOG_TEXT.tierRaised.what, fill(LOG_TEXT.tierRaised.result, { tier, removed }));
    case "tier_raise_removed":
      return row(LOG_TEXT.tierRaiseRemoved.what, LOG_TEXT.tierRaiseRemoved.result);
    case "clipboard_read": {
      const chars = typeof value.chars === "number" ? value.chars : 0;
      const cut = value.truncated === true ? LOG_TEXT.clipboardRead.truncated : "";
      return row(LOG_TEXT.clipboardRead.what, fill(chars === 1 ? LOG_TEXT.clipboardRead.resultOne : LOG_TEXT.clipboardRead.result, { n: chars }) + cut);
    }
    default:
      return null;
  }
}

/** Every known event of a day file, in file order; lines that are not event objects are skipped. */
async function describedDay(dataDir: string, day: string, options: LogReadOptions): Promise<Described[]> {
  const text = options.text ?? SETTINGS_TEXT.logRows;
  const resolved = {
    text,
    localTime: options.localTime ?? ((iso: string) => localClock(iso, text.noTime)),
    appName: options.appName ?? fileName,
  };
  const names = new Map<string, string>();
  const appName = (key: string, appPath: string | null): string => {
    const id = `${key}\u0000${appPath ?? ""}`;
    let name = names.get(id);
    if (name === undefined) {
      name = resolved.appName(key, appPath);
      names.set(id, name);
    }
    return name;
  };
  const described: Described[] = [];
  for (const line of (await readCapped(path.join(dataDir, "logs", `${day}.jsonl`))).split("\n")) {
    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch {
      continue;
    }
    if (typeof value !== "object" || value === null || Array.isArray(value)) continue;
    const one = describe(value as Fields, { ...resolved, appName });
    if (one) described.push(one);
  }
  return described;
}

/**
 * One day as groups: consecutive events of one app are one group (with the app's name, count, and
 * first and last time), and the apps that appear that day for the filter; `options.app` keeps only
 * that app's groups. Unknown events and malformed lines are skipped; a day with no file is empty.
 */
export async function logDay(dataDir: string, day: string, options: LogReadOptions = {}): Promise<LogDay> {
  const all = await describedDay(dataDir, day, options);
  const apps = new Map<string, string>();
  for (const { key, app } of all) if (key !== null && !apps.has(key)) apps.set(key, app);

  // Runs of one app are found before the filter, so the filter keeps the breaks another app made.
  let run = 0;
  const runs = all.map((one, index) => {
    if (index > 0 && all[index - 1]!.key !== one.key) run += 1;
    return { ...one, run };
  });
  const shown = options.app === undefined ? runs : runs.filter(({ key }) => key === options.app);
  const truncated = shown.length > LOG_ROWS_MAX;
  const groups: LogGroup[] = [];
  let lastRun = -1;
  for (const { key, app, row, run: number } of truncated ? shown.slice(-LOG_ROWS_MAX) : shown) {
    const group = groups[groups.length - 1];
    if (group && number === lastRun) {
      group.rows.push(row);
      group.count += 1;
      group.last = row.time;
    } else {
      groups.push({ key, app, count: 1, first: row.time, last: row.time, rows: [row] });
      lastRun = number;
    }
  }
  return {
    day,
    groups,
    apps: [...apps].map(([key, name]) => ({ key, name })).sort((a, b) => (a.name.toLowerCase() < b.name.toLowerCase() ? -1 : a.name.toLowerCase() > b.name.toLowerCase() ? 1 : 0)),
    truncated,
  };
}

/** The newest of `logDays` with at least one row to show, for "See the records of <date>" when today is empty; null when there is none. */
export async function latestLogDay(dataDir: string, today: string, options: LogReadOptions = {}): Promise<string | null> {
  for (const day of await logDays(dataDir, today)) {
    if ((await describedDay(dataDir, day, options)).length > 0) return day;
  }
  return null;
}

/** The retention line ("Records are kept for N days (...) and stay only on your computer", `logRows.retention`), from the real log settings. */
export function retentionSentence(text: LogRowText = SETTINGS_TEXT.logRows): string {
  return fill(text.retention, { days: LOG_RETENTION_DAYS, mb: Math.round(LOG_MAX_BYTES / (1024 * 1024)) });
}
