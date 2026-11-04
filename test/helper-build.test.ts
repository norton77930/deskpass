import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { launchScriptedHelper, repoRoot } from "./harness.ts";
import { ToolFailure } from "../src/errors.ts";
import { readExpectedBuild } from "../src/helper-build.ts";

async function failureOf(promise: Promise<unknown>): Promise<ToolFailure> {
  try {
    await promise;
  } catch (error) {
    assert.ok(error instanceof ToolFailure, String(error));
    return error;
  }
  assert.fail("expected the request to fail");
}

test("a helper whose build id matches the expected one is used", async () => {
  const logged: string[] = [];
  const { helper, close } = await launchScriptedHelper({ build: "aaaa1111aaaa1111" }, { expectedBuild: "aaaa1111aaaa1111", log: (line) => logged.push(line) });
  try {
    await helper.request("list_windows", {});
    assert.deepEqual(logged, []);
  } finally {
    await close();
  }
});

test("a helper built from other sources fails every request, naming both ids, and the server logs both", async () => {
  const logged: string[] = [];
  const { helper, close } = await launchScriptedHelper({ build: "bbbb2222bbbb2222" }, { expectedBuild: "aaaa1111aaaa1111", log: (line) => logged.push(line) });
  try {
    const first = await failureOf(helper.request("list_windows", {}));
    const second = await failureOf(helper.request("list_windows", {}));
    for (const failure of [first, second]) {
      assert.equal(failure.code, "helper_version_mismatch");
      assert.match(failure.message, /bbbb2222bbbb2222/);
      assert.match(failure.message, /aaaa1111aaaa1111/);
      assert.match(failure.message, /rebuild/i);
    }
    assert.equal(logged.length, 1);
    assert.match(logged[0], /bbbb2222bbbb2222/);
    assert.match(logged[0], /aaaa1111aaaa1111/);
  } finally {
    await close();
  }
});

test("a helper that reports no build id is a mismatch when one is expected", async () => {
  const { helper, close } = await launchScriptedHelper({}, { expectedBuild: "aaaa1111aaaa1111", log: () => {} });
  try {
    const failure = await failureOf(helper.request("list_windows", {}));
    assert.equal(failure.code, "helper_version_mismatch");
    assert.match(failure.message, /aaaa1111aaaa1111/);
  } finally {
    await close();
  }
});

test("without an expected build id only the protocol version is checked", async () => {
  const unknown = await launchScriptedHelper({ build: "bbbb2222bbbb2222" }, { expectedBuild: null, log: () => {} });
  const missing = await launchScriptedHelper({}, { expectedBuild: null, log: () => {} });
  try {
    await unknown.helper.request("list_windows", {});
    await missing.helper.request("list_windows", {});
  } finally {
    await unknown.close();
    await missing.close();
  }
  const wrongProtocol = await launchScriptedHelper({ version: "10" }, { expectedBuild: null, log: () => {} });
  try {
    const failure = await failureOf(wrongProtocol.helper.request("list_windows", {}));
    assert.equal(failure.code, "helper_version_mismatch");
  } finally {
    await wrongProtocol.close();
  }
});

test("the expected build id is read from helper-build.json next to the helper, and absent when there is none", async () => {
  await mkdir(join(repoRoot(), "test-tmp"), { recursive: true });
  const dir = await mkdtemp(join(repoRoot(), "test-tmp", "helper-build-"));
  try {
    const helperPath = join(dir, "deskpass-helper.exe");
    assert.equal(readExpectedBuild(helperPath), undefined);
    await writeFile(join(dir, "helper-build.json"), JSON.stringify({ build: "0123456789abcdef" }));
    assert.equal(readExpectedBuild(helperPath), "0123456789abcdef");
    await writeFile(join(dir, "helper-build.json"), JSON.stringify({ build: "" }));
    assert.equal(readExpectedBuild(helperPath), undefined);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("a missing helper-build.json is silent, a malformed one skips the check and says so on one line", async () => {
  await mkdir(join(repoRoot(), "test-tmp"), { recursive: true });
  const dir = await mkdtemp(join(repoRoot(), "test-tmp", "helper-build-"));
  try {
    const helperPath = join(dir, "deskpass-helper.exe");
    const logged: string[] = [];
    assert.equal(readExpectedBuild(helperPath, (line) => logged.push(line)), undefined);
    assert.deepEqual(logged, [], "no file is the normal case for a source or test build");
    await writeFile(join(dir, "helper-build.json"), "not json");
    assert.equal(readExpectedBuild(helperPath, (line) => logged.push(line)), undefined);
    assert.equal(logged.length, 1);
    assert.match(logged[0], /helper-build\.json/);
    assert.doesNotMatch(logged[0], /[\r\n]/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("the expected build id is read once, when the client is created, so a restarted helper cannot pass by matching a newer build", async () => {
  // The second read stands in for a rebuild that rewrote helper-build.json after this server started.
  const reads = ["aaaa1111aaaa1111", "bbbb2222bbbb2222"];
  const { helper, close } = await launchScriptedHelper(
    { build: "aaaa1111aaaa1111", handlers: { list_windows: [{ exit: true }, { result: { windows: [] } }] } },
    { readExpectedBuild: () => reads.shift(), log: () => {} },
  );
  try {
    assert.equal(reads.length, 1, "read when the client is created");
    assert.equal((await failureOf(helper.request("list_windows", {}))).code, "helper_crashed");
    await helper.request("list_windows", {});
    assert.equal(reads.length, 1, "not read again when the helper restarts");
  } finally {
    await close();
  }
});
