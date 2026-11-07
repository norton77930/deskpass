import { spawn } from "node:child_process";
import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { readdir, stat } from "node:fs/promises";
import { createServer, request as httpRequest, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import path from "node:path";
import { CONFIRM_CHANGE_DIALOG_MS, defaultHelperPath } from "./config.ts";
import { ToolFailure } from "./errors.ts";
import { GRANTS_FILE, GrantFileVoidError, GrantStore } from "./grants.ts";
import { HelperClient, type HelperClientOptions } from "./helper-client.ts";
import { defaultLockDir } from "./lock.ts";
import { EventLog, type SettingsChangeEvent } from "./log.ts";
import { TIER_RANK, type Tier } from "./policy.ts";
import { appRows, displayName, grantsState, noStartMenu, recentLogUses, startMenu, type SettingsApp, type StartMenu } from "./settings-apps.ts";
import { createSettingsLock, readSettingsLock, removeSettingsLock, type SettingsLockRecord } from "./settings-lock.ts";
import { latestLogDay, LOG_ROWS_MAX, logDay, logDays, retentionSentence } from "./settings-log.ts";
import { APP_CSS, APP_JS, appPageHtml } from "./settings-page.ts";
import { readAllowBackgroundOrUnreadable, SETTINGS_FILE, writeAllowBackground } from "./settings-file.ts";
import { fill, settingsText } from "./settings-strings.ts";
import { systemTool } from "./system-tools.ts";
import { uiLanguage, type UiLanguage } from "./ui-language.ts";

/**
 * Ticket 112: `deskpass settings`, the settings server (spec-deskpass, settings server). A process of its
 * own, apart from the MCP server: it never takes the MCP session lock, and starts a helper of its
 * own (the same JSON-lines protocol, `HelperClient`) for the grants file's MACs and the Start Menu's
 * app names. Ticket 113: the page tightens through POST routes behind the same checks (`/api/raise`,
 * `/api/revoke`); the confirmed restore (115) is one more.
 *
 * Access (every request): `Host` must be exactly `127.0.0.1:<port>` (DNS rebinding); the browser is
 * opened at `/open?t=<one-time token>`, and the token, once used (or after `TOKEN_TTL_MS`), is gone:
 * it is exchanged for an HttpOnly, SameSite=Strict session cookie and a redirect to `/`, so the
 * address bar keeps no token. Every other route needs that cookie (ticket 113: any one of several
 * of that name, as a page on another 127.0.0.1 port can set one too); a POST also needs `Origin` to
 * be `http://127.0.0.1:<port>`. Ticket 113: a request whose `Sec-Fetch-Site` is there and is not
 * `same-origin` or `none` (the user's own navigation) is refused, whatever the route. `/launch` is the one route for the next `deskpass settings` (no browser,
 * so no cookie): a POST with the launch secret of the settings lock, answered with a new one-time
 * token. Nothing is ever answered with a cross-origin header, and a refusal says nothing of which
 * check failed.
 *
 * The page asks `/api/status` every few seconds; when no authenticated request came for `idleMs`
 * (and nothing holds the server open: `hold`, for ticket 114's confirm window), the server stops,
 * its helper with it, and removes its lock. Ticket 113: not while a one-time token it gave out is
 * still unused and valid, so a slow browser still finds it (the first wait is `TOKEN_TTL_MS` at least).
 *
 * Ticket 113, the changes (spec-deskpass, the settings page's API (tightening limits)): `/api/raise` `{ key, tier }`
 * sets an app of the list to `high` or `blocked` and `/api/revoke` `{ key }` removes its always-allow
 * grant, each a JSON body of exactly those fields. A raise is taken only when the tier is above the
 * app's built-in tier (`built_in` otherwise) and above its tier now (`not_stricter`); it revokes the
 * app's grant in the same write, and the answer says so (`grantRemoved`). No route adds a grant or
 * lowers a tier. The app is looked up by its key in the list the page shows (`unknown_app` when it
 * is not there), which gives its executable and built-in tier. Ticket 133: that list is only the apps
 * the AI used (grants.json, the recent log), so an app only the Start Menu lists is unknown too. Each
 * write is logged (`tier_raised`, `grant_revoked`, session `settings`).
 *
 * Ticket 115, the one loosening (spec-deskpass, the settings page's API (loosening limits)): `/api/restore` `{ key }`
 * removes the user's raise of an app of the list (`no_raise` when it has none, `unknown_app`), but
 * never by itself: it writes nothing, asks the native confirm window (the helper's `confirm_change`)
 * and answers 202 with the confirm's `id` at once. `GET /api/restore?id=` is then `pending` until
 * the window is answered, and `allowed`, `cancelled` ("Cancel", closing it, or a stop), `timeout`,
 * `changed` (the raise is no longer the one shown, so nothing was removed) or `failed`. Only the
 * answer `allow` writes (`GrantStore.removeRaise`, which checks inside the write that the raise is
 * still the tier the user confirmed), logged `tier_raise_removed`. One confirm at a time: any
 * `/api/restore` while one is pending is refused (`confirm_busy`), and the server holds itself
 * open meanwhile. The window waits `CONFIRM_CHANGE_DIALOG_MS` (60 s, as the approval dialog).
 *
 * The confirm runs on a helper process of its own, started for it and ended after it: a helper
 * answers one request at a time and refuses every other one (`prompt_open`) while its window is
 * open, so on the server's helper the grants file's MACs (the page's poll, its list, a tightening)
 * would fail for up to the window's whole minute. What the window is sent (the app's name and the
 * two state texts) is flattened first: no control, format or line-separator character (they could
 * push its "If you did not press this on the settings page" line out of sight), and capped in length.
 */

/**
 * Ticket 132: 3 minutes. Chrome and Edge slow a background tab's timers to about once a minute, so
 * the page's 5 s poll can come that rarely while the tab is open; a shown tab polls at once.
 */
export const SETTINGS_IDLE_MS = 180_000;
export const SETTINGS_POLL_MS = 5_000;
/** Ticket 116: the shortest wait before a failed Start Menu listing is asked again. */
export const MENU_RETRY_MS = 30_000;
/** How long a one-time token waits to be used: long enough for a browser to start. */
export const TOKEN_TTL_MS = 120_000;
/** The most one-time tokens waiting at once; a new one past this drops the oldest. */
const MAX_TOKENS = 16;
const COOKIE = "deskpass_session";
const LAUNCH_HEADER = "x-deskpass-launch";
const LAUNCH_TIMEOUT_MS = 3_000;
const LAUNCH_ATTEMPTS = 3;
const TOKEN_PATTERN = /^[A-Za-z0-9_-]{43}$/;
/** The largest request body read; a change's is a few hundred bytes. */
const BODY_MAX_BYTES = 16 * 1024;
/** The session every change the page makes is logged under. */
export const SETTINGS_LOG_SESSION = "settings";
/** Ticket 115: the most characters (code points) of the app name the confirm window is sent. */
const CONFIRM_NAME_MAX = 80;
/** Ticket 115: the most characters of each state text the confirm window is sent. */
const CONFIRM_STATE_MAX = 40;
/** Ticket 124: how many replaced confirm ids are still answered `superseded`. */
const SUPERSEDED_MAX = 16;

/**
 * Ticket 115: `text` as the native confirm window may show it: a lone surrogate becomes U+FFFD, a
 * format character (a bidi override, a zero-width one) is dropped, a control character (a newline,
 * a tab) or a line or paragraph separator becomes a space, white space runs become one space, and
 * the rest is cut to `max` code points, the last one an ellipsis. The window quotes the name in
 * corner brackets (U+300C and U+300D), so a corner bracket in it becomes a plain one: a name cannot close the quote and pose as
 * another app's sentence.
 */
function confirmText(text: string, max: number): string {
  const flat = text
    .replace(/\p{Cs}/gu, "�")
    .replace(/\p{Cf}/gu, "")
    .replace(/[「『﹁﹃]/gu, "(")
    .replace(/[」』﹂﹄]/gu, ")")
    .replace(/[\p{Cc}\p{Zl}\p{Zp}]/gu, " ")
    .replace(/\s+/gu, " ")
    .trim();
  const points = [...flat];
  return points.length <= max ? flat : `${points.slice(0, max - 1).join("").trimEnd()}…`;
}

/** Ticket 115: where a confirm is: `pending` while its window is open, then what came of it. */
type ConfirmState = "pending" | "allowed" | "cancelled" | "timeout" | "changed" | "failed";

interface Confirm {
  id: string;
  key: string;
  state: ConfirmState;
  /** What the page shows for it. */
  message: string;
}

const SECURITY_HEADERS: Record<string, string> = {
  "cache-control": "no-store",
  "x-content-type-options": "nosniff",
  "x-frame-options": "DENY",
  "referrer-policy": "no-referrer",
  "content-security-policy": "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
};

export interface SettingsOptions {
  /** The data folder (settings.lock, grants.json, grants.key, logs); `%LOCALAPPDATA%\deskpass` by default. */
  dataDir?: string;
  helperPath?: string;
  /** Arguments before the server's own (`--grants-dir`, `--server-pid`); for tests (the fake helper). */
  helperArgs?: string[];
  /** How the helper process is started; for tests. */
  spawn?: HelperClientOptions["spawn"];
  /** Opens `url` in the user's default browser; tests capture it instead. */
  openBrowser?: (url: string) => void | Promise<void>;
  /** How long the server waits for an authenticated request before it stops. */
  idleMs?: number;
  /** How often the page asks for the server's state. */
  pollMs?: number;
  /** Ticket 116: how long after a failed Start Menu listing a page request asks again. */
  menuRetryMs?: number;
  log?: (line: string) => void;
  /** Open-source ticket 01: the page's language; decided by `uiLanguage` (DESKPASS_LANG, then Windows) when not given. */
  lang?: UiLanguage;
}

export interface SettingsServer {
  port: number;
  /** `http://127.0.0.1:<port>`. */
  origin: string;
  /** Settles once the server has stopped, its helper ended and its lock removed. */
  closed: Promise<void>;
  close(): Promise<void>;
  /** Keeps the server from stopping when idle until the returned function is called (ticket 114: a confirm window is open). */
  hold(): () => void;
}

export type LaunchResult = { started: true; server: SettingsServer } | { started: false; port: number };

/**
 * `deskpass settings`: asks a running settings server (named by the lock) for a new tab, or, when
 * there is none, starts one, takes the lock and opens the browser. A lock whose server does not
 * answer (gone, or another program on its port) is removed first.
 */
export async function launchSettings(options: SettingsOptions = {}): Promise<LaunchResult> {
  const dataDir = path.resolve(options.dataDir ?? defaultLockDir());
  const log = options.log ?? ((line: string) => console.error(line));
  // Open-source ticket 01: the language is decided once, here, while the lock is read.
  const language = options.lang !== undefined ? Promise.resolve(options.lang) : uiLanguage();
  // A browser that cannot be started leaves the server running: the address is said instead.
  const openBrowser = async (url: string) => {
    try {
      await (options.openBrowser ?? openInDefaultBrowser)(url);
    } catch (error) {
      log(`deskpass: the browser could not be opened (${error instanceof Error ? error.message : String(error)}); open ${url} yourself within ${TOKEN_TTL_MS / 60_000} minutes.`);
    }
  };
  let mine: Running | null = null;
  try {
    for (let attempt = 0; attempt < LAUNCH_ATTEMPTS; attempt += 1) {
      const existing = await readSettingsLock(dataDir);
      if (existing) {
        const token = existing.record ? await askForToken(existing.record) : null;
        if (existing.record && token) {
          if (mine) await mine.server.close();
          mine = null;
          await openBrowser(openUrl(existing.record.port, token));
          return { started: false, port: existing.record.port };
        }
        await removeSettingsLock(dataDir, existing.raw);
      }
      mine ??= await startServer(dataDir, options, await language);
      if (await mine.takeLock()) {
        const running = mine;
        mine = null;
        await openBrowser(openUrl(running.server.port, running.mintToken()));
        return { started: true, server: running.server };
      }
    }
    throw new Error(`the settings lock in ${dataDir} kept changing; try again`);
  } finally {
    if (mine) await mine.server.close();
  }
}

/** `deskpass settings` as a command: 0 once the server stopped (or another one opened the tab), 1 when it could not start. */
export async function runSettings(options: SettingsOptions = {}): Promise<number> {
  const log = options.log ?? ((line: string) => console.error(line));
  let result: LaunchResult;
  try {
    result = await launchSettings(options);
  } catch (error) {
    log(`deskpass: the settings page could not be opened (${error instanceof Error ? error.message : String(error)}).`);
    return 1;
  }
  if (!result.started) {
    log(`deskpass: the settings page is already running; a new tab was opened (http://127.0.0.1:${result.port}/).`);
    return 0;
  }
  const { server } = result;
  log(`deskpass: the settings page is running at ${server.origin}/ and stops when no tab has used it for a while. Press Ctrl+C to stop it now.`);
  const stop = () => void server.close();
  process.once("SIGINT", stop);
  await server.closed;
  process.off("SIGINT", stop);
  return 0;
}

function openUrl(port: number, token: string): string {
  return `http://127.0.0.1:${port}/open?t=${token}`;
}

/** Opens `url` with the user's default browser (Windows' URL handler: no shell parses it). */
function openInDefaultBrowser(url: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(systemTool("rundll32.exe"), ["url.dll,FileProtocolHandler", url], { detached: true, stdio: "ignore", windowsHide: true });
    child.once("error", reject);
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  });
}

/** Asks the server of `record` for a one-time token with its launch secret; null when it does not give one. */
function askForToken(record: SettingsLockRecord): Promise<string | null> {
  return new Promise((resolve) => {
    const req = httpRequest(
      { host: "127.0.0.1", port: record.port, path: "/launch", method: "POST", headers: { host: `127.0.0.1:${record.port}`, [LAUNCH_HEADER]: record.secret, "content-length": "0" }, timeout: LAUNCH_TIMEOUT_MS },
      (res) => {
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (chunk: string) => {
          if (body.length < 1024) body += chunk;
        });
        res.on("end", () => {
          if (res.statusCode !== 200) return resolve(null);
          try {
            const token = (JSON.parse(body) as { token?: unknown }).token;
            resolve(typeof token === "string" && TOKEN_PATTERN.test(token) ? token : null);
          } catch {
            resolve(null);
          }
        });
        res.on("error", () => resolve(null));
      },
    );
    req.on("timeout", () => req.destroy());
    req.on("error", () => resolve(null));
    req.end();
  });
}

function digest(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** Compares two secrets in time that does not depend on where they differ. */
function sameSecret(given: string, expected: string): boolean {
  const a = createHash("sha256").update(given, "utf8").digest();
  const b = createHash("sha256").update(expected, "utf8").digest();
  return timingSafeEqual(a, b) && given.length === expected.length;
}

function randomToken(): string {
  return randomBytes(32).toString("base64url");
}

/** Every value of the session cookie in a `Cookie` header: a page on another port of 127.0.0.1 can add one of the same name. */
function cookieValues(header: string | undefined): string[] {
  if (!header) return [];
  const values: string[] = [];
  for (const part of header.split(";")) {
    const index = part.indexOf("=");
    if (index < 0) continue;
    if (part.slice(0, index).trim() === COOKIE) values.push(part.slice(index + 1).trim());
  }
  return values;
}

/** Ticket 146: the Allow background operation request: a JSON object (sent as `application/json`) of exactly `allow_background`, a boolean; null otherwise. */
function backgroundRequest(contentType: string | undefined, body: string | null): boolean | null {
  if ((contentType ?? "").split(";")[0]?.trim().toLowerCase() !== "application/json" || body === null) return null;
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const entry = value as Record<string, unknown>;
  const names = Object.keys(entry);
  return names.length === 1 && names[0] === "allow_background" && typeof entry.allow_background === "boolean" ? entry.allow_background : null;
}

/** The request's body, or null when it is larger than `BODY_MAX_BYTES` or breaks off. */
function readBody(req: IncomingMessage): Promise<string | null> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size <= BODY_MAX_BYTES) chunks.push(chunk);
    });
    req.on("end", () => resolve(size > BODY_MAX_BYTES ? null : Buffer.concat(chunks).toString("utf8")));
    req.on("error", () => resolve(null));
  });
}

/** A change's request: a JSON object (sent as `application/json`) of exactly `fields`, each a string that is not empty; null otherwise. */
function changeRequest(contentType: string | undefined, body: string | null, fields: readonly string[]): Record<string, string> | null {
  if ((contentType ?? "").split(";")[0]?.trim().toLowerCase() !== "application/json" || body === null) return null;
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const entry = value as Record<string, unknown>;
  const names = Object.keys(entry);
  if (names.length !== fields.length || !fields.every((name) => names.includes(name))) return null;
  if (!fields.every((name) => typeof entry[name] === "string" && (entry[name] as string).length > 0)) return null;
  return entry as Record<string, string>;
}

interface Running {
  server: SettingsServer;
  /** Takes the settings lock for this server; false when another server holds it. */
  takeLock(): Promise<boolean>;
  mintToken(): string;
}

async function startServer(dataDir: string, options: SettingsOptions, lang: UiLanguage): Promise<Running> {
  const T = settingsText(lang);
  const idleMs = options.idleMs !== undefined && options.idleMs > 0 ? options.idleMs : SETTINGS_IDLE_MS;
  const pollMs = options.pollMs !== undefined && options.pollMs > 0 ? options.pollMs : SETTINGS_POLL_MS;
  const secret = randomToken();
  const tokens = new Map<string, number>();
  const sessions = new Set<string>();
  let lastActivity = Date.now();
  let holds = 0;
  let lockRaw: string | null = null;
  /** Keeps the server from stopping when idle until the returned function is called. */
  const hold = (): (() => void) => {
    holds += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      holds -= 1;
      lastActivity = Date.now();
    };
  };

  const menuRetryMs = options.menuRetryMs !== undefined && options.menuRetryMs >= 0 ? options.menuRetryMs : MENU_RETRY_MS;
  const helperOptions: HelperClientOptions = {
    helperPath: options.helperPath ?? defaultHelperPath(),
    // As the MCP server does: the helper never resolves the folder itself, so its key is the one beside this grants.json.
    helperArgs: [...(options.helperArgs ?? []), "--grants-dir", dataDir],
    lang,
    serverPid: process.pid,
    spawn: options.spawn,
    log: options.log,
  };
  const helper = new HelperClient(helperOptions);
  const grantMac = (params: Record<string, unknown>) => helper.request("grant_mac", params);
  // What it finds wrong in the file is shown on the page, not logged: that log is the MCP server's.
  const grants = new GrantStore({ dir: dataDir, record: async () => undefined });
  const logDir = path.join(dataDir, "logs");
  // Ticket 113: the page's changes, logged as session `settings`; the log is opened at the first
  // change, so a server that only shows the page never touches the log folder.
  let changeLog: EventLog | null = null;
  const recordChange = (event: SettingsChangeEvent) => (changeLog ??= new EventLog({ dir: logDir, session: SETTINGS_LOG_SESSION })).record(event);
  // The Start Menu's apps and names (ticket 116: `list_start_menu_apps`, with their identity keys),
  // read in the background: listing them takes a helper seconds, so the first list is shown without
  // them, and the page's next poll (the revision changes) shows them. They are asked of a helper of
  // their own, started for the listing and ended after it: a helper answers one request at a time,
  // and the grants file's MACs (and a confirm window) must not wait behind the listing. A listing
  // that failed is asked again by a page request at least `menuRetryMs` later.
  let menu: StartMenu | null = null;
  let lister: HelperClient | null = null;
  let menuFailedAt: number | null = null;
  let stopping = false;
  const startMenuNow = (): StartMenu => {
    const due = menu === null ? menuFailedAt === null : menu.state === "failed" && menuFailedAt !== null && Date.now() - menuFailedAt >= menuRetryMs;
    if (due && lister === null && !stopping) {
      const asked = new HelperClient(helperOptions);
      lister = asked;
      void asked
        .request("list_start_menu_apps", {})
        .then(startMenu, () => noStartMenu("failed"))
        .then(async (listed) => {
          menuFailedAt = listed.state === "failed" ? Date.now() : null;
          menu = listed;
          await asked.dispose();
          lister = null;
        });
    }
    return menu ?? noStartMenu();
  };

  const mintToken = () => {
    const now = Date.now();
    for (const [hash, expires] of tokens) if (expires <= now) tokens.delete(hash);
    while (tokens.size >= MAX_TOKENS) tokens.delete(tokens.keys().next().value as string);
    const token = randomToken();
    tokens.set(digest(token), now + TOKEN_TTL_MS);
    return token;
  };
  /** Whether a one-time token given out is still waiting to be used. */
  const tokenWaiting = (): boolean => {
    const now = Date.now();
    for (const expires of tokens.values()) if (expires > now) return true;
    return false;
  };
  const takeToken = (token: string): boolean => {
    const hash = digest(token);
    const expires = tokens.get(hash);
    tokens.delete(hash);
    return expires !== undefined && expires > Date.now();
  };

  const http: Server = createServer();
  await new Promise<void>((resolve, reject) => {
    http.once("error", reject);
    http.listen(0, "127.0.0.1", () => {
      http.off("error", reject);
      resolve();
    });
  });
  const address = http.address();
  if (address === null || typeof address === "string") throw new Error("the settings server has no port");
  const port = address.port;
  const host = `127.0.0.1:${port}`;
  const origin = `http://${host}`;

  /** What the page's poll compares: changes when grants.json or the newest log file does. */
  const revision = async (): Promise<string> => {
    const version = async (file: string) => {
      try {
        const info = await stat(file);
        return `${info.mtimeMs}:${info.size}`;
      } catch {
        return "-";
      }
    };
    let newest = "";
    try {
      newest = (await readdir(logDir)).filter((name) => /^\d{4}-\d{2}-\d{2}\.jsonl$/.test(name)).sort().pop() ?? "";
    } catch {
      // No log folder yet.
    }
    const verdict = (await grants.current(grantMac)).verdict();
    // While the helper cannot verify the file, every poll reloads, so the page shows it once it can.
    const retry = verdict.state === "unverified" ? `:${Date.now()}` : "";
    // The page's poll is what asks a failed listing again.
    const named = startMenuNow().state;
    return digest(`${await version(path.join(dataDir, GRANTS_FILE))}|${await version(path.join(dataDir, SETTINGS_FILE))}|${newest}|${newest ? await version(path.join(logDir, newest)) : "-"}|${verdict.state}${retry}|${named}`).slice(0, 16);
  };

  const apps = async () => {
    const [view, logged] = await Promise.all([grants.current(grantMac), recentLogUses(logDir)]);
    const listedMenu = startMenuNow();
    return { grants: grantsState(view.verdict(), T), menu: listedMenu.state, apps: await appRows(view, logged, listedMenu, T) };
  };

  /**
   * Ticket 117: an app's name in the log is the App page's (`displayName`: the Start Menu's name, else
   * its grant's, else its file name), so one id reads the same on every page.
   */
  const logOptions = async () => {
    const { grants: granted } = (await grants.current(grantMac)).entries();
    const { names } = startMenuNow();
    return { text: T.logRows, appName: (key: string, appPath: string | null) => displayName(names, key, appPath, granted.find((grant) => grant.key === key)?.display_name ?? null) };
  };
  /** The log files are named by the local date, as `EventLog` names them. */
  const today = (): string => {
    const now = new Date();
    return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  };
  /** `/api/log/days`: the days to pick from, and the newest of them with rows (for "See the records of <date>"). */
  const logDayList = async () => {
    const day = today();
    return { today: day, days: await logDays(dataDir, day), latest: await latestLogDay(dataDir, day, await logOptions()), retention: retentionSentence(T.logRows) };
  };
  /**
   * `/api/log?day=YYYY-MM-DD[&app=key]`: a day as groups. The day must be written exactly so (a real
   * date; 400) and be one of the days on offer (404); the app one that appears that day (404).
   */
  const logOfDay = async (res: ServerResponse, params: URLSearchParams) => {
    const days = params.getAll("day");
    const apps = params.getAll("app");
    const given = days[0] ?? "";
    const real = /^\d{4}-\d{2}-\d{2}$/.test(given) && Number.isFinite(Date.parse(`${given}T00:00:00Z`)) && new Date(`${given}T00:00:00Z`).toISOString().slice(0, 10) === given;
    if (days.length !== 1 || !real || apps.length > 1 || (apps.length === 1 && apps[0] === "")) return badRequest(res);
    if (!(await logDays(dataDir, today())).includes(given)) return send(res, 404, "text/plain; charset=utf-8", T.notFound);
    const app = apps[0];
    const result = await logDay(dataDir, given, { ...(await logOptions()), ...(app === undefined ? {} : { app }) });
    if (app !== undefined && !result.apps.some((entry) => entry.key === app)) return send(res, 404, "text/plain; charset=utf-8", T.notFound);
    return json(res, { ...result, rowsMax: LOG_ROWS_MAX });
  };

  const send = (res: ServerResponse, status: number, type: string, body: string, extra: Record<string, string> = {}) => {
    res.writeHead(status, { ...SECURITY_HEADERS, "content-type": type, "content-length": Buffer.byteLength(body), ...extra });
    res.end(body);
  };
  const refuse = (res: ServerResponse) => send(res, 403, "text/plain; charset=utf-8", T.forbidden);
  const json = (res: ServerResponse, value: unknown, status = 200) => send(res, status, "application/json; charset=utf-8", JSON.stringify(value));
  const tierName = T.page.tiers;
  const text = T.change;
  const refuseChange = (res: ServerResponse, reason: string, message: string) => json(res, { ok: false, reason, message }, 409);
  const badRequest = (res: ServerResponse) => json(res, { ok: false, reason: "bad_request", message: text.badRequest }, 400);
  /** A write that did not happen: over a void file (refused, `GrantFileVoidError`), or one the helper could not sign or the disk took not. */
  const notWritten = (res: ServerResponse, error: unknown) =>
    error instanceof GrantFileVoidError ? refuseChange(res, "grants_void", text.grantsVoid) : json(res, { ok: false, reason: "unavailable", message: text.unavailable }, 503);
  /** The app of `key` in the list the page shows, or null. */
  const listed = async (key: string): Promise<SettingsApp | null> => (await apps()).apps.find((app) => app.key === key) ?? null;
  const pathOf = (app: SettingsApp) => (app.path ? { path: app.path } : {});

  /** `/api/raise`: only above the app's built-in tier and its tier now; its grant is revoked with it. */
  const raise = async (res: ServerResponse, request: Record<string, string>) => {
    const tier = request.tier;
    if (tier !== "high" && tier !== "blocked") return badRequest(res);
    const app = await listed(request.key as string);
    if (!app) return refuseChange(res, "unknown_app", text.unknownApp);
    const builtIn: Tier = app.builtIn?.tier ?? "normal";
    if (TIER_RANK[tier] <= TIER_RANK[builtIn]) return refuseChange(res, "built_in", fill(text.builtIn, { name: app.name, tier: tierName[builtIn] }));
    let outcome;
    try {
      outcome = await grants.raise(grantMac, app.key, tier, builtIn);
    } catch (error) {
      return notWritten(res, error);
    }
    if (!outcome.raised) return refuseChange(res, "not_stricter", fill(text.notStricter, { name: app.name, tier: tierName[outcome.tier] }));
    await recordChange({ event: "tier_raised", key: app.key, ...pathOf(app), tier, grant_removed: outcome.grantRemoved });
    const message = fill(text.raised, { name: app.name, tier: tierName[tier] }) + (outcome.grantRemoved ? text.grantRemoved : "");
    return json(res, { ok: true, grantRemoved: outcome.grantRemoved, message });
  };

  /** `/api/revoke`: removes the app's always-allow grant. */
  const revoke = async (res: ServerResponse, request: Record<string, string>) => {
    const app = await listed(request.key as string);
    if (!app) return refuseChange(res, "unknown_app", text.unknownApp);
    let revoked: boolean;
    try {
      revoked = await grants.revoke(grantMac, app.key);
    } catch (error) {
      return notWritten(res, error);
    }
    if (!revoked) return refuseChange(res, "no_grant", fill(text.noGrant, { name: app.name }));
    await recordChange({ event: "grant_revoked", key: app.key, ...pathOf(app) });
    return json(res, { ok: true, message: fill(text.revoked, { name: app.name }) });
  };

  /**
   * Ticket 146: `GET /api/settings`, the Allow background operation switch as the MCP server reads it (on when settings.json
   * is missing or lacks it). Ticket 153: a file that cannot be read is `{ allowBackground: null, unreadable: true }`,
   * since the MCP server then keeps the last value it read and this page cannot say which that is.
   */
  const settings = async () => {
    const allowBackground = await readAllowBackgroundOrUnreadable(dataDir);
    return allowBackground === null ? { allowBackground, unreadable: true } : { allowBackground };
  };
  /**
   * Ticket 146: `POST /api/settings`, the switch in settings.json. Either way is taken at once, with
   * no confirm window: on is the default, off only makes the AI work where the user can see it, and
   * neither changes who may be operated (approvals, tiers and grants are untouched).
   */
  const setBackground = async (res: ServerResponse, allowed: boolean) => {
    try {
      await writeAllowBackground(dataDir, allowed);
    } catch {
      return json(res, { ok: false, reason: "unavailable", message: text.settingsUnavailable }, 503);
    }
    return json(res, { ok: true, allowBackground: allowed, message: allowed ? text.backgroundOn : text.backgroundOff });
  };

  // Ticket 115: the newest confirm (pending while its window is open), and the helper showing it.
  const restoreText = text.restore;
  let confirm: Confirm | null = null;
  let confirmer: HelperClient | null = null;
  // Ticket 124: the ids of the confirms a newer one replaced (the newest `SUPERSEDED_MAX`), so a page
  // still asking about one is told it was replaced rather than that it is unknown.
  const superseded: string[] = [];

  /**
   * Shows the confirm window for `entry` on a helper of its own and waits for its answer; only
   * `allow` removes the raise, and only while it is still `tier`. The entry is settled whatever
   * happens: an answer that is not `allow`, a stop, an error or a server closing writes nothing.
   */
  const runConfirm = async (entry: Confirm, app: SettingsApp, tier: "high" | "blocked", params: Record<string, unknown>) => {
    const settle = (state: ConfirmState, message: string) => {
      entry.state = state;
      entry.message = message;
    };
    const values = { name: app.name, tier: tierName[tier], restored: String(params.restored_state) };
    const asked = new HelperClient(helperOptions);
    confirmer = asked;
    let answer: unknown = null;
    try {
      if (!stopping) answer = ((await asked.request("confirm_change", params)) as { answer?: unknown } | null)?.answer;
    } catch (error) {
      // A stop (Esc) while the window was open closes it: not allowed, as "Cancel" is.
      if (error instanceof ToolFailure && error.code === "interrupted") answer = "cancel";
    } finally {
      if (confirmer === asked) confirmer = null;
      await asked.dispose();
    }
    if (answer === "cancel") return settle("cancelled", fill(restoreText.cancelled, values));
    if (answer === "timeout") return settle("timeout", fill(restoreText.timeout, values));
    if (answer !== "allow" || stopping) return settle("failed", restoreText.failed);
    let removed: boolean;
    try {
      removed = await grants.removeRaise(grantMac, app.key, tier);
    } catch (error) {
      return settle("failed", error instanceof GrantFileVoidError ? text.grantsVoid : text.unavailable);
    }
    if (!removed) return settle("changed", fill(restoreText.changed, values));
    await recordChange({ event: "tier_raise_removed", key: app.key, ...pathOf(app) });
    settle("allowed", fill(restoreText.allowed, values));
  };

  /** `/api/restore`: asks the confirm window to remove the app's raise; writes nothing itself. */
  const restore = async (res: ServerResponse, request: Record<string, string>) => {
    const busy = () => confirm !== null && confirm.state === "pending";
    if (busy()) return refuseChange(res, "confirm_busy", restoreText.busy);
    const { grants: state, apps: rows } = await apps();
    const app = rows.find((row) => row.key === request.key);
    if (!app) return refuseChange(res, "unknown_app", text.unknownApp);
    // The write is made only over a file that verified: no window is shown for one that would not be written.
    if (state.state === "void") return refuseChange(res, "grants_void", text.grantsVoid);
    if (state.state !== "valid") return json(res, { ok: false, reason: "unavailable", message: text.unavailable }, 503);
    // Ticket 125: the user's raise, in force or not (one not above the built-in tier raises nothing, and is cleared the same way).
    const tier = app.raised;
    if (tier === null) return refuseChange(res, "no_raise", fill(restoreText.noRaise, { name: app.name }));
    // Checked again after the reads above: two requests at once must not open two windows.
    if (busy() || stopping) return refuseChange(res, "confirm_busy", restoreText.busy);
    const builtIn: Tier = app.builtIn?.tier ?? "normal";
    const restored = builtIn === "normal" ? restoreText.restoredNormal : fill(restoreText.restoredBuiltIn, { tier: tierName[builtIn] });
    const params = {
      app_name: confirmText(app.name, CONFIRM_NAME_MAX) || restoreText.unnamed,
      current_state: confirmText(fill(restoreText.current, { tier: tierName[tier] }), CONFIRM_STATE_MAX),
      restored_state: confirmText(restored, CONFIRM_STATE_MAX),
      timeout_ms: CONFIRM_CHANGE_DIALOG_MS,
    };
    const entry: Confirm = { id: randomToken(), key: app.key, state: "pending", message: restoreText.pending };
    if (confirm !== null) {
      superseded.push(confirm.id);
      if (superseded.length > SUPERSEDED_MAX) superseded.shift();
    }
    confirm = entry;
    const release = hold();
    void runConfirm(entry, app, tier, params)
      .catch(() => {
        if (entry.state === "pending") {
          entry.state = "failed";
          entry.message = restoreText.failed;
        }
      })
      .finally(release);
    return json(res, { ok: true, pending: true, id: entry.id, message: restoreText.pending }, 202);
  };

  /**
   * `GET /api/restore?id=`: where the confirm of `id` is (only the newest one is known). Ticket 124:
   * one a newer confirm replaced is `superseded` (what came of it is no longer kept: the page reloads
   * the App list); an id this server never gave is 404.
   */
  const confirmStatus = (res: ServerResponse, params: URLSearchParams) => {
    const ids = params.getAll("id");
    if (ids.length === 1 && superseded.includes(ids[0]!)) return json(res, { id: ids[0], state: "superseded", message: restoreText.superseded });
    if (ids.length !== 1 || confirm === null || confirm.id !== ids[0]) return send(res, 404, "text/plain; charset=utf-8", T.notFound);
    return json(res, { id: confirm.id, key: confirm.key, state: confirm.state, message: confirm.message });
  };

  /**
   * Ticket 126: the confirm whose window is open, as `/api/apps` sends it (null when none is): a page
   * reloaded meanwhile (or another tab) waits for that one instead of asking for a second window.
   */
  const openConfirm = () => (confirm !== null && confirm.state === "pending" ? { id: confirm.id, key: confirm.key, state: confirm.state, message: confirm.message } : null);

  const handle = async (req: IncomingMessage, res: ServerResponse) => {
    const method = req.method ?? "";
    // Only a change (a POST) has a body: read as it comes, looked at only by a route past the checks.
    const body = method === "POST" ? readBody(req) : null;
    if (body === null) req.resume();
    if (req.headers.host !== host || !req.url?.startsWith("/")) return refuse(res);
    // A browser says where a request came from: only this page itself, or the user's own navigation.
    const site = req.headers["sec-fetch-site"];
    if (site !== undefined && site !== "same-origin" && site !== "none") return refuse(res);
    const url = new URL(req.url, origin);

    if (url.pathname === "/launch") {
      const given = req.headers[LAUNCH_HEADER];
      const fromBrowser = req.headers.origin !== undefined && req.headers.origin !== origin;
      if (method !== "POST" || typeof given !== "string" || fromBrowser || !sameSecret(given, secret)) return refuse(res);
      lastActivity = Date.now();
      return json(res, { token: mintToken() });
    }
    if (url.pathname === "/open") {
      const token = url.searchParams.get("t");
      if (method !== "GET" || token === null || !takeToken(token)) return refuse(res);
      const session = randomToken();
      sessions.add(digest(session));
      lastActivity = Date.now();
      return send(res, 303, "text/plain; charset=utf-8", "", { location: "/", "set-cookie": `${COOKIE}=${session}; HttpOnly; SameSite=Strict; Path=/` });
    }

    if (!cookieValues(req.headers.cookie).some((session) => sessions.has(digest(session)))) return refuse(res);
    if (method === "POST") {
      if (req.headers.origin !== origin) return refuse(res);
    } else if (method !== "GET") {
      return refuse(res);
    }
    lastActivity = Date.now();

    if (method === "GET") {
      switch (url.pathname) {
        case "/":
          return send(res, 200, "text/html; charset=utf-8", appPageHtml(pollMs, T));
        case "/app.css":
          return send(res, 200, "text/css; charset=utf-8", APP_CSS);
        case "/app.js":
          return send(res, 200, "text/javascript; charset=utf-8", APP_JS);
        case "/api/status":
          return json(res, { revision: await revision() });
        case "/api/apps":
          return json(res, { ...(await apps()), confirm: openConfirm() });
        case "/api/log/days":
          return json(res, await logDayList());
        case "/api/log":
          return logOfDay(res, url.searchParams);
        case "/api/restore":
          return confirmStatus(res, url.searchParams);
        case "/api/settings":
          return json(res, await settings());
      }
    }
    if (method === "POST" && body !== null) {
      switch (url.pathname) {
        case "/api/settings": {
          const allowed = backgroundRequest(req.headers["content-type"], await body);
          return allowed === null ? badRequest(res) : setBackground(res, allowed);
        }
        case "/api/restore": {
          const request = changeRequest(req.headers["content-type"], await body, ["key"]);
          return request ? restore(res, request) : badRequest(res);
        }
        case "/api/raise": {
          const request = changeRequest(req.headers["content-type"], await body, ["key", "tier"]);
          return request ? raise(res, request) : badRequest(res);
        }
        case "/api/revoke": {
          const request = changeRequest(req.headers["content-type"], await body, ["key"]);
          return request ? revoke(res, request) : badRequest(res);
        }
      }
    }
    return send(res, 404, "text/plain; charset=utf-8", T.notFound);
  };
  http.on("request", (req: IncomingMessage, res: ServerResponse) => {
    handle(req, res).catch(() => {
      if (!res.headersSent) send(res, 500, "text/plain; charset=utf-8", "");
      else res.destroy();
    });
  });

  let resolveClosed: () => void = () => {};
  const closed = new Promise<void>((resolve) => {
    resolveClosed = resolve;
  });
  let closing: Promise<void> | null = null;
  const close = () =>
    (closing ??= (async () => {
      clearInterval(idleTimer);
      await new Promise<void>((resolve) => {
        http.close(() => resolve());
        http.closeAllConnections();
      });
      stopping = true;
      // A confirm window still open closes with its helper: not allowed, so nothing is written.
      await Promise.all([helper.dispose(), lister?.dispose(), confirmer?.dispose()]);
      if (lockRaw !== null) await removeSettingsLock(dataDir, lockRaw);
      resolveClosed();
    })());
  const idleTimer = setInterval(() => {
    if (holds === 0 && Date.now() - lastActivity >= idleMs && !tokenWaiting()) void close();
  }, Math.max(10, Math.min(1_000, Math.floor(idleMs / 4))));
  idleTimer.unref();

  const server: SettingsServer = {
    port,
    origin,
    closed,
    close,
    hold,
  };
  return {
    server,
    async takeLock() {
      const record: SettingsLockRecord = { pid: process.pid, port, secret, started_at: new Date().toISOString() };
      if (!(await createSettingsLock(dataDir, record))) return false;
      lockRaw = JSON.stringify(record);
      lastActivity = Date.now();
      return true;
    },
    mintToken,
  };
}
