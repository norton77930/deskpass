import { randomUUID, timingSafeEqual } from "node:crypto";
import { mkdir, readFile, rename, stat, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { takeAlwaysAllow, type AlwaysAllowAnswer } from "./approval-dialog.ts";
import type { GrantInvalidReason, GrantLogEvent } from "./log.ts";
import { appKey, effectiveTier, TIER_RANK, type Tier, type TierMatch } from "./policy.ts";

/**
 * Ticket 94: the user's always-allow grants and raised tiers, in
 * `%LOCALAPPDATA%\deskpass\grants.json` (spec-phase4b, grants file and tamper protection):
 *
 *     { "version": 1, "generation": 7,
 *       "grants": [{ "key", "display_name", "path", "created_at", "last_used_at", "mac" }],
 *       "raised": [{ "key", "tier", "created_at", "mac" }],
 *       "file_mac": "..." }
 *
 * `key` is the app's identity key (`identityKey`). Every field of an entry is a string, and an
 * entry has exactly these fields. Each `mac` is HMAC-SHA256, as 64 lower-case hex digits, of the
 * entry's other fields as canonical JSON, under the 32-byte key the helper keeps in `grants.key`,
 * protected with DPAPI (CurrentUser). `file_mac` is the same over the file itself:
 * `{ version, generation, grant_macs, raised_macs }` (the entries' `mac`s in file order), so
 * deleting, adding or reordering entries voids the whole file. `generation` goes up by one with
 * every write; a reading with a lower generation than one this server has accepted is a rollback
 * (an older copy put back) and voids the file as well. The helper's `grant_mac` canonicalizes and
 * makes every MAC, so the key never leaves the helper and the control panel (ticket 96, in the
 * helper) shares the one canonical form; this module reads and writes the file and compares.
 *
 * An entry that is malformed or whose MAC does not verify is ignored and logged `grant_invalid`;
 * a file whose own MAC is missing or wrong, or that is a rollback, is void as a whole; a key that
 * could not be unlocked voids every entry, logged `grant_key_lost`, and the helper makes a new key.
 * While the file is void, no grant counts and the raises last verified still do (a raise only makes
 * an app stricter). The file is read again whenever its modification time or size changes, checked
 * at every approval (`current`). No MCP tool writes it: the server writes the approval dialog's
 * always-allow (ticket 95) and the last-used times; revoking a grant and raising a tier are the
 * settings page's (ticket 113: `revoke`, `raise`, through the settings server's own GrantStore;
 * `removeRaise` for ticket 115), as they were the control panel's (ticket 96,
 * helper/src/grants.rs, the same format and rules). A running server sees another process's write
 * at its next approval check, since `current` compares the file's modification time and size.
 */

export const GRANTS_FILE = "grants.json";
export const GRANTS_VERSION = 1;
/** A grants.json larger than this is not read (`unreadable`). */
export const GRANTS_MAX_BYTES = 1024 * 1024;
/** `last_used_at` is written at most this often for one grant (spec-phase4b, app identity and always allow). */
export const LAST_USED_WRITE_MS = 60_000;

export interface Grant {
  key: string;
  display_name: string;
  path: string;
  created_at: string;
  last_used_at: string;
}

export interface Raised {
  key: string;
  /** `high` or `blocked`; anything else, though signed, raises nothing (`tierFor`). */
  tier: string;
  created_at: string;
}

type Signed<T> = T & { mac: string };

/** What the helper's `grant_mac` said about its key: `created` for a key it had to make where there was none. */
export type KeyState = "ok" | "created" | "lost_recreated";

/** What one MAC is made over; the helper checks the fields and canonicalizes them. */
export type MacEntry =
  | { kind: "grant"; fields: Grant }
  | { kind: "raised"; fields: Raised }
  | { kind: "file"; fields: { version: number; generation: number; grant_macs: string[]; raised_macs: string[] } };

/**
 * Sends `grant_mac` to the helper: `{ entries }` → `{ macs, key_state }`. Ticket 95: the helper keeps
 * its key in the grants folder it was started with (`--grants-dir`, which the server always passes:
 * ticket 96), so no request names the key file.
 */
export type GrantMac = (params: { entries: MacEntry[] }) => Promise<unknown>;

const GRANT_FIELDS = ["key", "display_name", "path", "created_at", "last_used_at"] as const;
const RAISED_FIELDS = ["key", "tier", "created_at"] as const;
const RAISED_TIERS: readonly Tier[] = ["normal", "high", "blocked"];
const MAC_PATTERN = /^[0-9a-f]{64}$/;
/** A lone surrogate: JSON text the helper could not read back. */
const LONE_SURROGATE = /\p{Cs}/u;

/** `text` with each lone surrogate replaced by U+FFFD, so the helper can read it. */
function wellFormed(text: string): string {
  return text.replace(/\p{Cs}/gu, "�");
}
/** The longest `key` a log event repeats; a hand-made entry's key can be anything. */
const LOGGED_KEY_MAX = 300;

/** Only an app with an identity can be always allowed: a signer's or a package's key, never a path. */
export function isSignedKey(key: string): boolean {
  return key.startsWith("signer:") || key.startsWith("package:");
}

/** One reading of the file: what verified, and what counts. */
interface Loaded {
  /** The file's modification time and size when read; `missing` when there was none. */
  version: string;
  /** The file's generation (0 for none); a write makes the next one. */
  generation: number;
  /** The verified entries, which a write keeps; none while the file is void. */
  grants: Signed<Grant>[];
  raised: Signed<Raised>[];
  /** The raises that count: the file's, or while it is void, the ones last verified. */
  effectiveRaised: Signed<Raised>[];
  /** Keys whose `not_higher` raise has been logged since this reading. */
  notHigherLogged: Set<string>;
  /** Ticket 112: whether this reading verified, for the settings page. */
  verdict: GrantVerdict;
}

/**
 * Ticket 112: what one reading of grants.json came to, for the settings page: `valid` (a missing
 * file too; `ignored` entries did not verify and do not count), `void` as a whole (`reason`; a
 * lost key is `key_lost`), or `unverified` (the helper could not make the MACs this time).
 */
export type GrantVerdict = { state: "valid"; ignored: number } | { state: "void"; reason: GrantInvalidReason | "key_lost" } | { state: "unverified" };

/**
 * Ticket 113: the settings page's writes (`raise`, `revoke`, `removeRaise`) refuse a reading that
 * did not verify, as the control panel's did (helper/src/grants.rs `update`): written over, a void
 * file would lose the raises a running MCP server still keeps from it, and this process may never
 * have verified them. The user is told the file is void instead.
 */
export class GrantFileVoidError extends Error {
  constructor() {
    super("grants.json did not verify; no change is written over it");
    this.name = "GrantFileVoidError";
  }
}

/** Ticket 113: what `GrantStore.raise` did: raised (and whether a grant went with it), or nothing, the app being in `tier` already or higher. */
export type RaiseOutcome = { raised: true; grantRemoved: boolean } | { raised: false; tier: Tier };

export interface GrantStoreOptions {
  /** The folder of grants.json and grants.key. */
  dir: string;
  /** Writes a grant event to the log. */
  record: (event: GrantLogEvent) => Promise<void>;
}

/**
 * Ticket 94: grants.json as of the last approval check. One per server; `current` reads the file
 * again when it changed, and the write methods replace it whole (a temporary file renamed over it).
 */
export class GrantStore {
  readonly file: string;
  private readonly record: GrantStoreOptions["record"];
  private loaded: Loaded | undefined;
  /** The raises of the last reading that verified, kept while the file cannot be verified. */
  private lastRaised: Signed<Raised>[] = [];
  /** The highest generation this server has accepted or written; a lower one is a rollback. */
  private highWater = 0;
  private writes: Promise<unknown> = Promise.resolve();

  constructor(options: GrantStoreOptions) {
    this.file = path.join(options.dir, GRANTS_FILE);
    this.record = options.record;
  }

  /**
   * The verified grants and raises, the file read again when it changed. When the helper cannot
   * make the MACs (an error, a timeout, an answer out of shape), no grant counts for this check,
   * the raises last verified still do, and the file is read again at the next check.
   */
  async current(mac: GrantMac): Promise<GrantView> {
    const loaded = await this.load(mac);
    return new GrantView(this, loaded ?? this.voided("", 0, { state: "unverified" }));
  }

  /**
   * Ticket 95: adds the always-allow grant the user gave in the approval dialog (`answer`, which
   * only the dialog's answer makes, and which is taken once), replacing any its key had; its
   * `created_at` and `last_used_at` are now. Nothing is written (false) for any other value, an
   * answer already taken, a key that is not a signer's or a package's, a key with a lone surrogate
   * (JSON the helper cannot read: its request would be dropped), or an app the user raised. A lone
   * surrogate in the name or path is replaced with U+FFFD.
   */
  addGrant(mac: GrantMac, answer: AlwaysAllowAnswer, now = Date.now()): Promise<boolean> {
    const app = takeAlwaysAllow(answer);
    if (!app || !isSignedKey(app.key) || LONE_SURROGATE.test(app.key)) return Promise.resolve(false);
    const time = new Date(now).toISOString();
    const grant: Grant = { key: app.key, display_name: wellFormed(app.displayName), path: wellFormed(app.path), created_at: time, last_used_at: time };
    return this.update(mac, (file) => {
      if (file.raised.some((raised) => raised.key === grant.key && isRaise(raised.tier))) return false;
      file.grants = file.grants.filter((entry) => entry.key !== grant.key);
      file.grants.push(grant);
      return true;
    });
  }

  /**
   * Ticket 113: raises the app of `key` to `tier` (`high` or `blocked`) as of `now`, replacing any
   * raise it had, and removes its always-allow grant in the same write (spec: raising removes that app's
   * always-allow; helper/src/grants.rs `raise`). Only stricter: nothing is written (`not_stricter`, with
   * the tier it is in) unless `tier` is above the app's tier as of this write, the higher of its
   * built-in tier (`builtIn`, the caller's to know) and the raise the file has for it. Checked on
   * the reading the write starts from, so a raise made meanwhile elsewhere is not lowered.
   */
  raise(mac: GrantMac, key: string, tier: "high" | "blocked", builtIn: Tier, now = Date.now()): Promise<RaiseOutcome> {
    if (!writableKey(key) || !isRaise(tier)) return Promise.reject(new RangeError("a raise needs an app key and the tier high or blocked"));
    let outcome: RaiseOutcome = { raised: false, tier: builtIn };
    return this.update(mac, (file) => {
      let current = builtIn;
      for (const entry of file.raised) {
        if (entry.key === key && isRaise(entry.tier) && TIER_RANK[entry.tier as Tier] > TIER_RANK[current]) current = entry.tier as Tier;
      }
      if (TIER_RANK[tier] <= TIER_RANK[current]) {
        outcome = { raised: false, tier: current };
        return false;
      }
      const grants = file.grants.length;
      file.grants = file.grants.filter((entry) => entry.key !== key);
      file.raised = file.raised.filter((entry) => entry.key !== key);
      file.raised.push({ key, tier, created_at: new Date(now).toISOString() });
      outcome = { raised: true, grantRemoved: file.grants.length !== grants };
      return true;
    }, SETTINGS_WRITE).then(() => outcome);
  }

  /** Ticket 113: removes the always-allow grant of `key` (helper/src/grants.rs `revoke`); false when it had none. */
  revoke(mac: GrantMac, key: string): Promise<boolean> {
    if (!writableKey(key)) return Promise.reject(new RangeError("a revoke needs an app key"));
    return this.update(mac, (file) => {
      const before = file.grants.length;
      file.grants = file.grants.filter((entry) => entry.key !== key);
      return file.grants.length !== before;
    }, SETTINGS_WRITE);
  }

  /**
   * Ticket 113, for ticket 115: removes the user's raise of `key`, which goes back to its built-in
   * tier (helper/src/grants.rs `remove_raise`); false when it had none. This lowers an app: only a
   * change the user confirmed in the native confirm window may call it. Ticket 115: `expected` is
   * the tier the user was shown and confirmed removing; checked on the reading the write starts
   * from, nothing is written (false) unless the app's raise is still that tier, so a raise made
   * stricter (or replaced) while the window was open is not removed with a confirm for another one.
   */
  removeRaise(mac: GrantMac, key: string, expected?: "high" | "blocked"): Promise<boolean> {
    if (!writableKey(key)) return Promise.reject(new RangeError("removing a raise needs an app key"));
    return this.update(mac, (file) => {
      const mine = file.raised.filter((entry) => entry.key === key);
      if (mine.length === 0) return false;
      if (expected !== undefined && !mine.some((entry) => entry.tier === expected)) return false;
      if (expected !== undefined && mine.some((entry) => isRaise(entry.tier) && TIER_RANK[entry.tier as Tier] > TIER_RANK[expected])) return false;
      file.raised = file.raised.filter((entry) => entry.key !== key);
      return true;
    }, SETTINGS_WRITE);
  }

  /**
   * Records that the grant of `key` was used: its `last_used_at` is set to now, at most once a
   * minute, and the entry signed again, since the MAC covers it.
   */
  markUsed(mac: GrantMac, key: string, now = Date.now()): Promise<boolean> {
    return this.update(mac, (file) => {
      const grant = file.grants.find((entry) => entry.key === key);
      if (!grant) return false;
      const last = Date.parse(grant.last_used_at);
      if (Number.isFinite(last) && now - last < LAST_USED_WRITE_MS && now >= last) return false;
      grant.last_used_at = new Date(now).toISOString();
      return true;
    });
  }

  /**
   * One write: the verified entries as of now, changed by `change` (false: nothing to write), each
   * signed again under the helper's current key, then the file signed with the next generation,
   * written to a temporary file in the same folder and renamed over grants.json. Entries that did
   * not verify are dropped. Should the key turn out to be new (lost and made again) while the
   * reading had entries, they are not signed under it: the reading is voided as a load would, and
   * the change applied to nothing. Writes of this server run one at a time. Another writer (the
   * control panel) replaces the file whole as well, so the file is always one writer's complete
   * version; should the file change between this write's reading and its rename, the write starts
   * over from the new file, so a revoke or raise made meanwhile is not undone (the window left is
   * the rename itself).
   *
   * Ticket 95: a write while the reading is void (no file_mac, a bad one, a rollback, a lost key)
   * keeps the raises in force then, the ones last verified (`effectiveRaised`), and writes them
   * back signed with the rest: the file it writes replaces the void one, and a raise only makes an
   * app stricter, so dropping one there would lower the app. For a reading that verified,
   * `effectiveRaised` is the file's raises.
   */
  private update(mac: GrantMac, change: (file: { grants: Grant[]; raised: Raised[] }) => boolean, { verifiedOnly = false }: { verifiedOnly?: boolean } = {}): Promise<boolean> {
    const run = this.writes.then(async () => {
      for (let attempt = 0; attempt < 3; attempt += 1) {
        const loaded = await this.load(mac);
        if (!loaded) throw new Error("the grants file could not be verified");
        // Ticket 113: the settings page's writes are made only over a file that verified (`GrantFileVoidError`).
        if (verifiedOnly && loaded.verdict.state !== "valid") throw new GrantFileVoidError();
        const file = { grants: loaded.grants.map(({ mac: _mac, ...grant }) => grant), raised: loaded.effectiveRaised.map(({ mac: _mac, ...raised }) => raised) };
        // Only entries the file holds now were verified under the key it was read with; carried raises were verified earlier.
        const hadEntries = loaded.grants.length + loaded.raised.length > 0;
        if (!change(file)) return false;
        const entries: MacEntry[] = [...file.grants.map((fields) => ({ kind: "grant" as const, fields })), ...file.raised.map((fields) => ({ kind: "raised" as const, fields }))];
        const signed = entries.length === 0 ? { macs: [], keyState: "ok" as KeyState } : await askMacs(mac, entries);
        if (!signed) throw new Error("the helper did not sign the grants");
        if (signed.keyState !== "ok" && hadEntries) {
          // The entries were verified under a key that is gone: void them, as a load would have.
          await this.record({ event: "grant_key_lost", entries: loaded.grants.length + loaded.raised.length });
          this.loaded = this.voided(loaded.version, loaded.generation, { state: "void", reason: "key_lost" });
          continue;
        }
        const generation = Math.max(loaded.generation, this.highWater) + 1;
        const grantMacs = signed.macs.slice(0, file.grants.length);
        const raisedMacs = signed.macs.slice(file.grants.length);
        const sealed = await askMacs(mac, [{ kind: "file", fields: { version: GRANTS_VERSION, generation, grant_macs: grantMacs, raised_macs: raisedMacs } }]);
        if (!sealed || (sealed.keyState !== "ok" && hadEntries)) throw new Error("the helper did not sign the grants file");
        const content = {
          version: GRANTS_VERSION,
          generation,
          grants: file.grants.map((grant, index) => ({ ...grant, mac: grantMacs[index] })),
          raised: file.raised.map((raised, index) => ({ ...raised, mac: raisedMacs[index] })),
          file_mac: sealed.macs[0],
        };
        await mkdir(path.dirname(this.file), { recursive: true });
        const temp = `${this.file}.${process.pid}.${randomUUID()}.tmp`;
        try {
          await writeFile(temp, `${JSON.stringify(content, null, 2)}\n`, "utf8");
          if ((await fileVersion(this.file)).version !== loaded.version) {
            await unlink(temp);
            this.loaded = undefined;
            continue;
          }
          await renameOver(temp, this.file);
        } catch (error) {
          await unlink(temp).catch(() => undefined);
          throw error;
        }
        this.highWater = Math.max(this.highWater, generation);
        // Read (and verified) again at the next check, like any change to the file.
        this.loaded = undefined;
        return true;
      }
      throw new Error("grants.json kept changing while it was written");
    });
    this.writes = run.catch(() => undefined);
    return run;
  }

  /** A reading that does not count: no grants, the raises last verified. */
  private voided(version: string, generation: number, verdict: GrantVerdict): Loaded {
    return { version, generation, grants: [], raised: [], effectiveRaised: this.lastRaised, notHigherLogged: new Set(), verdict };
  }

  /** The file as last read, or read again (and verified) when it changed; undefined when it could not be verified now. */
  private async load(mac: GrantMac): Promise<Loaded | undefined> {
    const before = await fileVersion(this.file);
    if (this.loaded && before.version !== null && this.loaded.version === before.version) return this.loaded;
    const settle = async (result: Loaded, events: GrantLogEvent[]): Promise<Loaded> => {
      for (const event of events) await this.record(event);
      // Kept only when the file did not change while it was read; otherwise it is read again next time.
      this.loaded = before.version !== null && (await fileVersion(this.file)).version === before.version ? result : undefined;
      return result;
    };
    const version = before.version ?? "";
    const parsed = await this.read(before);
    if (parsed === null) return settle(this.voided(version, 0, { state: "void", reason: "unreadable" }), [invalid("file", null, "unreadable")]);
    // A reading that does not count keeps the generation this server accepted: an unverified one is not taken.
    const voidFile = (reason: GrantInvalidReason) => settle(this.voided(version, this.highWater, { state: "void", reason }), [invalid("file", null, reason)]);

    const entryCount = parsed.grants.length + parsed.raised.length;
    const storedMacs = [...parsed.grants, ...parsed.raised].map(macOf);
    // Only a missing file is unsealed: any file there, empty or not, must carry its file_mac.
    if (before.version !== "missing" && parsed.fileMac === undefined) return voidFile("not_signed");
    if (parsed.fileMac !== undefined && !storedMacs.every((stored) => stored !== null)) return voidFile("bad_mac");

    const events: GrantLogEvent[] = [];
    const candidates = { grants: [] as Signed<Grant>[], raised: [] as Signed<Raised>[] };
    for (const value of parsed.grants) {
      const grant = readEntry<Grant>(value, GRANT_FIELDS);
      if (!grant) events.push(invalid("grant", keyOf(value), "malformed"));
      else candidates.grants.push(grant);
    }
    for (const value of parsed.raised) {
      const raised = readEntry<Raised>(value, RAISED_FIELDS);
      if (!raised || !RAISED_TIERS.includes(raised.tier as Tier)) events.push(invalid("raised", keyOf(value), "malformed"));
      else candidates.raised.push(raised);
    }
    const request: MacEntry[] = [
      ...candidates.grants.map(({ mac: _mac, ...fields }) => ({ kind: "grant" as const, fields })),
      ...candidates.raised.map(({ mac: _mac, ...fields }) => ({ kind: "raised" as const, fields })),
    ];
    if (parsed.fileMac !== undefined) {
      const macs = storedMacs as string[];
      request.push({ kind: "file", fields: { version: GRANTS_VERSION, generation: parsed.generation, grant_macs: macs.slice(0, parsed.grants.length), raised_macs: macs.slice(parsed.grants.length) } });
    }
    const result: Loaded = { version, generation: parsed.generation, grants: [], raised: [], effectiveRaised: [], notHigherLogged: new Set(), verdict: { state: "valid", ignored: 0 } };
    if (request.length > 0) {
      const answer = await askMacs(mac, request);
      if (!answer) return undefined;
      if (answer.keyState !== "ok") {
        // Everything was made under a key that is gone: none of it can be trusted, and none is logged on its own.
        return settle(this.voided(version, this.highWater, { state: "void", reason: "key_lost" }), [{ event: "grant_key_lost", entries: entryCount }]);
      }
      if (parsed.fileMac !== undefined && !sameMac(parsed.fileMac, answer.macs[request.length - 1] ?? "")) return voidFile("bad_mac");
      const entries: Signed<Grant | Raised>[] = [...candidates.grants, ...candidates.raised];
      entries.forEach((entry, index) => {
        const kind = index < candidates.grants.length ? "grant" : "raised";
        if (!sameMac(entry.mac, answer.macs[index] ?? "")) events.push(invalid(kind, entry.key, "bad_mac"));
        else if (kind === "grant" && !isSignedKey(entry.key)) events.push(invalid(kind, entry.key, "not_signed"));
        else if (kind === "grant") result.grants.push(entry as Signed<Grant>);
        else result.raised.push(entry as Signed<Raised>);
      });
    }
    // An older copy of the file (or none, or an empty one) after a newer one was accepted: a rollback.
    if (parsed.generation < this.highWater) return voidFile("rollback");
    this.highWater = parsed.generation;
    result.effectiveRaised = result.raised;
    this.lastRaised = result.raised;
    result.verdict = { state: "valid", ignored: events.length };
    return settle(result, events);
  }

  /** The file's parts, an empty file when there is none, or null when it is not a version 1 grants file. */
  private async read(before: { version: string | null; size: number }): Promise<ParsedFile | null> {
    if (before.version === "missing") return { generation: 0, grants: [], raised: [] };
    if (before.size > GRANTS_MAX_BYTES) return null;
    let text: string;
    try {
      const bytes = await readFile(this.file);
      if (bytes.length > GRANTS_MAX_BYTES) return null;
      text = bytes.toString("utf8");
    } catch (error) {
      return (error as { code?: unknown }).code === "ENOENT" ? { generation: 0, grants: [], raised: [] } : null;
    }
    return parseFile(text);
  }

  /** Logs once per reading of the file that the raise of `key` raises nothing. */
  async noteNotHigher(loaded: Loaded, key: string): Promise<void> {
    if (loaded.notHigherLogged.has(key)) return;
    loaded.notHigherLogged.add(key);
    await this.record(invalid("raised", key, "not_higher"));
  }
}

/** The grants and raises that count, as of one approval check. */
export class GrantView {
  private readonly store: GrantStore;
  private readonly loaded: Loaded;

  constructor(store: GrantStore, loaded: Loaded) {
    this.store = store;
    this.loaded = loaded;
  }

  /**
   * The tier `key` is treated in (`effectiveTier`): its built-in tier, or the user's raise when that
   * is stricter. A raise that is not (a request to lower a built-in tier, or one equal to it) is
   * ignored and logged once per reading of the file. A raise under `priorKey` (`priorSignerKey`, the
   * key the app had before the original-name rule) counts too.
   */
  async tierFor(key: string, builtIn: TierMatch, priorKey?: string | null): Promise<TierMatch> {
    let raised: Tier | null = null;
    for (const entry of this.loaded.effectiveRaised) {
      if (entry.key !== key && entry.key !== priorKey) continue;
      const tier = entry.tier as Tier;
      if (raised === null || TIER_RANK[tier] > TIER_RANK[raised]) raised = tier;
    }
    const { tier, ignoredRaise } = effectiveTier(builtIn, raised);
    if (ignoredRaise) await this.store.noteNotHigher(this.loaded, key);
    return tier;
  }

  /** Ticket 96: the keys the user raised (to high or blocked) that are in force, for checks that look an app's identity up only when a raise could be about it. */
  raisedKeys(): string[] {
    return this.loaded.effectiveRaised.filter((entry) => isRaise(entry.tier)).map((entry) => entry.key);
  }

  /** Ticket 112: whether the reading behind this view verified (`GrantVerdict`). */
  verdict(): GrantVerdict {
    return this.loaded.verdict;
  }

  /**
   * Ticket 112: the entries that count, without their MACs, for the settings page: the verified
   * grants (none while the file is void) and the raises in force (`effectiveRaised`).
   */
  entries(): { grants: Grant[]; raised: Raised[] } {
    return {
      grants: this.loaded.grants.map(({ mac: _mac, ...grant }) => grant),
      raised: this.loaded.effectiveRaised.map(({ mac: _mac, ...raised }) => raised),
    };
  }

  /** Whether `key` has an always-allow grant that counts: verified, a signed app's, and not raised by the user. */
  granted(key: string): boolean {
    if (!isSignedKey(key)) return false;
    if (this.loaded.effectiveRaised.some((entry) => entry.key === key && isRaise(entry.tier))) return false;
    return this.loaded.grants.some((entry) => entry.key === key);
  }

  /**
   * Ticket 95: the verified grants that were given for the executable at `appPath` but under
   * another key than `key`, its identity now: the signer changed, the signature no longer
   * verifies, or the identity could not be had. They are not used (`granted` matches keys only);
   * the server logs them `grant_mismatch` when it asks. Matched by the grant's `path`, normalized
   * as `appKey` does: a grant follows its app's identity, and its path is the one place it names
   * the executable it was given for. An app that moved (a packaged app's new version folder) and
   * lost its identity as well matches nothing and is simply asked.
   */
  mismatched(key: string, appPath: string): string[] {
    const where = appKey(appPath);
    return this.loaded.grants.filter((entry) => entry.key !== key && appKey(entry.path) === where).map((entry) => entry.key);
  }
}

interface ParsedFile {
  generation: number;
  grants: unknown[];
  raised: unknown[];
  fileMac?: string;
}

function isRaise(tier: string): boolean {
  return tier === "high" || tier === "blocked";
}

/** A key a write can sign: not empty, and no lone surrogate (JSON the helper could not read). */
/** How the settings page's writes call `update`: never over a file that did not verify. */
const SETTINGS_WRITE = { verifiedOnly: true } as const;

function writableKey(key: string): boolean {
  return typeof key === "string" && key.length > 0 && !LONE_SURROGATE.test(key);
}

function invalid(entry: "grant" | "raised" | "file", key: string | null, reason: GrantInvalidReason): GrantLogEvent {
  return { event: "grant_invalid", entry, key: key === null ? null : Array.from(key).slice(0, LOGGED_KEY_MAX).join(""), reason };
}

function keyOf(value: unknown): string | null {
  const key = typeof value === "object" && value !== null ? (value as { key?: unknown }).key : undefined;
  return typeof key === "string" ? key : null;
}

/** An entry's stored `mac` when it is 64 hex digits, else null. */
function macOf(value: unknown): string | null {
  const mac = typeof value === "object" && value !== null ? (value as { mac?: unknown }).mac : undefined;
  return typeof mac === "string" && MAC_PATTERN.test(mac) ? mac : null;
}

/**
 * The file's parts, or null when it is not a version 1 grants file: not a JSON object, another
 * version, lists that are not arrays, a generation that is not a whole number, a `file_mac` that is
 * not 64 hex digits, or a field it does not have.
 */
function parseFile(text: string): ParsedFile | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const { version, generation, grants, raised, file_mac: fileMac, ...rest } = value as Record<string, unknown>;
  if (version !== GRANTS_VERSION || Object.keys(rest).length > 0) return null;
  if ((grants !== undefined && !Array.isArray(grants)) || (raised !== undefined && !Array.isArray(raised))) return null;
  if (generation !== undefined && !(Number.isSafeInteger(generation) && (generation as number) >= 0)) return null;
  if (fileMac !== undefined && !(typeof fileMac === "string" && MAC_PATTERN.test(fileMac))) return null;
  return {
    generation: (generation as number | undefined) ?? 0,
    grants: (grants as unknown[] | undefined) ?? [],
    raised: (raised as unknown[] | undefined) ?? [],
    ...(fileMac === undefined ? {} : { fileMac: fileMac as string }),
  };
}

/**
 * An entry with exactly `fields` and `mac`, every one a string without a lone surrogate, the key
 * not empty and the MAC 64 hex digits; else null.
 */
function readEntry<T>(value: unknown, fields: readonly string[]): Signed<T> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const entry = value as Record<string, unknown>;
  const names = Object.keys(entry);
  if (names.length !== fields.length + 1 || !names.includes("mac") || !fields.every((name) => names.includes(name))) return null;
  if (!names.every((name) => typeof entry[name] === "string" && !LONE_SURROGATE.test(entry[name] as string))) return null;
  if ((entry.key as string).length === 0 || !MAC_PATTERN.test(entry.mac as string)) return null;
  return entry as unknown as Signed<T>;
}

/** The helper's MACs for `entries`, or null when its answer is an error or out of shape. */
async function askMacs(mac: GrantMac, entries: MacEntry[]): Promise<{ macs: string[]; keyState: KeyState } | null> {
  let answer: unknown;
  try {
    answer = await mac({ entries });
  } catch {
    return null;
  }
  if (typeof answer !== "object" || answer === null) return null;
  const { macs, key_state: keyState } = answer as Record<string, unknown>;
  if (keyState !== "ok" && keyState !== "created" && keyState !== "lost_recreated") return null;
  if (!Array.isArray(macs) || macs.length !== entries.length || !macs.every((value) => typeof value === "string" && MAC_PATTERN.test(value))) return null;
  return { macs: macs as string[], keyState };
}

/** Compares two MACs (64 hex digits each) in time that does not depend on where they differ. */
function sameMac(stored: string, computed: string): boolean {
  const a = Buffer.from(stored, "hex");
  const b = Buffer.from(computed, "hex");
  return a.length === 32 && b.length === 32 && timingSafeEqual(a, b);
}

/** The file's modification time and size as one string (`missing` when there is none, null when it cannot be read), and its size. */
async function fileVersion(file: string): Promise<{ version: string | null; size: number }> {
  try {
    const { mtimeMs, size } = await stat(file);
    return { version: `${mtimeMs}:${size}`, size };
  } catch (error) {
    return { version: (error as { code?: unknown }).code === "ENOENT" ? "missing" : null, size: 0 };
  }
}

/** Renames `temp` over `file`, retrying briefly while a reader has the file open (Windows refuses then). */
async function renameOver(temp: string, file: string): Promise<void> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      await rename(temp, file);
      return;
    } catch (error) {
      const code = (error as { code?: unknown }).code;
      if ((code !== "EPERM" && code !== "EBUSY" && code !== "EACCES") || attempt >= 20) throw error;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
}
