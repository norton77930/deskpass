import { randomUUID } from "node:crypto";
import { link, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";

/**
 * Ticket 112: the settings lock, `<data folder>\settings.lock`, which says that a settings server is
 * running and how to reach it (spec-deskpass, settings server: only one at a time):
 *
 *     { "pid": 1234, "port": 51234, "secret": "<43 base64url characters>", "started_at": "<ISO time>" }
 *
 * `secret` is the launch secret (32 random bytes): a second `deskpass settings` presents it to the
 * running server to get a new one-time token. Only the current user can read it because of where it
 * is: the data folder is under `%LOCALAPPDATA%`, whose ACL (inherited by the files made in it) grants
 * access to the user, SYSTEM and Administrators only, as for the session lock and grants.json beside
 * it. Node cannot set a Windows ACL itself (a file mode only toggles read-only), so nothing here
 * widens or narrows it. Unlike grants.key (DPAPI-protected, since it signs what lasts), the secret
 * dies with its server, and anyone who can read it as this user can also read the browser's cookie
 * store or the folder itself: it keeps out other users and web pages, not the user's own programs.
 *
 * It is not the MCP server's session lock (`session.lock`, src/lock.ts) and never touches it.
 */

export const SETTINGS_LOCK_FILE = "settings.lock";

export interface SettingsLockRecord {
  pid: number;
  port: number;
  secret: string;
  started_at: string;
}

const SECRET_PATTERN = /^[A-Za-z0-9_-]{43}$/;

export function settingsLockPath(dir: string): string {
  return path.join(dir, SETTINGS_LOCK_FILE);
}

/** The lock's text and record (null when it is not one), or null when there is no lock file. */
export async function readSettingsLock(dir: string): Promise<{ raw: string; record: SettingsLockRecord | null } | null> {
  let raw: string;
  try {
    raw = await readFile(settingsLockPath(dir), "utf8");
  } catch (error) {
    if ((error as { code?: unknown }).code === "ENOENT") return null;
    // Unreadable now (held open for a moment, no permission): treated as a lock naming no server.
    return { raw: "", record: null };
  }
  return { raw, record: parseRecord(raw) };
}

/**
 * Creates the lock holding `record`, complete from the first moment it exists: written to a file of
 * its own, then hard-linked to the lock's name, which fails instead of replacing another server's
 * lock. False when there is one already.
 */
export async function createSettingsLock(dir: string, record: SettingsLockRecord): Promise<boolean> {
  await mkdir(dir, { recursive: true });
  const file = settingsLockPath(dir);
  const staging = `${file}.${process.pid}-${randomUUID()}.tmp`;
  try {
    await writeFile(staging, JSON.stringify(record), { flag: "wx" });
    await link(staging, file);
    return true;
  } catch (error) {
    if ((error as { code?: unknown }).code === "EEXIST") return false;
    throw error;
  } finally {
    await rm(staging, { force: true });
  }
}

/**
 * Removes the lock, but only while it still holds `raw` (what was read, or what this server wrote):
 * it is first renamed to a name of its own and compared there, and a lock another server made
 * meanwhile is linked back instead of deleted.
 */
export async function removeSettingsLock(dir: string, raw: string): Promise<void> {
  const file = settingsLockPath(dir);
  const aside = `${file}.${process.pid}-${randomUUID()}.stale`;
  try {
    await rename(file, aside);
  } catch {
    // Gone already, or held open for a moment: nothing to remove now.
    return;
  }
  let moved: string | null = null;
  try {
    moved = await readFile(aside, "utf8");
  } catch {
    // Unreadable here means it cannot be shown to be the lock that was read; it is put back.
  }
  if (moved !== raw) {
    try {
      await link(aside, file);
    } catch {
      // Another lock was made meanwhile (EEXIST), which keeps its place; anything else leaves the
      // newer lock under the aside name, kept rather than deleted.
      return;
    }
  }
  await rm(aside, { force: true });
}

function parseRecord(raw: string): SettingsLockRecord | null {
  try {
    const value = JSON.parse(raw) as Record<string, unknown>;
    const { pid, port, secret, started_at: startedAt } = value;
    if (typeof pid !== "number" || !Number.isInteger(pid) || pid <= 0) return null;
    if (typeof port !== "number" || !Number.isInteger(port) || port <= 0 || port > 65_535) return null;
    if (typeof secret !== "string" || !SECRET_PATTERN.test(secret)) return null;
    if (typeof startedAt !== "string") return null;
    return { pid, port, secret, started_at: startedAt };
  } catch {
    return null;
  }
}
