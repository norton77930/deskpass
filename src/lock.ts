import { randomUUID } from "node:crypto";
import { link, mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { LOCK_IDLE_MS } from "./config.ts";

export interface LockIdentity {
  pid: number;
  startedAt: string;
}

/**
 * The lock file's content. Ticket 135 adds `lastUsedAt` (ISO): when the holder's last tool call ended
 * (or it took the lock). A record without it, from an older version, is still a lock.
 */
interface LockRecord {
  pid: number;
  startedAt: string;
  lastUsedAt?: string;
}

export interface SessionLockOptions {
  /** The clock `lastUsedAt` is written and read on, in milliseconds; for tests. */
  now?: () => number;
}

/** What is known about a process: gone, running since a time, or running with an unreadable start. */
export type ProcessStart = { state: "missing" } | { state: "running"; startedAt: Date } | { state: "unknown"; reason: string };

/**
 * Answers whether a pid is running and since when. The server asks the helper, which reads it
 * with OpenProcess + GetProcessTimes; a rejection (helper crashed, timed out, or failed) is passed
 * through to the tool call and the lock is left as it is.
 */
export type ProcessProbe = (pid: number) => Promise<ProcessStart>;

export type LockOutcome = { acquired: true } | { acquired: false; message: string };

const START_TOLERANCE_MS = 2_000;

/**
 * How long a lock file that cannot be read as a record is treated as another session still
 * creating it. This server never exposes a partial file (see `acquire`), but another writer might;
 * after this long an unreadable file is a leftover and is taken over, so it cannot block forever.
 */
const UNREADABLE_GRACE_MS = 5_000;

const IN_USE_MESSAGE = "The computer is being operated by another AI session. Report this to the user instead of retrying.";

/**
 * Ticket 135: the in_use message for a live holder whose record was read: when it started (local time
 * and ISO), how long ago its last tool call ended when the record says, and that an idle holder
 * releases the computer by itself, so the model can tell the user to wait or which session to close.
 */
function inUseMessage(record: LockRecord, now: number): string {
  const started = `The computer is being operated by another AI session (process ${record.pid}, started ${localTime(new Date(record.startedAt))} local time, ${record.startedAt}).`;
  const lastUsed = record.lastUsedAt === undefined ? Number.NaN : new Date(record.lastUsedAt).getTime();
  if (Number.isNaN(lastUsed)) {
    return [
      started,
      "Its lock file does not say when it last made a tool call: it is from an older Deskpass version, which may keep the computer until that session exits.",
      "Report this to the user instead of retrying: the user can close that AI session (the window or client started at that time) and then ask again.",
    ].join(" ");
  }
  const since = Math.max(0, now - lastUsed);
  const remaining = LOCK_IDLE_MS - since;
  return [
    started,
    `Its last tool call ended ${formatDuration(since)} ago.`,
    `A session that makes no tool call for ${formatDuration(LOCK_IDLE_MS)} releases the computer automatically, so retrying after that may work`,
    remaining > 0
      ? `(in about ${formatDuration(remaining)}, unless a long tool call of its own is still running).`
      : "(that session should release it shortly, unless a long tool call of its own is still running).",
    "Report this to the user instead of retrying right away: the user can wait and ask again, or close that AI session (the window or client started at that time).",
  ].join(" ");
}

/** A local date and time as `YYYY-MM-DD HH:MM:SS`. */
function localTime(date: Date): string {
  const two = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
}

/** A duration in words, to the second: "45 seconds", "1 minute 15 seconds", "2 hours 3 minutes". */
function formatDuration(ms: number): string {
  const total = Math.floor(ms / 1000);
  const unit = (count: number, name: string) => `${count} ${name}${count === 1 ? "" : "s"}`;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours > 0) return minutes > 0 ? `${unit(hours, "hour")} ${unit(minutes, "minute")}` : unit(hours, "hour");
  if (minutes > 0) return seconds > 0 ? `${unit(minutes, "minute")} ${unit(seconds, "second")}` : unit(minutes, "minute");
  return unit(seconds, "second");
}

export function defaultLockDir(): string {
  const local = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local");
  return path.join(local, "deskpass");
}

/** Reads the helper's `process_info` result: `{alive}`, plus `started_ms` or a `reason`. */
export function readProcessInfo(result: unknown): ProcessStart {
  const info = typeof result === "object" && result !== null ? (result as Record<string, unknown>) : {};
  if (info.alive === false) return { state: "missing" };
  if (info.alive !== true) return { state: "unknown", reason: "the helper did not say whether the process is running" };
  if (typeof info.started_ms === "number" && Number.isFinite(info.started_ms)) {
    return { state: "running", startedAt: new Date(info.started_ms) };
  }
  return { state: "unknown", reason: typeof info.reason === "string" && info.reason.length > 0 ? info.reason : "its start time is not available" };
}

export class SessionLock {
  private readonly file: string;
  private readonly probe: ProcessProbe;
  private readonly identityOverride: LockIdentity | undefined;
  private readonly now: () => number;
  private held = false;
  private identity: LockIdentity | null = null;

  constructor(lockDir: string, probe: ProcessProbe, identityOverride?: LockIdentity, options: SessionLockOptions = {}) {
    this.file = path.join(lockDir, "session.lock");
    this.probe = probe;
    this.identityOverride = identityOverride;
    this.now = options.now ?? Date.now;
  }

  /** Whether this session holds the lock, as of its last acquire, touch or release. */
  isHeld(): boolean {
    return this.held;
  }

  async acquire(): Promise<LockOutcome> {
    if (this.held) return { acquired: true };
    const identity = await this.ourIdentity();
    await mkdir(path.dirname(this.file), { recursive: true });
    for (let attempt = 0; attempt < 5; attempt += 1) {
      // A file held open for a moment by another program (an indexer, antivirus) is given time.
      if (attempt > 0) await new Promise((resolve) => setTimeout(resolve, 50 * attempt));
      try {
        await this.create(this.record(identity));
        this.held = true;
        return { acquired: true };
      } catch (error) {
        const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
        if (code !== "EEXIST") throw error;
        const holder = await this.holderState(identity);
        if (holder.state === "ours") {
          this.held = true;
          return { acquired: true };
        }
        if (holder.state === "live") return { acquired: false, message: inUseMessage(holder.record, this.now()) };
        if (holder.state === "unreadable") {
          return {
            acquired: false,
            message: [
              `The computer may be in use by another AI session: the lock file ${this.file} was changed moments ago and does not name its holder yet, so another session may be taking it.`,
              `Report this to the user instead of retrying. An unreadable lock file older than ${UNREADABLE_GRACE_MS / 1000} seconds is treated as a leftover and taken over.`,
            ].join(" "),
          };
        }
        if (holder.state === "unknown") {
          return {
            acquired: false,
            message: [
              `The computer may be in use by another AI session: the lock file ${this.file} names process ${holder.pid}, which is running, but its start time could not be read (${holder.reason}), so it cannot be confirmed to be a leftover.`,
              `Report this to the user instead of retrying. If no other AI session is operating the computer, the user can delete ${this.file}.`,
            ].join(" "),
          };
        }
        await this.removeStale(holder.raw);
      }
    }
    return { acquired: false, message: IN_USE_MESSAGE };
  }

  /**
   * Ticket 135: writes the end of a tool call into our lock file (`lastUsedAt`), so a session refused
   * with in_use can say how long this one has been idle. The new record is written to a file of its
   * own and renamed over the lock file, which replaces it in one step: a reader sees the old record
   * or the new one, never a partial file. Only a file that still names this session is replaced; one
   * that names another session, or is gone, means the lock was lost (someone deleted it, and maybe
   * another session took it) and is left alone. A failed write (the file held open for a moment by
   * another program) skips this update; the record still names this session.
   */
  async touch(): Promise<void> {
    if (!this.held || !this.identity) return;
    const identity = this.identity;
    let current: LockRecord | null;
    try {
      current = parseRecord(await readFile(this.file, "utf8"));
    } catch (error) {
      const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
      if (code === "ENOENT") this.held = false;
      return;
    }
    if (!current || current.pid !== identity.pid || current.startedAt !== identity.startedAt) {
      this.held = false;
      return;
    }
    const staging = `${this.file}.${process.pid}-${randomUUID()}.tmp`;
    try {
      await writeFile(staging, JSON.stringify(this.record(identity)), { flag: "wx" });
      await rename(staging, this.file);
    } catch {
      // Not updated this time.
    } finally {
      await rm(staging, { force: true });
    }
  }

  async release(): Promise<void> {
    if (!this.held || !this.identity) return;
    try {
      const record = parseRecord(await readFile(this.file, "utf8"));
      if (record && record.pid === this.identity.pid && record.startedAt === this.identity.startedAt) {
        await rm(this.file, { force: true });
      }
    } catch {
      // The file is already gone, or another session replaced it.
    }
    this.held = false;
  }

  /**
   * This process's start time as the helper reads it, so it matches what other sessions will
   * read for this pid. The helper can always read its own server; should it not, the time Node
   * started is close enough to fall back on.
   */
  private async ourIdentity(): Promise<LockIdentity> {
    if (this.identity) return this.identity;
    if (this.identityOverride) {
      this.identity = this.identityOverride;
      return this.identity;
    }
    const own = await this.probe(process.pid);
    const started = own.state === "running" ? own.startedAt : new Date(Math.round(performance.timeOrigin));
    this.identity = { pid: process.pid, startedAt: started.toISOString() };
    return this.identity;
  }

  /** Our record as of now: `lastUsedAt` is this moment (a lock just taken, or a tool call just ended). */
  private record(identity: LockIdentity): LockRecord {
    return { pid: identity.pid, startedAt: identity.startedAt, lastUsedAt: new Date(this.now()).toISOString() };
  }

  /**
   * Creates the lock file with its content already complete: the record is written to a file of
   * its own, then hard-linked to the lock name, which fails with EEXIST instead of replacing a
   * holder's file. Another session therefore never reads an empty or half-written lock from us.
   */
  private async create(record: LockRecord): Promise<void> {
    const staging = `${this.file}.${process.pid}-${randomUUID()}.tmp`;
    try {
      await writeFile(staging, JSON.stringify(record), { flag: "wx" });
      await link(staging, this.file);
    } finally {
      await rm(staging, { force: true });
    }
  }

  /**
   * Removes the lock file judged stale, but only if it still holds what was read (`raw`; null when
   * it was already gone). Between that read and the removal a faster session may have removed the
   * same stale file and created its own lock, so the file is first renamed to a name of our own and
   * compared there: a different content is that session's lock and is linked back. Should a third
   * session have created a lock in that short moment, the link fails and that session keeps it.
   */
  private async removeStale(raw: string | null): Promise<void> {
    if (raw === null) return;
    const aside = `${this.file}.${process.pid}-${randomUUID()}.stale`;
    try {
      await rename(this.file, aside);
    } catch {
      // Gone already, or held open for a moment by a reader: the next attempt looks again.
      return;
    }
    let moved: string | null = null;
    try {
      moved = await readFile(aside, "utf8");
    } catch {
      // Unreadable here means it cannot be shown to be the stale file; put it back.
    }
    if (moved !== raw) {
      try {
        await link(aside, this.file);
      } catch (error) {
        const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
        // EEXIST: another session created a lock meanwhile and keeps it. Anything else leaves the
        // newer holder's lock under the aside name, which is kept rather than deleted.
        if (code !== "EEXIST") {
          throw new Error(`The lock file ${this.file} of another session could not be put back from ${aside} (${code || String(error)}); report this to the user.`);
        }
      }
    }
    await rm(aside, { force: true });
  }

  private async holderState(
    ours: LockIdentity,
  ): Promise<{ state: "ours" } | { state: "live"; record: LockRecord } | { state: "unreadable" } | { state: "stale"; raw: string | null } | { state: "unknown"; pid: number; reason: string }> {
    let raw: string;
    try {
      raw = await readFile(this.file, "utf8");
    } catch {
      return { state: "stale", raw: null };
    }
    const record = parseRecord(raw);
    if (!record) return (await this.changedRecently()) ? { state: "unreadable" } : { state: "stale", raw };
    const actual = await this.probe(record.pid);
    if (actual.state === "missing") return { state: "stale", raw };
    if (actual.state === "unknown") return { state: "unknown", pid: record.pid, reason: actual.reason };
    if (!sameStart(actual.startedAt, new Date(record.startedAt))) return { state: "stale", raw };
    if (record.pid === ours.pid && sameStart(actual.startedAt, new Date(ours.startedAt))) return { state: "ours" };
    return { state: "live", record };
  }

  /** Whether the lock file was written within the grace period (so it may still be being written). */
  private async changedRecently(): Promise<boolean> {
    try {
      const { mtimeMs } = await stat(this.file);
      return Date.now() - mtimeMs < UNREADABLE_GRACE_MS;
    } catch {
      return false;
    }
  }
}

function parseRecord(raw: string): LockRecord | null {
  try {
    const parsed = JSON.parse(raw) as { pid?: unknown; startedAt?: unknown; lastUsedAt?: unknown };
    // A pid that no Windows process can have makes the file a leftover, not a question for the helper.
    if (typeof parsed.pid !== "number" || !Number.isInteger(parsed.pid) || parsed.pid <= 0 || parsed.pid > 0xffff_ffff) return null;
    if (typeof parsed.startedAt !== "string") return null;
    if (Number.isNaN(new Date(parsed.startedAt).getTime())) return null;
    // Ticket 135: lastUsedAt is optional (older versions do not write it); one that is not a time is ignored.
    const lastUsedAt = typeof parsed.lastUsedAt === "string" && !Number.isNaN(new Date(parsed.lastUsedAt).getTime()) ? parsed.lastUsedAt : undefined;
    return { pid: parsed.pid, startedAt: parsed.startedAt, ...(lastUsedAt === undefined ? {} : { lastUsedAt }) };
  } catch {
    return null;
  }
}

function sameStart(actual: Date, recorded: Date): boolean {
  return Math.abs(actual.getTime() - recorded.getTime()) <= START_TOLERANCE_MS;
}
