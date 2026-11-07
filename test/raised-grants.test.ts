import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { copyFile, mkdtemp, readFile, rm, utimes, writeFile } from "node:fs/promises";
import path, { join } from "node:path";
import { test } from "node:test";
import { GrantStore, type GrantMac } from "../src/grants.ts";
import { defaultLockDir } from "../src/lock.ts";
import { appRows, noStartMenu, recentLogUses } from "../src/settings-apps.ts";
import { approvalAnswers, connectServer, repoRoot, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 96: a signed grants.json, revoking a grant or raising a tier, is taken by the running
// server at its next approval check. Seam: the MCP client, the server and the scripted helper. The
// files in test/fixtures/signed-grants were written by the native control panel's grants writer
// under the fake helper's test key (ticket 118 retired both); they are checked here with the
// server's rules, as the files the settings page writes are.

/**
 * The fake helper's key (test/fake-helper.mjs `GRANT_KEY`), which the fixtures are signed with. Test-only HMAC key,
 * not a real key; the MACs in the JSON files of test/fixtures/signed-grants (JSON has no comments) are test-only signatures made with it.
 */
const FAKE_GRANT_KEY = "0f1e2d3c4b5a69788796a5b4c3d2e1f000112233445566778899aabbccddeeff";
const FIXTURES = join(repoRoot(), "test", "fixtures", "signed-grants");

const SIGNER = "C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd";
const DEMO = "C:\\Apps\\Demo.exe";
const NOTES = "C:\\Apps\\Notes.exe";
const DEMO_KEY = `signer:${SIGNER}|demo.exe`;
const NOTES_KEY = `signer:${SIGNER}|notes.exe`;

const windowOf = (app: string, name: string, id: string, pid: number) => ({
  window_id: id,
  app,
  app_name: name,
  title: name,
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 200,
  height: 100,
  pid,
});
const demo = windowOf(DEMO, "Demo", "5", 4242);

const signedAs = (fileName: string): FakeHandler => ({ result: { signed: true, signer: SIGNER, file_name: fileName, original_name: fileName } });
const repeat = <T>(count: number, value: T): T[] => Array.from({ length: count }, () => value);

let stamp = Date.parse("2026-05-01T00:00:00Z");

/** Puts a fixture in place as the case's grants.json (the fake's key beside it), with a modification time of its own. */
async function putFixture(harness: Harness, name: string): Promise<void> {
  await writeFile(join(harness.dir, "grants.key"), FAKE_GRANT_KEY);
  const file = join(harness.dir, "grants.json");
  await copyFile(join(FIXTURES, name), file);
  stamp += 60_000;
  await utimes(file, new Date(stamp), new Date(stamp));
}

async function methods(harness: Harness): Promise<string[]> {
  return ((await harness.requests()) as { method?: string }[]).map((entry) => entry.method ?? "");
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

function typeText(harness: Harness, windowId = "5") {
  return harness.client.callTool({ name: "computer_type_text", arguments: { window_id: windowId, text: "x" } });
}

test("the fixtures are signed with the fake helper's key: each entry and the file verify", async () => {
  const hmac = (text: string) => createHmac("sha256", Buffer.from(FAKE_GRANT_KEY, "hex")).update(text, "utf8").digest("hex");
  const canonical = (fields: Record<string, unknown>) => JSON.stringify(Object.fromEntries(Object.entries(fields).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))));
  const generations: number[] = [];
  for (const name of ["start.json", "revoked.json", "raised.json"]) {
    const file = JSON.parse(await readFile(join(FIXTURES, name), "utf8"));
    for (const entry of [...file.grants, ...file.raised]) {
      const { mac, ...fields } = entry;
      assert.equal(mac, hmac(canonical(fields)), `${name}: ${entry.key}`);
    }
    const sealed = { version: 1, generation: file.generation, grant_macs: file.grants.map((entry: { mac: string }) => entry.mac), raised_macs: file.raised.map((entry: { mac: string }) => entry.mac) };
    assert.equal(file.file_mac, hmac(canonical(sealed)), name);
    generations.push(file.generation);
  }
  // Each write is the verified generation plus one, never a new count.
  assert.deepEqual(generations, [1, 2, 3]);
  const raised = JSON.parse(await readFile(join(FIXTURES, "raised.json"), "utf8"));
  // Raising Notes removed its always-allow grant.
  assert.deepEqual(raised.grants.map((entry: { key: string }) => entry.key), []);
  assert.deepEqual(raised.raised.map(({ key, tier }: { key: string; tier: string }) => ({ key, tier })), [{ key: NOTES_KEY, tier: "blocked" }]);
});

test("a raise made under a signer's key still applies to a signed app whose version resource names another file", async () => {
  const notes = windowOf(NOTES, "Notes", "6", 4343);
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: repeat(2, { result: notes }),
        app_identity: repeat(2, { result: { signed: true, signer: SIGNER, file_name: "notes.exe", original_name: "notes.dll" } }),
        ask_user: [{ result: { answer: "allow" } }],
      },
    },
  });
  try {
    await putFixture(harness, "raised.json");
    const result = await typeText(harness, "6");
    assert.equal(code(result), "blocked_app", textContent(result));
    assert.equal((await methods(harness)).includes("ask_user"), false, "a blocked app is refused without asking");

    // The settings page shows that app once, under the key the raise is under, so what it shows is
    // what the tool does, and its controls act on the raise that is in force.
    const hmac = (fields: Record<string, unknown>) =>
      createHmac("sha256", Buffer.from(FAKE_GRANT_KEY, "hex"))
        .update(JSON.stringify(Object.fromEntries(Object.entries(fields).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))), "utf8")
        .digest("hex");
    const mac: GrantMac = async ({ entries }) => ({ macs: entries.map((entry) => hmac(entry.fields)), key_state: "ok" });
    const view = await new GrantStore({ dir: harness.dir, record: async () => undefined }).current(mac);
    const rows = await appRows(view, await recentLogUses(join(harness.dir, "logs")), noStartMenu());
    assert.deepEqual(rows.map((row) => [row.key, row.tier, row.source, row.raised, row.path]), [[NOTES_KEY, "blocked", "user", "blocked", NOTES]]);
  } finally {
    await harness.close();
  }
});

test("a revoke written to the file takes effect at the running server's next check: the app is asked again", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { get_window: repeat(2, { result: demo }), app_identity: repeat(2, signedAs("demo.exe")), ask_user: [{ result: { answer: "allow" } }] } },
  });
  try {
    await putFixture(harness, "start.json");
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal((await methods(harness)).includes("ask_user"), false, "the grant allowed Demo without asking");
    await putFixture(harness, "revoked.json");
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal((await methods(harness)).filter((method) => method === "ask_user").length, 1, "Demo was asked once its grant was revoked");
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "grant", answer: "allow_always" },
      { channel: "dialog", answer: "allow_once" },
    ]);
  } finally {
    await harness.close();
  }
});

test("an app raised to blocked in the file is refused before it is launched, and marked blocked in computer_list_apps", async () => {
  const notesApp = { app_id: `exe:${NOTES}`, name: "Notes", kind: "desktop", running: false };
  const paint = { app_id: "exe:C:\\Apps\\paint.exe", name: "Paint", kind: "desktop", running: false };
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async (app) => app,
    script: { handlers: { app_identity: repeat(4, signedAs("notes.exe")), list_apps: [{ result: { apps: [notesApp, paint], total: 2 } }] } },
  });
  try {
    await putFixture(harness, "raised.json");
    const launched = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: NOTES } });
    assert.equal(launched.isError, true);
    assert.equal(code(launched), "blocked_app");
    assert.match(textContent(launched), /blocked tier \(the user raised it/);
    const sent = await methods(harness);
    assert.equal(sent.includes("launch_app"), false, sent.join(", "));
    assert.equal(sent.includes("ask_user"), false, sent.join(", "));
    assert.deepEqual(await approvalAnswers(harness), [{ channel: null, answer: "blocked" }]);

    const listed = textContent(await harness.client.callTool({ name: "computer_list_apps", arguments: {} })).split("\n");
    assert.match(listed[0] ?? "", /name: "Notes".*blocked: true/);
    assert.doesNotMatch(listed[1] ?? "", /blocked/);
    // Only an app a raise could be about is identified: Paint's file name is in no raise.
    const identified = ((await harness.requests()) as { method?: string; params?: { path?: string } }[]).filter((entry) => entry.method === "app_identity").map((entry) => entry.params?.path);
    assert.equal(identified.every((app) => app === NOTES), true, identified.join(", "));
  } finally {
    await harness.close();
  }
});

test("a packaged app raised to blocked is marked blocked in computer_list_apps by its package family", async () => {
  const hmac = (text: string) => createHmac("sha256", Buffer.from(FAKE_GRANT_KEY, "hex")).update(text, "utf8").digest("hex");
  const raised = { key: "package:contoso.notes_8wekyb3d8bbwe", tier: "blocked", created_at: "2026-01-02T00:00:00.000Z" };
  const mac = hmac(JSON.stringify({ created_at: raised.created_at, key: raised.key, tier: raised.tier }));
  const fileMac = hmac(JSON.stringify({ generation: 1, grant_macs: [], raised_macs: [mac], version: 1 }));
  const app = { app_id: "uwp:Contoso.Notes_8wekyb3d8bbwe!App", name: "Contoso Notes", kind: "uwp", running: false };
  const harness = await connectServer({ script: { handlers: { list_apps: [{ result: { apps: [app], total: 1 } }] } } });
  try {
    await writeFile(join(harness.dir, "grants.key"), FAKE_GRANT_KEY);
    await writeFile(join(harness.dir, "grants.json"), JSON.stringify({ version: 1, generation: 1, grants: [], raised: [{ ...raised, mac }], file_mac: fileMac }));
    const listed = textContent(await harness.client.callTool({ name: "computer_list_apps", arguments: {} }));
    assert.match(listed, /blocked: true/);
    assert.equal((await methods(harness)).includes("app_identity"), false, "a package's key needs no identity lookup");
  } finally {
    await harness.close();
  }
});

test("the server always starts its helper with --grants-dir, its own folder resolved when it was given none", async () => {
  const seen: string[][] = [];
  // The default folder is under LOCALAPPDATA: a test folder here, never the user's own.
  const local = await mkdtemp(join(repoRoot(), "test-tmp", "localappdata-"));
  const saved = process.env.LOCALAPPDATA;
  process.env.LOCALAPPDATA = local;
  const harness = await connectServer({
    grantsDir: undefined,
    spawn: ((_command: string, args: readonly string[]) => {
      seen.push([...args]);
      throw new Error("not started in this test");
    }) as never,
  });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.ok(seen.length > 0, "the helper was started");
    const args = seen[0]!;
    const at = args.indexOf("--grants-dir");
    assert.ok(at >= 0, args.join(" "));
    assert.equal(args[at + 1], path.resolve(local, "deskpass"));
    assert.equal(args[at + 1], path.resolve(defaultLockDir()));
  } finally {
    await harness.close();
    if (saved === undefined) delete process.env.LOCALAPPDATA;
    else process.env.LOCALAPPDATA = saved;
    await rm(local, { recursive: true, force: true });
  }
});
