import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { isBlockedTier } from "../src/policy.ts";
import { resolveAppPath } from "../src/resolve-app.ts";
import { repoRoot } from "./harness.ts";

// Ticket 79 review F1: a launch path is checked under its real name, not an 8.3 alias of it.

/** The 8.3 short form of an existing path, or null where the volume keeps none. */
function shortPath(file: string): string | null {
  const command = `/d /c for %I in ("${file}") do @echo %~sI`;
  const out = execFileSync("cmd.exe", [command], { encoding: "utf8", windowsHide: true, windowsVerbatimArguments: true }).trim();
  return out.length > 0 && out.toLowerCase() !== file.toLowerCase() ? out : null;
}

test("an 8.3 alias of a high-risk executable resolves to its real name, so the high-risk check sees it", { skip: process.platform !== "win32" }, async (t) => {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const dir = await mkdtemp(join(root, "case-"));
  try {
    const file = join(dir, "powershell.exe");
    await writeFile(file, "");
    const alias = shortPath(file);
    if (!alias) {
      t.skip("this volume keeps no 8.3 names");
      return;
    }
    assert.ok(!isBlockedTier(alias, []), `the alias itself should not read as high-risk: ${alias}`);
    const resolved = await resolveAppPath(alias);
    assert.equal(resolved?.toLowerCase(), file.toLowerCase());
    assert.ok(isBlockedTier(resolved!, []), resolved!);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
