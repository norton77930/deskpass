import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { test } from "node:test";
import { repoRoot } from "./harness.ts";

// Ticket 118: the native control panel is gone; `deskpass panel` only points to the settings page.
// Open-source follow-up: the message follows the UI language (DESKPASS_LANG pinned per case).

function runPanel(lang: "en" | "zh-TW") {
  return spawnSync(process.execPath, ["--experimental-strip-types", join(repoRoot(), "src", "index.ts"), "panel"], {
    encoding: "utf8",
    env: { ...process.env, DESKPASS_LANG: lang },
    stdio: ["ignore", "pipe", "pipe"],
    timeout: 30_000,
    windowsHide: true,
  });
}

const withoutNodeWarning = (stderr: string) => stderr.replace(/^\(node:\d+\) ExperimentalWarning.*\r?\n(\(Use `node --trace-warnings.*\r?\n)?/m, "");

for (const [lang, message] of [
  ["zh-TW", "控制台已改成設定網頁，請用 deskpass settings"],
  ["en", "The control panel is now the settings page: run deskpass settings"],
] as const) {
  test(`\`deskpass panel\` says the control panel is now the settings page (${lang}) and ends, starting nothing`, () => {
    const run = runPanel(lang);
    assert.equal(run.error, undefined);
    assert.equal(run.status, 0, run.stderr);
    assert.equal(run.stdout, "");
    assert.equal(withoutNodeWarning(run.stderr), `${message}\n`);
  });
}
