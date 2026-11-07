import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { ASK_USER_DIALOG_MS } from "../src/config.ts";
import { approvalAnswers, approvalEvents, connectServer, repoRoot, textContent, type FakeHandler, type Harness, type HarnessOptions } from "./harness.ts";

// Ticket 95: an app that can be always allowed (normal tier, a signer's or a package's identity) is
// asked through the helper's dialog with a third button, Always allow, even when the client has form
// elicitation; the answer `allow_always` writes a grant to grants.json, and later checks (of this
// server or a new one) allow the app without asking, until the app's identity no longer matches.
// Elicitation stays allow-once. Seam one: the MCP client, the server and the scripted helper, which
// signs grants with its fixed test key in the case's folder.

const SIGNER = "C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd";
const OTHER_SIGNER = "C=US, O=Somebody Else, CN=Somebody Else";
const DEMO_KEY = `signer:${SIGNER}|demo.exe`;
// Test-only HMAC key, not a real key: the fake helper's fixed key (test/fake-helper.mjs), used to sign the grants.json files built here.
const FAKE_GRANT_KEY = "0f1e2d3c4b5a69788796a5b4c3d2e1f000112233445566778899aabbccddeeff";

const demo = {
  window_id: "5",
  app: "C:\\Apps\\Demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 200,
  height: 100,
  pid: 4242,
};

const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const edge = { ...demo, window_id: "6", app: EDGE, app_name: "Microsoft Edge", title: "Edge", pid: 4343 };

const signed = (signer = SIGNER, fileName = "demo.exe"): FakeHandler => ({ result: { signed: true, signer, file_name: fileName, original_name: fileName } });
const unsigned = (fileName = "demo.exe"): FakeHandler => ({ result: { signed: false, signer: null, file_name: fileName } });
const answer = (value: string): FakeHandler => ({ result: { answer: value } });
const cannotShow: FakeHandler = { error: { code: "prompt_unavailable", message: "There is no interactive desktop." } };
const repeat = <T>(count: number, value: T): T[] => Array.from({ length: count }, () => value);

interface Logged {
  method?: string;
  params?: Record<string, unknown>;
}

function typeText(harness: Harness, windowId = "5") {
  return harness.client.callTool({ name: "computer_type_text", arguments: { window_id: windowId, text: "x" } });
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

async function asks(harness: Harness): Promise<Record<string, unknown>[]> {
  return ((await harness.requests()) as Logged[]).filter((entry) => entry.method === "ask_user").map((entry) => entry.params ?? {});
}

async function logged(harness: Harness, ...kinds: string[]): Promise<Record<string, unknown>[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  let raw = "";
  try {
    raw = await readFile(join(harness.dir, "logs", name), "utf8");
  } catch {
    return [];
  }
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => kinds.includes(event.event as string))
    .map(({ time: _time, session: _session, ...rest }) => rest);
}

async function grantsFile(dir: string): Promise<{ grants: Record<string, string>[]; raised: Record<string, string>[] } | null> {
  const file = join(dir, "grants.json");
  if (!existsSync(file)) return null;
  return JSON.parse(await readFile(file, "utf8"));
}

/** A server for `demo` (and any other windows), its identity answers and dialog answers scripted. */
function connect(handlers: Record<string, FakeHandler[]>, options: HarnessOptions = {}) {
  return connectServer({ elicitation: "form", ...options, script: { handlers } });
}

const approveParams = (allowAlways: boolean, window: { app: string; app_name: string } = demo) => ({
  kind: "approve_app",
  app_name: window.app_name,
  app_path: window.app,
  timeout_ms: ASK_USER_DIALOG_MS,
  allow_always: allowAlways,
});

test("a signed normal app is asked through the dialog with Always allow offered, even when the client has elicitation", async () => {
  const harness = await connect({ get_window: repeat(3, { result: demo }), app_identity: repeat(3, signed()), ask_user: [answer("allow")] });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(harness.elicitations.length, 0, "no elicitation for an app that can be always allowed");
    assert.deepEqual(await asks(harness), [approveParams(true)]);
    // Allow once: this session only, no grant.
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "dialog", answer: "allow_once" },
      { channel: "session", answer: "allow_once" },
    ]);
    assert.equal(await grantsFile(harness.dir), null);
  } finally {
    await harness.close();
  }
});

test("a packaged app is offered Always allow too", async () => {
  const packaged: FakeHandler = { result: { signed: false, signer: null, file_name: "demo.exe", package_family_name: "Contoso.Demo_8wekyb3d8bbwe" } };
  const harness = await connect({ get_window: repeat(2, { result: demo }), app_identity: [packaged], ask_user: [answer("deny")] });
  try {
    assert.equal(code(await typeText(harness)), "approval_declined");
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual(await asks(harness), [approveParams(true)]);
  } finally {
    await harness.close();
  }
});

test("when the dialog cannot be shown for such an app, the call is refused approval_unavailable and no elicitation is sent", async () => {
  const harness = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, signed()), ask_user: [cannotShow] });
  try {
    assert.equal(code(await typeText(harness)), "approval_unavailable");
    assert.equal(harness.elicitations.length, 0, "the model-reachable channel is never the fallback");
    assert.deepEqual(await approvalAnswers(harness), [{ channel: "dialog", answer: "unavailable" }]);
  } finally {
    await harness.close();
  }
});

test("an app that cannot be always allowed is asked as before: elicitation when the client has it, otherwise a dialog without Always allow", async () => {
  // Unsigned and high-risk (signed Edge), with form elicitation: the elicitation asks.
  const withElicitation = await connect({
    get_window: [{ result: demo }, { result: edge }],
    app_identity: [unsigned(), signed("C=US, O=Microsoft Corporation, CN=Microsoft Corporation", "msedge.exe")],
  });
  try {
    assert.notEqual((await typeText(withElicitation)).isError, true);
    const edgeResult = await typeText(withElicitation, "6");
    assert.notEqual(edgeResult.isError, true, textContent(edgeResult));
    assert.equal(withElicitation.elicitations.length, 2);
    assert.deepEqual(await asks(withElicitation), []);
  } finally {
    await withElicitation.close();
  }
  // Without elicitation: the dialog, two buttons.
  const withoutElicitation = await connect(
    {
      get_window: [{ result: demo }, { result: edge }],
      app_identity: [unsigned(), signed("C=US, O=Microsoft Corporation, CN=Microsoft Corporation", "msedge.exe")],
      ask_user: [answer("allow"), answer("allow")],
    },
    { elicitation: false },
  );
  try {
    assert.notEqual((await typeText(withoutElicitation)).isError, true);
    assert.notEqual((await typeText(withoutElicitation, "6")).isError, true);
    assert.deepEqual(await asks(withoutElicitation), [approveParams(false), approveParams(false, edge)]);
  } finally {
    await withoutElicitation.close();
  }
});

test("an allow_always answer for a dialog that did not offer it allows nothing and writes no grant", async () => {
  const harness = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, unsigned()), ask_user: [answer("allow_always")] }, { elicitation: false });
  try {
    assert.equal(code(await typeText(harness)), "approval_unavailable");
    assert.equal(await grantsFile(harness.dir), null);
  } finally {
    await harness.close();
  }
});

test("Elicitation Accept is allow-once only: it writes no grant", async () => {
  const harness = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, unsigned()) });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(harness.elicitations.length, 1);
    assert.deepEqual(await approvalAnswers(harness), [{ channel: "elicitation", answer: "allow_once" }]);
    assert.equal(await grantsFile(harness.dir), null);
  } finally {
    await harness.close();
  }
});

test("Always allow writes a signed grant; this server and a new one allow the app without asking", async () => {
  const before = Date.now();
  const first = await connect({ get_window: repeat(3, { result: demo }), app_identity: repeat(3, signed()), ask_user: [answer("allow_always")] });
  try {
    assert.notEqual((await typeText(first)).isError, true);
    const file = await grantsFile(first.dir);
    assert.equal(file?.grants.length, 1, JSON.stringify(file));
    const [grant] = file?.grants ?? [];
    assert.equal(grant?.key, DEMO_KEY);
    assert.equal(grant?.display_name, "Demo");
    assert.equal(grant?.path, demo.app);
    assert.ok(Date.parse(grant?.created_at ?? "") >= before - 1000, grant?.created_at);
    assert.equal(grant?.last_used_at, grant?.created_at);
    assert.match(grant?.mac ?? "", /^[0-9a-f]{64}$/);
    // The next check of this server uses the grant.
    assert.notEqual((await typeText(first)).isError, true);
    assert.deepEqual(await approvalAnswers(first), [
      { channel: "dialog", answer: "allow_always" },
      { channel: "grant", answer: "allow_always" },
    ]);
    assert.equal((await asks(first)).length, 1);
  } finally {
    await first.finish();
  }
  const second = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, signed()) }, { grantsDir: first.dir });
  try {
    assert.notEqual((await typeText(second)).isError, true);
    assert.deepEqual(await asks(second), [], "a new server does not ask again");
    assert.equal(second.elicitations.length, 0);
    assert.deepEqual(await approvalAnswers(second), [{ channel: "grant", answer: "allow_always" }]);
  } finally {
    await second.close();
    await first.close();
  }
});

test("a grant whose app's identity changed is not used: the user is asked and grant_mismatch is logged", async () => {
  const first = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, signed()), ask_user: [answer("allow_always")] });
  try {
    assert.notEqual((await typeText(first)).isError, true);
  } finally {
    await first.finish();
  }
  // Another signer at the same path: asked (still offered, the new identity is signed), not allowed by the grant.
  const resigned = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, signed(OTHER_SIGNER)), ask_user: [answer("deny")] }, { grantsDir: first.dir });
  // No valid signature any more: asked with two buttons.
  const broken = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, unsigned()), ask_user: [answer("deny")] }, { grantsDir: first.dir, elicitation: false });
  try {
    assert.equal(code(await typeText(resigned)), "approval_declined");
    assert.deepEqual(await asks(resigned), [approveParams(true)]);
    assert.deepEqual(await logged(resigned, "grant_mismatch"), [
      { event: "grant_mismatch", key: DEMO_KEY, current_key: `signer:${OTHER_SIGNER}|demo.exe`, path: demo.app },
    ]);
    assert.equal(code(await typeText(broken)), "approval_declined");
    assert.deepEqual(await asks(broken), [approveParams(false)]);
    assert.deepEqual(await logged(broken, "grant_mismatch"), [{ event: "grant_mismatch", key: DEMO_KEY, current_key: "c:\\apps\\demo.exe", path: demo.app }]);
    // The grant itself is kept: the original app, signed as before, is still allowed by it.
    assert.equal((await grantsFile(first.dir))?.grants[0]?.key, DEMO_KEY);
  } finally {
    await resigned.close();
    await broken.close();
    await first.close();
  }
});

test("allow once and a refusal are remembered by the app's identity key: the same signer and file name at another path is not asked again", async () => {
  const copy = { ...demo, window_id: "7", app: "D:\\Portable\\Demo\\demo.exe", pid: 5151 };
  const harness = await connect({
    get_window: [{ result: demo }, { result: demo }, { result: copy }],
    app_identity: [signed(), signed()],
    ask_user: [answer("allow")],
  });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness, "7")).isError, true);
    assert.equal((await asks(harness)).length, 1, "one identity, one question");
  } finally {
    await harness.close();
  }
  const refused = await connect({
    get_window: [{ result: demo }, { result: copy }],
    app_identity: [signed(), signed()],
    ask_user: [answer("deny")],
  });
  try {
    assert.equal(code(await typeText(refused)), "approval_declined");
    assert.equal(code(await typeText(refused, "7")), "approval_refused_earlier");
    assert.equal((await asks(refused)).length, 1);
  } finally {
    await refused.close();
  }
});

test("a refusal is still remembered when the app's identity cannot be read at the next check", async () => {
  const harness = await connect({
    get_window: [{ result: demo }, { result: demo }],
    app_identity: [signed(), { error: { code: "internal", message: "WinVerifyTrust failed" } }],
    ask_user: [answer("deny")],
  });
  try {
    assert.equal(code(await typeText(harness)), "approval_declined");
    // Keyed by its path now, which the refusal under its signer's key is remembered by as well.
    assert.equal(code(await typeText(harness)), "approval_refused_earlier");
    assert.equal((await asks(harness)).length, 1);
    assert.equal(harness.elicitations.length, 0, "not asked again through the client either");
  } finally {
    await harness.close();
  }
});

test("a raise in force while the file is void is written back with a new grant, not dropped", async () => {
  const otherKey = `signer:${SIGNER}|other.exe`;
  const hmac = (text: string) => createHmac("sha256", Buffer.from(FAKE_GRANT_KEY, "hex")).update(text, "utf8").digest("hex");
  const canonical = (fields: Record<string, unknown>) => JSON.stringify(Object.fromEntries(Object.entries(fields).sort(([a], [b]) => (a < b ? -1 : 1))));
  const raise = { key: otherKey, tier: "blocked", created_at: "2026-01-02T00:00:00.000Z" };
  const raised = { ...raise, mac: hmac(canonical(raise)) };
  const third = { ...demo, window_id: "8", app: "C:\\Apps\\Third.exe", app_name: "Third", pid: 6161 };
  const harness = await connect({
    get_window: [{ result: demo }, { result: third }, { result: third }],
    app_identity: [signed(), signed(SIGNER, "third.exe"), signed(SIGNER, "third.exe")],
    ask_user: [answer("deny"), answer("allow_always")],
  });
  const file = join(harness.dir, "grants.json");
  const stamp = (day: string) => utimes(file, new Date(day), new Date(day));
  try {
    await writeFile(join(harness.dir, "grants.key"), FAKE_GRANT_KEY);
    const fileMac = hmac(canonical({ version: 1, generation: 1, grant_macs: [], raised_macs: [raised.mac] }));
    await writeFile(file, JSON.stringify({ version: 1, generation: 1, grants: [], raised: [raised], file_mac: fileMac }));
    await stamp("2026-03-01T00:00:00Z");
    // The signed raise is read and verified.
    assert.equal(code(await typeText(harness)), "approval_declined");
    // Taken out by hand: the file is void, and the raise last verified stays in force.
    await writeFile(file, JSON.stringify({ version: 1, generation: 2, grants: [], raised: [] }));
    await stamp("2026-03-02T00:00:00Z");
    assert.notEqual((await typeText(harness, "8")).isError, true);
    const written = await grantsFile(harness.dir);
    assert.deepEqual(written?.grants.map((grant) => grant.key), [`signer:${SIGNER}|third.exe`]);
    assert.deepEqual(written?.raised.map(({ key, tier }) => ({ key, tier })), [{ key: otherKey, tier: "blocked" }], "the raise is carried, not dropped");
  } finally {
    await harness.close();
  }
});

test("the identity a window check gets with its process is the one a later launch of the same executable uses", async () => {
  const root = await mkdtemp(join(repoRoot(), "test-tmp", "identity-"));
  const exe = join(root, "Demo.exe");
  await writeFile(exe, "demo");
  const window = { ...demo, app: exe };
  const launched = { pid: 50, window: { window_id: "5", app: exe, app_name: "Demo", title: "Demo", minimized: false, elevated: false } };
  const packaged: FakeHandler = { result: { signed: false, signer: null, file_name: "demo.exe", package_family_name: "Contoso.Demo_8wekyb3d8bbwe" } };
  const harness = await connect(
    {
      launch_app: [{ result: launched }, { result: launched }],
      get_window: repeat(3, { result: window }),
      app_identity: [signed(), packaged],
      ask_user: [answer("allow"), answer("allow")],
    },
    { resolveApp: async () => exe },
  );
  try {
    assert.notEqual((await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } })).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } })).isError, true);
    const identityRequests = ((await harness.requests()) as Logged[]).filter((entry) => entry.method === "app_identity").map((entry) => entry.params);
    // The launch's answer (no process) does not stand in for the window's: the window asks with its pid,
    // and that answer is the one the second launch uses, so both are keyed alike from then on.
    assert.deepEqual(identityRequests, [{ path: exe }, { path: exe, pid: demo.pid }]);
    const keys = (await approvalEvents(harness)).map((event) => (event.app as { key?: unknown }).key);
    assert.deepEqual(keys, [`signer:${SIGNER}|demo.exe`, "package:contoso.demo_8wekyb3d8bbwe", "package:contoso.demo_8wekyb3d8bbwe"]);
  } finally {
    await harness.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("a lone surrogate in the app's name or path is written as U+FFFD, so the helper can read the request", async () => {
  const odd = { ...demo, app_name: "Demo\ud800", app: "C:\\Apps\\Demo\udc00.exe" };
  const harness = await connect({ get_window: repeat(2, { result: odd }), app_identity: repeat(2, signed()), ask_user: [answer("allow_always")] });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    const [grant] = (await grantsFile(harness.dir))?.grants ?? [];
    assert.equal(grant?.display_name, "Demo\ufffd");
    assert.equal(grant?.path, "C:\\Apps\\Demo\ufffd.exe");
    const signing = ((await harness.requests()) as Logged[]).filter((entry) => entry.method === "grant_mac");
    const fields = signing.flatMap((entry) => (entry.params?.entries as { fields: Record<string, unknown> }[]).flatMap((macEntry) => Object.values(macEntry.fields)));
    assert.equal(fields.some((value) => typeof value === "string" && /\p{Cs}/u.test(value)), false, "no grant_mac request holds a lone surrogate");
  } finally {
    await harness.close();
  }
});

// A signature covers a file's content, never its name: a signed copy of another program saved
// under an app's name is known by its path (its version resource names another program), and is in
// the tier of either name.
const copyOf = (originalName: string | null): FakeHandler => ({ result: { signed: true, signer: SIGNER, file_name: "demo.exe", original_name: originalName } });

test("a signed copy of another program saved as an always-allowed app's name does not use its grant", async () => {
  const first = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, signed()), ask_user: [answer("allow_always")] });
  try {
    assert.notEqual((await typeText(first)).isError, true);
  } finally {
    await first.finish();
  }
  const copy = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, copyOf("other.exe")), ask_user: [answer("deny")] }, { grantsDir: first.dir, elicitation: false });
  try {
    assert.equal(code(await typeText(copy)), "approval_declined");
    // Asked, and without 永遠允許: it has no signer's identity.
    assert.deepEqual(await asks(copy), [approveParams(false)]);
    assert.deepEqual(await logged(copy, "grant_mismatch"), [{ event: "grant_mismatch", key: DEMO_KEY, current_key: "c:\\apps\\demo.exe", path: demo.app }]);
    assert.equal((await grantsFile(first.dir))?.grants[0]?.key, DEMO_KEY, "the grant itself is kept");
  } finally {
    await copy.close();
    await first.close();
  }
});

test("a copy of a shell under another name is in the blocked tier: refused without asking, whatever grant its name has", async () => {
  for (const original of ["cmd.exe", "PowerShell.EXE", "wt.exe"]) {
    const harness = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, copyOf(original.toLowerCase())) });
    try {
      const result = await typeText(harness);
      assert.equal(code(result), "blocked_app", original);
      assert.match(textContent(result), /blocked tier/i, original);
      assert.deepEqual(await asks(harness), [], original);
      assert.equal(harness.elicitations.length, 0, original);
    } finally {
      await harness.close();
    }
  }
});

test("a signed Electron app keeps the always-allow it was given under its signer", async () => {
  const first = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, signed()), ask_user: [answer("allow_always")] });
  try {
    assert.notEqual((await typeText(first)).isError, true);
  } finally {
    await first.finish();
  }
  const electron = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, copyOf("electron.exe")) }, { grantsDir: first.dir, elicitation: false });
  try {
    assert.notEqual((await typeText(electron)).isError, true);
    assert.deepEqual(await asks(electron), [], "the grant applies, so nobody is asked");
    assert.deepEqual(await logged(electron, "grant_mismatch"), []);
  } finally {
    await electron.close();
    await first.close();
  }
});

test("a signed app whose version resource names no program is asked allow-once only", async () => {
  const harness = await connect({ get_window: repeat(2, { result: demo }), app_identity: repeat(2, copyOf(null)), ask_user: [answer("allow")] }, { elicitation: false });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await asks(harness), [approveParams(false)]);
  } finally {
    await harness.close();
  }
});
