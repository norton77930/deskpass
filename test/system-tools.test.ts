import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { resolveAppPath } from "../src/resolve-app.ts";
import { systemTool } from "../src/system-tools.ts";

test("a Windows tool is named by its full System32 path, never by its bare name", () => {
  assert.equal(systemTool("reg.exe", { SystemRoot: "D:\\WINNT" }), "D:\\WINNT\\System32\\reg.exe");
  assert.equal(systemTool("rundll32.exe", { windir: "C:\\Windows" }), "C:\\Windows\\System32\\rundll32.exe");
  // A Windows folder that is not a drive path is not taken: the tool would be looked up relative to it.
  for (const env of [{}, { SystemRoot: "" }, { SystemRoot: "Windows" }, { SystemRoot: ".\\evil" }, { SystemRoot: "\\\\server\\share" }]) {
    assert.equal(systemTool("reg.exe", env), "C:\\Windows\\System32\\reg.exe", JSON.stringify(env));
  }
});

test("resolveAppPath never finds an app in the server's current directory", async () => {
  const dir = await mkdtemp(join(tmpdir(), "deskpass-cwd-"));
  const before = process.cwd();
  try {
    await writeFile(join(dir, "deskpass-planted.exe"), "");
    process.chdir(dir);
    assert.equal(await resolveAppPath("deskpass-planted.exe"), null, "a bare name is only looked up in App Paths");
    assert.equal(await resolveAppPath("deskpass-planted"), null);
    assert.equal(await resolveAppPath(".\\deskpass-planted.exe"), null, "a relative path is not looked up");
    assert.equal(await resolveAppPath("./deskpass-planted.exe"), null);
  } finally {
    process.chdir(before);
    await rm(dir, { recursive: true, force: true });
  }
});
