import { open, readdir } from "node:fs/promises";
import path from "node:path";
import { LOG_MAX_BYTES } from "./config.ts";
import type { GrantVerdict, GrantView } from "./grants.ts";
import { builtInTierOf, packageFamilyFromPath, RAISED_WHY, TIER_RANK, type Tier } from "./policy.ts";
import { fill, SETTINGS_TEXT, type SettingsText } from "./settings-strings.ts";

/**
 * Ticket 112: the settings page's App list, "Used by the AI" (spec-deskpass, contents of the App table): the apps of
 * grants.json (its grants, then its raises) and of the newest seven day files of the log, each
 * once, by identity key, as the native control panel listed them (the retired helper panel.rs,
 * `candidates`, `logged_apps`). Ticket 116: the Start Menu (helper `list_start_menu_apps`,
 * `startMenu`) gives those apps their names, by the same identity key. Ticket 133: it adds no rows;
 * an app the AI never used is not listed.
 */

/** How many of the newest log day files name the apps the AI operated (the control panel's rule). */
export const RECENT_LOG_DAYS = 7;

const DAY_FILE = /^\d{4}-\d{2}-\d{2}\.jsonl$/;

/** One app a log event names: its identity key, its executable, and when (ms). */
export interface LoggedUse {
  key: string;
  path: string;
  time: number;
}

/**
 * The apps the newest `RECENT_LOG_DAYS` day files in `logDir` name (any event with an `app` that
 * has a `key` and a `path`), in file order. Lines that are not JSON objects are skipped; a file is
 * read up to the log's size limit.
 */
export async function recentLogUses(logDir: string): Promise<LoggedUse[]> {
  let names: string[];
  try {
    names = await readdir(logDir);
  } catch {
    return [];
  }
  const days = names.filter((name) => DAY_FILE.test(name)).sort().reverse().slice(0, RECENT_LOG_DAYS);
  const uses: LoggedUse[] = [];
  for (const day of days) {
    for (const line of (await readCapped(path.join(logDir, day))).split("\n")) {
      let event: unknown;
      try {
        event = JSON.parse(line);
      } catch {
        continue;
      }
      if (typeof event !== "object" || event === null) continue;
      const { app, time } = event as { app?: unknown; time?: unknown };
      if (typeof app !== "object" || app === null) continue;
      const { key, path: appPath } = app as { key?: unknown; path?: unknown };
      if (typeof key !== "string" || key.length === 0 || typeof appPath !== "string") continue;
      const at = typeof time === "string" ? Date.parse(time) : Number.NaN;
      uses.push({ key, path: appPath, time: at });
    }
  }
  return uses;
}

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

/**
 * The names the Start Menu gives apps (helper `list_start_menu_apps`): by identity key (or, for an
 * entry with no program, its `app_id`), and by package family, executable path and file name, lower case.
 */
export interface StartMenuNames {
  byKey: Map<string, string>;
  byFamily: Map<string, string>;
  byPath: Map<string, string>;
  byFile: Map<string, string>;
}

export function noStartMenuNames(): StartMenuNames {
  return { byKey: new Map(), byFamily: new Map(), byPath: new Map(), byFile: new Map() };
}

/** Ticket 116: the Start Menu's names; `state` says whether the helper has answered. */
export interface StartMenu {
  names: StartMenuNames;
  state: "loading" | "ready" | "failed";
}

export function noStartMenu(state: "loading" | "failed" = "loading"): StartMenu {
  return { names: noStartMenuNames(), state };
}

/** Ticket 116: names that are no app the AI would operate (spec-deskpass, contents of the App table). */
const NOT_AN_APP_NAME = /^\s*(uninstall|help|解除安裝|移除|說明)/i;
/** Ticket 116: an uninstaller's file name (unins000.exe, uninstall.exe, LineUnInst.exe). */
const UNINSTALLER = /unins/i;
/** Ticket 116: the shell's own entries (the Run dialog), which run no app. */
const SHELL_OWN = /^shell:microsoft\.windows\.shell\./i;
interface MenuEntry {
  appId: string;
  name: string;
  key: string | null;
  program: string | null;
}

/**
 * Ticket 116: a `list_start_menu_apps` result (`{ apps: [{ app_id, name, key, program? }] }`): its
 * entries without uninstallers, help links and the shell's own entries, merged by identity key into
 * one app each. An app's one name is that of its entry named like its program (TortoiseGit's
 * "TortoiseGit", not its "Settings" shortcut, which runs TortoiseGitProc.exe too), else (ticket
 * 129) of an entry with a Chinese name, else of its entry that is the program itself (`exe:`), else
 * of its first; the names are the same by key, by
 * package family, by program path and by file name (the first app listed under one keeps it).
 */
export function startMenu(result: unknown): StartMenu {
  const menu: StartMenu = { names: noStartMenuNames(), state: "ready" };
  const listed = typeof result === "object" && result !== null ? (result as { apps?: unknown }).apps : undefined;
  if (!Array.isArray(listed)) return menu;
  const groups = new Map<string, MenuEntry[]>();
  for (const value of listed) {
    const entry = menuEntry(value);
    if (!entry || !isApp(entry)) continue;
    const group = (entry.key ?? entry.appId).toLowerCase();
    const members = groups.get(group);
    if (members) members.push(entry);
    else groups.set(group, [entry]);
  }
  const keep = (map: Map<string, string>, key: string, name: string) => {
    if (key.length > 0 && !map.has(key)) map.set(key, name);
  };
  for (const members of groups.values()) {
    const first = members[0]!;
    const key = first.key ?? first.appId;
    const program = members.map((entry) => entry.program).find((value) => value !== null) ?? null;
    const name = preferredName(members, program);
    keep(menu.names.byKey, key, name);
    if (key.startsWith("package:")) keep(menu.names.byFamily, key.slice("package:".length), name);
    for (const entry of members) {
      if (entry.appId.toLowerCase().startsWith("uwp:")) keep(menu.names.byFamily, entry.appId.slice(4).split("!")[0]!.toLowerCase(), name);
      if (entry.program === null) continue;
      keep(menu.names.byPath, entry.program.replaceAll("/", "\\").toLowerCase(), name);
      keep(menu.names.byFile, baseName(entry.program).toLowerCase(), name);
    }
  }
  return menu;
}

function menuEntry(value: unknown): MenuEntry | null {
  if (typeof value !== "object" || value === null) return null;
  const { app_id: appId, name, key, program } = value as Record<string, unknown>;
  if (typeof appId !== "string" || appId.length === 0 || typeof name !== "string" || name.trim().length === 0) return null;
  const executable = typeof program === "string" && program.length > 0 ? program : appId.toLowerCase().startsWith("exe:") ? appId.slice(4) : null;
  return { appId, name, key: typeof key === "string" && key.length > 0 ? key : null, program: executable };
}

function isApp(entry: MenuEntry): boolean {
  if (NOT_AN_APP_NAME.test(entry.name) || SHELL_OWN.test(entry.appId)) return false;
  return entry.program === null || !UNINSTALLER.test(baseName(entry.program));
}

function preferredName(members: MenuEntry[], program: string | null): string {
  const stem = program === null ? "" : comparable(baseName(program).replace(/\.exe$/i, ""));
  const likeProgram = (entry: MenuEntry) => {
    const name = comparable(entry.name);
    return stem.length >= 3 && name.length >= 3 && (stem.startsWith(name) || name.startsWith(stem));
  };
  const itself = (entry: MenuEntry) => program !== null && entry.appId.toLowerCase() === `exe:${program}`.toLowerCase();
  // Ticket 129: past a name like the program's, a Chinese (CJK) name is chosen over a Latin-only one (the Chinese name of Task Manager, not "Task Manager").
  const cjk = members.filter((entry) => CJK.test(entry.name));
  return (members.find(likeProgram) ?? cjk.find(itself) ?? cjk[0] ?? members.find(itself) ?? members[0]!).name;
}

/** Ticket 129: a Chinese, Japanese or Korean character. */
const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u;

/** Lower case, letters and digits only. */
function comparable(text: string): string {
  return text.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

export type AlwaysAllow = "yes" | "no" | "na";
export type TierSource = "default" | "builtIn" | "user";

/** One row of the App table, as `/api/apps` sends it. */
export interface SettingsApp {
  key: string;
  name: string;
  /** The signer's name, or what kind of app it is. */
  publisher: string;
  /** Its executable, when a grant or the log named one. */
  path: string | null;
  tier: Tier;
  source: TierSource;
  alwaysAllow: AlwaysAllow;
  /** When the AI last operated it (ISO), or null. */
  lastUsedAt: string | null;
  /** "Why this state". */
  why: string;
  builtIn: { tier: "blocked" | "high"; label: string } | null;
  grantCreatedAt: string | null;
  /**
   * Ticket 125: the user's raise of it in grants.json, whether or not it is in force: one not above
   * the built-in tier (written before the page refused such raises) raises nothing, and Restore default...
   * clears it too. Null when it has none.
   */
  raised: "high" | "blocked" | null;
}

/** The grants file's state as the page shows it: `message` is null when there is nothing to say. */
export interface GrantsState {
  state: GrantVerdict["state"];
  message: string | null;
}

export function grantsState(verdict: GrantVerdict, T: SettingsText = SETTINGS_TEXT): GrantsState {
  const text = T.grants;
  if (verdict.state === "void") return { state: "void", message: fill(text.void, { reason: text.reasons[verdict.reason] }) };
  if (verdict.state === "unverified") return { state: "unverified", message: text.unverified };
  return { state: "valid", message: verdict.ignored > 0 ? fill(verdict.ignored === 1 ? text.ignoredOne : text.ignored, { n: verdict.ignored }) : null };
}

/**
 * The rows of the "Used by the AI" list: each app's tier as the approval check decides it (its built-in
 * tier, or the user's raise when stricter: `GrantView.tierFor`), its always-allow, and the AI's last
 * use (the newer of its grant's `last_used_at` and its newest log event); newest use first, apps
 * without one last, the same time (or none) by name, compared lower-cased. Ticket 133: only these
 * apps are rows; the Start Menu (`menu`) gives them their names. Their sentences are in the language of `T`.
 */
export async function appRows(view: GrantView, logged: LoggedUse[], menu: StartMenu, T: SettingsText = SETTINGS_TEXT): Promise<SettingsApp[]> {
  const { names } = menu;
  const { grants, raised } = view.entries();
  const found = new Map<string, string | null>();
  const add = (key: string, appPath: string | null) => {
    if (key.length === 0) return;
    const usable = appPath !== null && appPath.length > 0 ? appPath : null;
    if (!found.has(key) || found.get(key) === null) found.set(key, usable);
  };
  for (const grant of grants) add(grant.key, grant.path);
  for (const raise of raised) add(raise.key, null);
  for (const use of logged) add(use.key, use.path);

  const lastLogged = new Map<string, number>();
  for (const use of logged) {
    if (!Number.isFinite(use.time)) continue;
    lastLogged.set(use.key, Math.max(lastLogged.get(use.key) ?? Number.NEGATIVE_INFINITY, use.time));
  }

  const rows: (SettingsApp & { lastMs: number | null })[] = [];
  for (const [key, appPath] of found) {
    const grant = grants.find((entry) => entry.key === key);
    const builtIn = builtInTierOf(key, appPath ?? "");
    const tier = await view.tierFor(key, builtIn ? { tier: builtIn.tier, why: builtIn.why } : { tier: "normal", why: null });
    const source: TierSource = tier.why === RAISED_WHY ? "user" : builtIn ? "builtIn" : "default";
    const signed = key.startsWith("signer:") || key.startsWith("package:");
    const granted = view.granted(key);
    const alwaysAllow: AlwaysAllow = tier.tier !== "normal" || !signed ? "na" : granted ? "yes" : "no";
    const times = [grant ? Date.parse(grant.last_used_at) : Number.NaN, lastLogged.get(key) ?? Number.NaN].filter(Number.isFinite);
    const lastMs = times.length > 0 ? Math.max(...times) : null;
    const userRaise = raisedTier(raised, key);
    const why = whyText(T, tier.tier, source, builtIn, granted, signed);
    rows.push({
      key,
      name: displayName(names, key, appPath, grant?.display_name ?? null),
      publisher: publisherOf(T, key),
      path: appPath,
      tier: tier.tier,
      source,
      alwaysAllow,
      lastUsedAt: lastMs === null ? null : new Date(lastMs).toISOString(),
      why: source !== "user" && userRaise !== null ? why + fill(T.why.idleRaise, { tier: T.page.tiers[userRaise] }) : why,
      builtIn: builtIn ? { tier: builtIn.tier, label: T.builtInTiers[builtIn.group].label } : null,
      grantCreatedAt: granted && grant ? grant.created_at : null,
      raised: userRaise,
      lastMs,
    });
  }
  rows.sort((a, b) => {
    if (a.lastMs !== null && b.lastMs !== null && a.lastMs !== b.lastMs) return b.lastMs - a.lastMs;
    if (a.lastMs !== null && b.lastMs === null) return -1;
    if (a.lastMs === null && b.lastMs !== null) return 1;
    const left = a.name.toLowerCase();
    const right = b.name.toLowerCase();
    return left < right ? -1 : left > right ? 1 : 0;
  });
  return rows.map(({ lastMs: _lastMs, ...row }) => row);
}

/** Ticket 125: the highest raise (`high` or `blocked`) grants.json has for `key`, in force or not; null when none. */
function raisedTier(raised: readonly { key: string; tier: string }[], key: string): "high" | "blocked" | null {
  let found: "high" | "blocked" | null = null;
  for (const entry of raised) {
    if (entry.key !== key || (entry.tier !== "high" && entry.tier !== "blocked")) continue;
    if (found === null || TIER_RANK[entry.tier] > TIER_RANK[found]) found = entry.tier;
  }
  return found;
}

function whyText(T: SettingsText, tier: Tier, source: TierSource, builtIn: ReturnType<typeof builtInTierOf>, granted: boolean, signed: boolean): string {
  const text = T.why;
  const tierName = T.page.tiers;
  if (source === "user") return fill(text.user, { tier: tierName[tier], builtIn: tierName[builtIn?.tier ?? "normal"] });
  if (builtIn) return fill(text.builtIn, { ...T.builtInTiers[builtIn.group], tier: tierName[builtIn.tier] });
  if (granted) return text.alwaysAllowed;
  return signed ? text.normal : text.unsigned;
}

/** A signer's name (its subject's CN, else O, else the whole subject), or what kind of app it is. */
function publisherOf(T: SettingsText, key: string): string {
  const text = T.publisher;
  if (key.startsWith("package:")) return text.store;
  if (!key.startsWith("signer:")) return text.unsigned;
  const subject = key.slice("signer:".length, key.lastIndexOf("|"));
  const field = (name: string) => new RegExp(`(?:^|,\\s*)${name}=("(?:[^"]|"")*"|[^,]*)`).exec(subject)?.[1]?.replace(/^"|"$/g, "").replaceAll('""', '"').trim();
  const name = field("CN") || field("O") || subject;
  return fill(text.signed, { name });
}

/**
 * The app's one name (the retired helper panel.rs, `app_name`): its Start Menu name (ticket 116: by its
 * identity key first; then a package by its family, a program by its path, then by its file name),
 * else its grant's `display_name`, else its executable's file name; a package known by nothing else
 * by its package name.
 */
export function displayName(names: StartMenuNames, key: string, appPath: string | null, grantName: string | null): string {
  const byKey = names.byKey.get(key);
  if (byKey) return byKey;
  let family: string | null = null;
  let keyPath: string | null = null;
  let signerFile: string | null = null;
  if (key.startsWith("signer:")) signerFile = key.slice(key.lastIndexOf("|") + 1);
  else if (key.startsWith("package:")) family = key.slice("package:".length).toLowerCase();
  else keyPath = key;
  const families = [family, appPath ? packageFamilyFromPath(appPath) : null, keyPath ? packageFamilyFromPath(keyPath) : null].filter((value): value is string => value !== null);
  const paths = [appPath, keyPath].filter((value): value is string => value !== null && value.length > 0);
  const files = [appPath ? baseName(appPath) : null, signerFile, keyPath ? baseName(keyPath) : null].filter((value): value is string => value !== null && value.length > 0);
  const listed =
    families.map((value) => names.byFamily.get(value)).find(Boolean) ??
    paths.map((value) => names.byPath.get(value.replaceAll("/", "\\").toLowerCase())).find(Boolean) ??
    files.map((value) => names.byFile.get(value.toLowerCase())).find(Boolean);
  if (listed) return listed;
  if (grantName !== null && grantName.trim().length > 0) return grantName;
  if (files[0]) return files[0];
  return family ? (family.split("_")[0] ?? family) : key;
}

function baseName(file: string): string {
  return path.win32.basename(file.replaceAll("/", "\\"));
}
