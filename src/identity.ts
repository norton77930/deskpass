import { stat } from "node:fs/promises";
import path from "node:path";
import { appKey } from "./policy.ts";

/**
 * Ticket 93: who an app is, as the helper's `app_identity` reports it (spec-phase4b, app identity and
 * always allow). `signer` is the leaf certificate's full subject of a verified Authenticode signature;
 * a packaged app has `package_family_name` instead (its package is Windows' to verify).
 */
export interface AppIdentity {
  signed: boolean;
  signer: string | null;
  /** Lower case. */
  file_name: string;
  /**
   * The `OriginalFilename` of the executable's own version resource, lower case, without `.mui`
   * (`cmd.exe` for any copy of cmd.exe, whatever it is called); null when it has none. The
   * signature covers it, as it never covers the file name.
   */
  original_name: string | null;
  package_family_name?: string;
}

/** A well-formed `app_identity` result, or null (then the app is treated as unsigned). */
export function readAppIdentity(result: unknown): AppIdentity | null {
  if (typeof result !== "object" || result === null) return null;
  const { signed, signer, file_name: fileName, original_name: original, package_family_name: family } = result as Record<string, unknown>;
  if (typeof signed !== "boolean" || typeof fileName !== "string" || fileName.length === 0) return null;
  if (signed && (typeof signer !== "string" || signer.length === 0)) return null;
  if (family !== undefined && (typeof family !== "string" || family.length === 0)) return null;
  if (original !== undefined && original !== null && (typeof original !== "string" || original.length === 0)) return null;
  return {
    signed,
    signer: signed ? (signer as string) : null,
    file_name: fileName.toLowerCase(),
    original_name: typeof original === "string" ? original.toLowerCase() : null,
    ...(family === undefined ? {} : { package_family_name: family }),
  };
}

/** A file name without `.exe`, lower case: how a file name and an original name are compared. */
function programStem(name: string): string {
  return name.toLowerCase().replace(/\.exe$/, "");
}

/** The original name every Electron app has (`isOriginalName`), as `programStem` writes it. */
const ELECTRON_STEM = "electron";

/**
 * Whether the signed file `fileName` is the program its version resource names (`original_name`).
 * A signature covers a file's content, never its name: a copy of cmd.exe saved as notepad.exe
 * verifies as Microsoft's, and would otherwise be known as Notepad (and use its always-allow).
 *
 * An original name of `electron` counts for any file name: every Electron app (VS Code's Code.exe,
 * for one) keeps the name of the Electron binary it was built from, so the rule would take
 * always-allow from all of them. What this leaves open is a copy of one signer's Electron app saved
 * under the name of another app of that signer. The tiers still look at both names.
 */
export function isOriginalName(fileName: string, originalName: string | null): boolean {
  if (originalName === null || originalName.length === 0) return false;
  const original = programStem(originalName);
  return original === programStem(fileName) || original === ELECTRON_STEM;
}

/**
 * The signer's key a signed app that is not known by its signer (`isOriginalName`) would have, which
 * it did have before the original-name rule: a raise the user made under it still applies (a raise
 * can only make an app stricter), and the settings page lists it under that key. Null for any other
 * app, whose key already is the whole answer. A grant under it does not apply: that is what the rule
 * is for.
 */
export function priorSignerKey(identity: AppIdentity | null): string | null {
  if (!identity?.signed || !identity.signer || identity.package_family_name) return null;
  return isOriginalName(identity.file_name, identity.original_name) ? null : `signer:${identity.signer}|${identity.file_name}`;
}

/**
 * Ticket 93: the key an app is known by in the log, and (tickets 94, 95) in its grants and this
 * session's approvals:
 * - a packaged app: `package:` and its package family name, lower case;
 * - a signed app whose file name is its original name (`isOriginalName`): `signer:`, the signer's
 *   subject as the helper reports it, `|` and the file name in lower case (a file name cannot hold
 *   `|`, so the last `|` splits the two);
 * - anything else, including an app whose identity could not be had: its normalized path
 *   (`appKey`), as the log had it before this ticket. A path begins with a drive or `\\`, so it
 *   never reads as one of the other two.
 */
export function identityKey(appPath: string, identity: AppIdentity | null): string {
  if (identity?.package_family_name) return `package:${identity.package_family_name.toLowerCase()}`;
  // Only a file that is the program its version resource names; a renamed copy is known by its path.
  if (identity?.signed && identity.signer && isOriginalName(identity.file_name, identity.original_name)) return `signer:${identity.signer}|${identity.file_name}`;
  return appKey(appPath);
}

/**
 * Ticket 93: the identities this server has had from the helper, by normalized path, modification
 * time and size, so each approval check has one without verifying the signature again until the
 * executable changes. Only a well-formed answer for a file whose modification time and size were
 * read (before and after asking, unchanged) is kept; a failure is asked again next time.
 *
 * Ticket 95: the identity kept for a path does not depend on which check asked first. A window's
 * check sends the process (`pid`), from which the helper can read a package identity the path
 * alone may not show (a packaged app outside `WindowsApps`); a launch has no process yet. An answer
 * got with a process replaces one got without, and a check without a process uses whichever is
 * kept, so once a window of the executable was checked, its launches are keyed (and granted) as
 * its windows are. Only a launch before any window check of a packaged app outside `WindowsApps`
 * can still get the other key, and then its grant is not found and the user is asked.
 */
export class IdentityCache {
  private readonly entries = new Map<string, { version: string; identity: AppIdentity; withPid: boolean }>();

  /** The identity of `appPath` (run by `pid`, when a window's), or null when it cannot be had. */
  async of(appPath: string, pid: number | undefined, ask: (params: Record<string, unknown>) => Promise<unknown>): Promise<AppIdentity | null> {
    // The helper takes Windows paths only, so a path written with `/` is sent with `\`.
    const windowsPath = appPath.replaceAll("/", "\\");
    if (!path.win32.isAbsolute(windowsPath) || !/^([a-z]:\\|\\\\)/i.test(windowsPath)) return null;
    const key = appKey(windowsPath);
    const before = await fileVersion(windowsPath);
    const cached = this.entries.get(key);
    if (cached && before !== null && cached.version === before && (cached.withPid || pid === undefined)) return cached.identity;
    let identity: AppIdentity | null;
    try {
      identity = readAppIdentity(await ask(pid === undefined ? { path: windowsPath } : { path: windowsPath, pid }));
    } catch {
      // A failed or timed-out check is no identity: the app is treated as unsigned.
      return null;
    }
    if (identity && before !== null && (await fileVersion(windowsPath)) === before) this.entries.set(key, { version: before, identity, withPid: pid !== undefined });
    return identity;
  }
}

/** The file's modification time and size, as one comparable string; null when it cannot be read. */
async function fileVersion(appPath: string): Promise<string | null> {
  try {
    const { mtimeMs, size } = await stat(appPath);
    return `${mtimeMs}:${size}`;
  } catch {
    return null;
  }
}
