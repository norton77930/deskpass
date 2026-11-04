import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

/** Written next to the helper exe by `scripts/copy-helper.mjs` when the build installs it. */
export const HELPER_BUILD_FILE = "helper-build.json";

/**
 * The build id the helper next to `helperPath` is expected to report, or undefined when none is
 * recorded (running from source, a test build, a fake helper). Then only the protocol version is
 * checked. A file that exists but cannot be read as a build record also skips the check, and says
 * so on one line to `log`, because the check is then silently off.
 */
export function readExpectedBuild(helperPath: string, log: (line: string) => void = (line) => console.error(line)): string | undefined {
  const file = join(dirname(helperPath), HELPER_BUILD_FILE);
  let text: string;
  try {
    text = readFileSync(file, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") log(`deskpass: could not read ${file}; the helper build is not checked (${oneLine(error)}).`);
    return undefined;
  }
  try {
    const parsed = JSON.parse(text) as { build?: unknown };
    return typeof parsed.build === "string" && parsed.build !== "" ? parsed.build : undefined;
  } catch (error) {
    log(`deskpass: ${file} is not valid JSON; the helper build is not checked (${oneLine(error)}).`);
    return undefined;
  }
}

/** What to do about a helper whose build is not the one recorded beside it (the server's check). */
export function buildMismatchMessage(build: string, expected: string): string {
  return (
    `Helper build "${build || "unknown"}" was not built together with this server (expected build "${expected}"). ` +
    "An old helper is probably still on disk or running: stop every Deskpass server, rebuild with npm run build, and restart."
  );
}

function oneLine(error: unknown): string {
  return (error instanceof Error ? error.message : String(error)).replace(/\s+/g, " ");
}
