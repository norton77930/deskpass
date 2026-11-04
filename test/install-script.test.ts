import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { delimiter, dirname, join } from "node:path";
import { test } from "node:test";
import { repoRoot } from "./harness.ts";

// The messages are pinned to Traditional Chinese with DESKPASS_LANG unless a test asks for English (ticket 04).
// Ticket 134. install.ps1 runs here with -SkipShellIntegration and LOCALAPPDATA pointing at a
// temporary folder: nothing outside that folder is touched (no MCP registration, no shortcut).
// Ticket 11 tests run the whole script (no -SkipShellIntegration) with LOCALAPPDATA and APPDATA in a temporary
// folder, a PATH that has either no claude or a fake claude.cmd, and a Set-Clipboard function defined before the
// script runs (it shadows the cmdlet), so neither the real Claude Code config nor the real clipboard is touched.

async function workDir(): Promise<string> {
  await mkdir(join(repoRoot(), "test-tmp"), { recursive: true });
  return mkdtemp(join(repoRoot(), "test-tmp", "install-script-"));
}

async function put(root: string, files: Record<string, string>): Promise<void> {
  for (const [file, content] of Object.entries(files)) {
    await mkdir(dirname(join(root, file)), { recursive: true });
    await writeFile(join(root, file), content);
  }
}

/** An unzipped package as install.ps1 expects it (no node_modules). */
async function makePackage(root: string, version: string): Promise<string> {
  const dir = join(root, `package-${version}`);
  await put(dir, {
    "dist/index.js": `server ${version}`,
    "dist/deskpass-helper.exe": "helper",
    "dist/helper-build.json": "{}",
    "package.json": "{}",
    "README.md": "readme",
    "deskpass-settings.vbs": "vbs",
    "uninstall.ps1": "uninstall",
    VERSION: `${version}\n`,
  });
  await writeFile(join(dir, "install.ps1"), await readFile(join(repoRoot(), "packaging", "install.ps1")));
  return dir;
}

function install(packageDir: string, localAppData: string, lang = "zh-TW"): { code: number | null; output: string } {
  const script = join(packageDir, "install.ps1");
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-Command",
      `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; & '${script}' -SkipShellIntegration; exit $LASTEXITCODE`,
    ],
    { encoding: "utf8", env: { ...process.env, LOCALAPPDATA: localAppData, DESKPASS_LANG: lang }, timeout: 120_000 },
  );
  return { code: result.status, output: `${result.stdout}${result.stderr}` };
}

test("a fresh install copies the few files, with no node_modules", async () => {
  const dir = await workDir();
  try {
    const localAppData = join(dir, "local");
    await mkdir(localAppData);
    const result = install(await makePackage(dir, "1.2.3"), localAppData);
    assert.equal(result.code, 0, result.output);
    assert.match(result.output, /安裝 Deskpass 1\.2\.3/);
    const programDir = join(localAppData, "Programs", "Deskpass");
    assert.equal(await readFile(join(programDir, "dist", "index.js"), "utf8"), "server 1.2.3");
    assert.equal(existsSync(join(programDir, "node_modules")), false);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("installing the same version over an old install with node_modules says reinstall and removes node_modules and node_modules.old-*", async () => {
  const dir = await workDir();
  try {
    const localAppData = join(dir, "local");
    const programDir = join(localAppData, "Programs", "Deskpass");
    await put(programDir, {
      VERSION: "1.2.3\n",
      "dist/index.js": "old server",
      "dist/panel.js": "a file the new version does not ship",
      "node_modules/zod/package.json": "{}",
      "node_modules.old-1/zod/package.json": "{}",
      "node_modules.old-2/x/y.js": "",
    });
    const result = install(await makePackage(dir, "1.2.3"), localAppData);
    assert.equal(result.code, 0, result.output);
    assert.match(result.output, /重新安裝 Deskpass 1\.2\.3\(覆蓋現有的\)/);
    assert.doesNotMatch(result.output, /升級 Deskpass/);
    assert.equal(await readFile(join(programDir, "dist", "index.js"), "utf8"), "server 1.2.3");
    assert.equal(existsSync(join(programDir, "dist", "panel.js")), false);
    for (const name of ["node_modules", "node_modules.old-1", "node_modules.old-2"]) {
      assert.equal(existsSync(join(programDir, name)), false, name);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("installing a different version says upgrade from the old to the new", async () => {
  const dir = await workDir();
  try {
    const localAppData = join(dir, "local");
    const programDir = join(localAppData, "Programs", "Deskpass");
    await put(programDir, { VERSION: "1.0.0\n", "dist/index.js": "old server" });
    const result = install(await makePackage(dir, "1.1.0"), localAppData);
    assert.equal(result.code, 0, result.output);
    assert.match(result.output, /升級 Deskpass 1\.0\.0 → 1\.1\.0/);
    assert.doesNotMatch(result.output, /重新安裝/);
    assert.equal((await readFile(join(programDir, "VERSION"), "utf8")).trim(), "1.1.0");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("with DESKPASS_LANG=en the messages are English with no CJK", async () => {
  const dir = await workDir();
  try {
    const localAppData = join(dir, "local");
    await mkdir(localAppData);
    const result = install(await makePackage(dir, "1.2.3"), localAppData, "en");
    assert.equal(result.code, 0, result.output);
    assert.match(result.output, /Installing Deskpass 1\.2\.3/);
    assert.doesNotMatch(result.output, /[\u3000-\u9fff\uff00-\uffef]/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

// -SkipShellIntegration ends the script before its closing message, so that wording is checked in the source.
test("the closing message says reinstall only for the same version", async () => {
  const script = await readFile(join(repoRoot(), "packaging", "install.ps1"), "utf8");
  assert.match(script, /if \(\$reinstall\) \{\s*Write-Host \(Msg "doneReinstall"\)/);
  assert.match(script, /elseif \(\$upgrade\) \{\s*Write-Host \(Msg "doneUpgrade"\)/);
  assert.match(script, /doneReinstall\s*= "重新安裝完成。請重新啟動你的 AI client/);
  assert.match(script, /doneUpgrade\s*= "升級完成。/);
  assert.match(script, /if \(\$oldVersion -eq \$version\)/);
});

// ---- Ticket 11: MCP clients other than Claude Code ------------------------------------------------------------

/** PATH without any directory that holds a claude command. */
function pathWithoutClaude(): string {
  const entries = (process.env.PATH ?? "").split(delimiter).filter((entry) => {
    if (entry === "") return false;
    return !["claude.exe", "claude.cmd", "claude.bat", "claude.ps1", "claude"].some((name) => existsSync(join(entry, name)));
  });
  return entries.join(delimiter);
}

interface FullInstall {
  code: number | null;
  output: string;
  clipboard: string | undefined;
  claudeLog: string | undefined;
  localAppData: string;
}

/**
 * Runs the whole install.ps1 (shortcut and MCP registration included) inside `dir`. `claude` is "none" (not on PATH)
 * or "fake" (a claude.cmd that logs its arguments and exits 0). `clipboard` is "stub" (Set-Clipboard writes to a
 * file) or "broken" (Set-Clipboard throws).
 */
async function installFull(
  dir: string,
  version: string,
  lang: string,
  claude: "none" | "fake",
  clipboard: "stub" | "broken" = "stub",
): Promise<FullInstall> {
  const localAppData = join(dir, "local");
  const appData = join(dir, "roaming");
  await mkdir(localAppData, { recursive: true });
  await mkdir(appData, { recursive: true });
  const claudeLogFile = join(dir, "claude-calls.txt");
  const clipboardFile = join(dir, "clipboard.txt");
  let path = pathWithoutClaude();
  if (claude === "fake") {
    const binDir = join(dir, "fake-bin");
    await put(binDir, { "claude.cmd": `@echo off\r\necho %*>> "${claudeLogFile}"\r\nexit /b 0\r\n` });
    path = `${binDir}${delimiter}${path}`;
  }
  const setClipboard =
    clipboard === "stub"
      ? `function Set-Clipboard { param([Parameter(ValueFromPipeline=$true)]$Value) Set-Content -LiteralPath '${clipboardFile}' -Value $Value -Encoding UTF8 }`
      : `function Set-Clipboard { param($Value) throw 'clipboard is not available here' }`;
  const script = join(await makePackage(dir, version), "install.ps1");
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-Command",
      `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; ${setClipboard}; & '${script}'; exit $LASTEXITCODE`,
    ],
    {
      encoding: "utf8",
      env: { ...process.env, LOCALAPPDATA: localAppData, APPDATA: appData, DESKPASS_LANG: lang, PATH: path },
      timeout: 120_000,
    },
  );
  const readIf = async (file: string) => (existsSync(file) ? (await readFile(file, "utf8")).replace(/^﻿/, "") : undefined);
  return {
    code: result.status,
    output: `${result.stdout}${result.stderr}`,
    clipboard: await readIf(clipboardFile),
    claudeLog: await readIf(claudeLogFile),
    localAppData,
  };
}

function expectedDefinition(localAppData: string): unknown {
  return {
    mcpServers: { deskpass: { command: "node", args: [join(localAppData, "Programs", "Deskpass", "dist", "index.js")] } },
  };
}

test("without claude on PATH the install completes, prints the MCP server definition and copies it to the clipboard", async () => {
  const dir = await workDir();
  try {
    const result = await installFull(dir, "1.2.3", "en", "none");
    assert.equal(result.code, 0, result.output);
    assert.ok(result.clipboard !== undefined, `Set-Clipboard was not called\n${result.output}`);
    assert.deepEqual(JSON.parse(result.clipboard), expectedDefinition(result.localAppData));
    assert.ok(result.output.includes(result.clipboard.trim()), `the printed definition differs from the clipboard\n${result.output}`);
    assert.match(result.output, /Copied the definition above to the clipboard/);
    assert.match(result.output, /CodeBuddy, Cursor, Claude Desktop, Codex CLI/);
    // Claude Code users still get their one-liner, worded as an option rather than an error.
    assert.match(result.output, /claude mcp add --scope user --transport stdio deskpass -- node "/);
    assert.match(result.output, /Install complete\. Restart your AI client/);
    assert.doesNotMatch(result.output, /[　-鿿＀-￯]/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("with claude on PATH the install still registers it at user scope and also prints the definition", async () => {
  const dir = await workDir();
  try {
    const result = await installFull(dir, "1.2.3", "en", "fake");
    assert.equal(result.code, 0, result.output);
    const serverPath = join(result.localAppData, "Programs", "Deskpass", "dist", "index.js");
    assert.ok(result.claudeLog !== undefined, "the fake claude was never called");
    const calls = result.claudeLog.split(/\r?\n/).filter((line) => line.length > 0);
    assert.equal(calls[0], "mcp remove --scope user deskpass");
    // The path is quoted when it holds a space (this repository's folder does).
    assert.equal(calls[1], `mcp add --scope user --transport stdio deskpass -- node ${/\s/.test(serverPath) ? `"${serverPath}"` : serverPath}`);
    assert.match(result.output, /Registered the MCP server 'deskpass' in Claude Code/);
    assert.ok(result.clipboard !== undefined, result.output);
    assert.deepEqual(JSON.parse(result.clipboard), expectedDefinition(result.localAppData));
    assert.ok(result.output.includes(result.clipboard.trim()), result.output);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("a clipboard that cannot be written only warns: the install still succeeds and the definition is printed", async () => {
  const dir = await workDir();
  try {
    const result = await installFull(dir, "1.2.3", "en", "none", "broken");
    assert.equal(result.code, 0, result.output);
    assert.match(result.output, /Could not copy to the clipboard/);
    assert.match(result.output, /"deskpass": \{/);
    assert.match(result.output, /Install complete\./);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("zh-TW runs print the same definition and tell the user to restart their AI client", async () => {
  const dir = await workDir();
  try {
    const result = await installFull(dir, "1.2.3", "zh-TW", "none");
    assert.equal(result.code, 0, result.output);
    assert.ok(result.clipboard !== undefined, result.output);
    assert.deepEqual(JSON.parse(result.clipboard), expectedDefinition(result.localAppData));
    assert.ok(result.output.includes(result.clipboard.trim()), result.output);
    assert.match(result.output, /已複製到剪貼簿/);
    assert.match(result.output, /安裝完成。請重新啟動你的 AI client/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

/** The message tables of a packaging script: language -> key -> value (the text between the quotes, as written). */
function messageTables(script: string): Record<"en" | "zh-TW", Record<string, string>> {
  const zhStart = script.indexOf('"zh-TW" = @{');
  const enStart = script.indexOf('"en" = @{');
  assert.ok(enStart >= 0 && zhStart > enStart, "message tables not found");
  const parse = (block: string) => {
    const table: Record<string, string> = {};
    for (const match of block.matchAll(/^\s+(\w+)\s*=\s*"(.*)"\s*$/gm)) table[match[1]!] = match[2]!;
    return table;
  };
  return { en: parse(script.slice(enStart, zhStart)), "zh-TW": parse(script.slice(zhStart)) };
}

for (const file of ["install.ps1", "uninstall.ps1"]) {
  test(`${file}: both message tables have the same keys, and the English one has no CJK`, async () => {
    const tables = messageTables(await readFile(join(repoRoot(), "packaging", file), "utf8"));
    assert.ok(Object.keys(tables.en).length > 10);
    assert.deepEqual(Object.keys(tables["zh-TW"]).sort(), Object.keys(tables.en).sort());
    for (const [key, value] of Object.entries(tables.en)) {
      assert.doesNotMatch(value, /[　-鿿＀-￯]/, key);
    }
  });

  test(`${file}: messages that tell the user to close or restart name the AI client, not only Claude Code`, async () => {
    const tables = messageTables(await readFile(join(repoRoot(), "packaging", file), "utf8"));
    // Only the messages about registering or de-registering with Claude Code itself may name just Claude Code.
    const claudeCodeOnly = new Set(["mcpRegistered", "mcpNoClaude", "mcpRemoved", "mcpNotThere"]);
    for (const language of ["en", "zh-TW"] as const) {
      for (const [key, value] of Object.entries(tables[language])) {
        if (claudeCodeOnly.has(key) || !value.includes("Claude Code")) continue;
        assert.match(value, /AI client/, `${file} ${language} ${key}: ${value}`);
      }
    }
  });
}

test("the install, upgrade and reinstall completion messages tell the user to restart their AI client in both languages", async () => {
  const tables = messageTables(await readFile(join(repoRoot(), "packaging", "install.ps1"), "utf8"));
  for (const language of ["en", "zh-TW"] as const) {
    for (const key of ["doneInstall", "doneUpgrade", "doneReinstall"]) {
      assert.match(tables[language][key]!, /AI client/, `${language} ${key}`);
      assert.match(tables[language][key]!, /CodeBuddy/, `${language} ${key}`);
    }
  }
});

test("packaging files hold no CJK outside the zh-TW message table and the zh-TW shortcut name", async () => {
  for (const file of ["install.ps1", "uninstall.ps1", "deskpass-settings.vbs"]) {
    const script = await readFile(join(repoRoot(), "packaging", file), "utf8");
    const zhStart = script.indexOf('"zh-TW" = @{');
    const zhEnd = zhStart < 0 ? -1 : script.indexOf("\n  }\n", zhStart);
    const rest = (zhStart < 0 ? script : script.slice(0, zhStart) + script.slice(zhEnd)).replaceAll("Deskpass 設定", "Deskpass");
    // Any non-ASCII character other than the BOM would be a leftover from another language.
    const stray = [...rest].filter((ch) => ch.charCodeAt(0) > 0x7f && ch !== "﻿");
    assert.deepEqual(stray, [], `${file} has non-ASCII text outside the zh-TW table`);
  }
});
