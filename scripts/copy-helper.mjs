// Puts the freshly built helper next to the server in dist/.
//
// A server that is still running keeps dist/deskpass-helper.exe locked, so overwriting it
// fails. Windows does allow renaming a running image, so a locked destination is renamed aside
// and the new file copied in its place. The build never reports success with a stale helper:
// when neither works it exits non-zero.
//
// The id of the helper being installed is recorded in dist/helper-build.json first. The server
// compares it with what the helper it starts reports, so even a replacement that went wrong
// somewhere is caught when it runs, not only at build time.
import { execFileSync } from "node:child_process";
import { realpathSync } from "node:fs";
import * as nodeFs from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// EPERM and EACCES are what a locked image can report, but also what a read-only file reports,
// so the messages say "locked or not writable".
const LOCK_CODES = new Set(["EBUSY", "EPERM", "EACCES"]);

/** @typedef {Pick<typeof nodeFs, "copyFile" | "mkdir" | "readdir" | "rename" | "unlink">} HelperFs */

function describe(error) {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Replaces `destination` with `source`, renaming a locked `destination` aside first. Renamed
 * files left by earlier runs are deleted when they can be; those still in use are skipped.
 *
 * @param {{ source: string, destination: string, fs?: HelperFs, now?: () => number }} options
 * @returns {Promise<{ movedAside: string | null }>}
 */
export async function replaceHelper({ source, destination, fs = nodeFs, now = Date.now }) {
  const dir = dirname(destination);
  const stem = basename(destination).replace(/\.exe$/i, "");
  await fs.mkdir(dir, { recursive: true });

  const leftovers = (await fs.readdir(dir)).filter((name) => name.startsWith(`${stem}.old-`) && name.toLowerCase().endsWith(".exe"));
  for (const name of leftovers) {
    try {
      await fs.unlink(join(dir, name));
    } catch {
      // Still running, or otherwise held: the next build tries again.
    }
  }

  try {
    await fs.copyFile(source, destination);
    return { movedAside: null };
  } catch (copyError) {
    if (!LOCK_CODES.has(copyError?.code)) {
      throw new Error(`Could not replace ${destination}: ${describe(copyError)}`);
    }
    const movedAside = join(dir, `${stem}.old-${now()}.exe`);
    try {
      await fs.rename(destination, movedAside);
    } catch (renameError) {
      throw new Error(
        `Could not replace ${destination}: it is locked or not writable (${describe(copyError)}) and could not be renamed aside (${describe(renameError)}). ` +
          "Stop the Deskpass server that is running it and build again.",
      );
    }
    try {
      await fs.copyFile(source, destination);
    } catch (secondError) {
      let restored = "the old helper was put back";
      try {
        await fs.rename(movedAside, destination);
      } catch (restoreError) {
        restored = `the old helper is left at ${movedAside} (${describe(restoreError)})`;
      }
      throw new Error(`Could not replace ${destination} even after renaming it aside: ${describe(secondError)}; ${restored}.`);
    }
    return { movedAside };
  }
}

/**
 * Records the build id of the helper at `source` in `buildFile`, then replaces `destination`.
 *
 * @param {{ source: string, destination: string, buildFile: string, readBuildId: (source: string) => string, fs?: HelperFs, now?: () => number }} options
 */
export async function installHelper({ source, destination, buildFile, readBuildId, fs = nodeFs, now }) {
  const build = readBuildId(source);
  await fs.mkdir(dirname(destination), { recursive: true });
  await nodeFs.writeFile(buildFile, `${JSON.stringify({ build })}\n`);
  return { build, ...(await replaceHelper({ source, destination, fs, now })) };
}

function readBuildIdFromHelper(source) {
  const id = execFileSync(source, ["--build-id"], { encoding: "utf8", windowsHide: true, timeout: 10_000 }).trim();
  if (!/^[0-9a-f]{16}$/.test(id)) throw new Error(`${source} --build-id printed ${JSON.stringify(id)}, not a build id.`);
  return id;
}

/**
 * Whether this file was started as a command (`node scripts/copy-helper.mjs`), not imported.
 * Both paths are resolved through links first: started through a junction, or with the drive
 * letter in another case, they differ as text but name the same file. Getting this wrong skips
 * the copy and still exits 0.
 */
function runAsCommand() {
  if (!process.argv[1]) return false;
  const canonical = (path) => {
    const real = realpathSync(path);
    return process.platform === "win32" ? real.toLowerCase() : real;
  };
  try {
    return canonical(fileURLToPath(import.meta.url)) === canonical(resolve(process.argv[1]));
  } catch {
    return false;
  }
}

if (runAsCommand()) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const source = join(root, "helper", "target", "release", "deskpass-helper.exe");
  const destination = join(root, "dist", "deskpass-helper.exe");
  try {
    const result = await installHelper({
      source,
      destination,
      buildFile: join(root, "dist", "helper-build.json"),
      readBuildId: readBuildIdFromHelper,
    });
    if (result.movedAside) console.log(`${destination} was in use; renamed it to ${result.movedAside}`);
    console.log(`copied ${source} to ${destination} (build ${result.build})`);
  } catch (error) {
    console.error(describe(error));
    process.exitCode = 1;
  }
}
