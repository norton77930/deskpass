import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { GrantFileVoidError, GrantStore, type GrantMac, type MacEntry } from "../src/grants.ts";
import { approvalAnswers, connectServer, launchScriptedHelper, repoRoot, textContent, type Harness } from "./harness.ts";

// Ticket 94: always-allow grants and raised tiers live in grants.json beside grants.key, each entry
// and the file itself signed with HMAC-SHA256 under the helper's key. An entry whose MAC does not
// verify is ignored and logged; a file whose own MAC does not verify (an entry added, removed or
// reordered), or an older copy of it, is void as a whole; a key that cannot be unlocked voids
// everything; the file is read again at the approval check after it changes. Seam one: the MCP
// client, the server and the scripted helper, which signs with a fixed test key kept as hex in the
// case's grants.key.

/** The fake helper's key (test/fake-helper.mjs `GRANT_KEY`); the real helper's never leaves it. Test-only HMAC key, not a real key. */
const FAKE_GRANT_KEY = "0f1e2d3c4b5a69788796a5b4c3d2e1f000112233445566778899aabbccddeeff";

const SIGNER = "C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd";
const DEMO_KEY = `signer:${SIGNER}|demo.exe`;

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
const MICROSOFT = "C=US, S=Washington, L=Redmond, O=Microsoft Corporation, CN=Microsoft Corporation";

type Entry = Record<string, string>;

function hmac(text: string): string {
  return createHmac("sha256", Buffer.from(FAKE_GRANT_KEY, "hex")).update(text, "utf8").digest("hex");
}

/** The canonical form: the fields, names sorted, as JSON without whitespace. */
function canonical(fields: Record<string, unknown>): string {
  return JSON.stringify(Object.fromEntries(Object.entries(fields).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))));
}

/** An entry with its MAC: HMAC-SHA256 of its fields but `mac`, canonical. */
function sign(entry: Entry): Entry {
  return { ...entry, mac: hmac(canonical(entry)) };
}

/** A grants.json with its generation and file_mac over `{ version, generation, grant_macs, raised_macs }`. */
function sealed(generation: number, grants: Entry[], raised: Entry[] = []) {
  const fields = { version: 1, generation, grant_macs: grants.map((entry) => entry.mac), raised_macs: raised.map((entry) => entry.mac) };
  return { version: 1, generation, grants, raised, file_mac: hmac(canonical(fields)) };
}

function grantOf(key: string, extra: Entry = {}): Entry {
  return { key, display_name: "Demo", path: demo.app, created_at: "2026-01-01T00:00:00.000Z", last_used_at: "2026-01-01T00:00:00.000Z", ...extra };
}

function raisedOf(key: string, tier: string): Entry {
  return { key, tier, created_at: "2026-01-02T00:00:00.000Z" };
}

let stamp = Date.parse("2026-03-01T00:00:00Z");

/** Writes the case's grants.json (and the fake's key, unless told not to), with a modification time of its own. */
async function writeGrants(harness: Harness, content: unknown, { key = true }: { key?: boolean } = {}): Promise<void> {
  if (key) await writeFile(join(harness.dir, "grants.key"), FAKE_GRANT_KEY);
  const file = join(harness.dir, "grants.json");
  await writeFile(file, typeof content === "string" ? content : JSON.stringify(content));
  stamp += 60_000;
  await utimes(file, new Date(stamp), new Date(stamp));
}

const identities = (count: number, signer = SIGNER, fileName = "demo.exe") =>
  Array.from({ length: count }, () => ({ result: { signed: true, signer, file_name: fileName } }));
const windows = (count: number, window: Record<string, unknown> = demo) => Array.from({ length: count }, () => ({ result: window }));

/** Ticket 95: a signed normal app is asked through the helper's dialog; these answer Allow once. */
const dialogAllows = (count: number) => Array.from({ length: count }, () => ({ result: { answer: "allow" } }));

/** How often the user was asked: elicitations (a high-risk or unsigned app) and dialogs (a signed normal app). */
async function askedCount(harness: Harness): Promise<number> {
  const dialogs = ((await harness.requests()) as { method?: string }[]).filter((entry) => entry.method === "ask_user").length;
  return harness.elicitations.length + dialogs;
}

function connect(count: number, window: Record<string, unknown> = demo, signer = SIGNER, fileName = "demo.exe") {
  return connectServer({ elicitation: "form", script: { handlers: { get_window: windows(count, window), app_identity: identities(count, signer, fileName), ask_user: dialogAllows(count) } } });
}

function typeText(harness: Harness) {
  return harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "x" } });
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

/** Today's events in the case's log, of `kinds`. */
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
    .filter((event) => kinds.includes(event.event as string));
}

async function grantEvents(harness: Harness) {
  return (await logged(harness, "grant_invalid", "grant_key_lost")).map(({ time: _time, session: _session, ...rest }) => rest);
}

async function tiers(harness: Harness): Promise<unknown[]> {
  return (await logged(harness, "tool_call")).map((event) => (event.app as { tier?: unknown } | null)?.tier);
}

const fileVoid = (reason: string) => ({ event: "grant_invalid", entry: "file", key: null, reason });

test("a signed always-allow grant lets its app be operated without asking: approval channel grant", async () => {
  const harness = await connect(2);
  try {
    await writeGrants(harness, sealed(1, [sign(grantOf(DEMO_KEY))]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 0, "the user was not asked");
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "grant", answer: "allow_always" },
      { channel: "grant", answer: "allow_always" },
    ]);
    const macRequests = ((await harness.requests()) as { method?: string; params?: Record<string, unknown> }[]).filter((entry) => entry.method === "grant_mac");
    assert.ok(macRequests.length >= 1);
    // Ticket 95: the helper keeps its key in the folder it was started with (beside grants.json); no request names it.
    assert.equal(macRequests.some((entry) => entry.params !== undefined && "key_path" in entry.params), false);
    assert.equal(existsSync(join(harness.dir, "grants.key")), true, "the key is the one beside grants.json");
    // The helper gets the entries' fields to canonicalize, not text the server made.
    assert.deepEqual((macRequests[0]?.params?.entries as { kind: string }[]).map((entry) => entry.kind), ["grant", "file"]);
    assert.deepEqual(await grantEvents(harness), []);
  } finally {
    await harness.close();
  }
});

test("using a grant updates its last_used_at, signs it and the file again, with the next generation", async () => {
  const harness = await connect(1);
  try {
    await writeGrants(harness, sealed(4, [sign(grantOf(DEMO_KEY))]));
    const before = Date.now();
    assert.notEqual((await typeText(harness)).isError, true);
    const written = JSON.parse(await readFile(join(harness.dir, "grants.json"), "utf8")) as { version: number; generation: number; grants: Entry[]; raised: Entry[]; file_mac: string };
    assert.equal(written.version, 1);
    assert.equal(written.generation, 5);
    const [grant] = written.grants;
    assert.ok(grant && Date.parse(grant.last_used_at ?? "") >= before - 1000, JSON.stringify(grant));
    assert.equal(grant?.created_at, "2026-01-01T00:00:00.000Z");
    const { mac, ...fields } = grant;
    assert.equal(mac, sign(fields).mac, "signed again over the new last_used_at");
    assert.equal(written.file_mac, sealed(5, [grant]).file_mac, "the file signed again");
  } finally {
    await harness.close();
  }
  // A new server (a new session) still takes a grant without asking, and does not write it again within a minute.
  const next = await connect(1);
  try {
    const recent = new Date().toISOString();
    await writeGrants(next, sealed(1, [sign(grantOf(DEMO_KEY, { last_used_at: recent }))]));
    assert.notEqual((await typeText(next)).isError, true);
    assert.equal(await askedCount(next), 0);
    const kept = JSON.parse(await readFile(join(next.dir, "grants.json"), "utf8")) as { grants: Entry[] };
    assert.equal(kept.grants[0]?.last_used_at, recent, "used again within a minute: the file is not written again");
  } finally {
    await next.close();
  }
});

test("an entry edited under its old MAC is ignored and logged once as grant_invalid", async () => {
  const forged = { ...sign(grantOf(DEMO_KEY)), display_name: "Changed" };
  const harness = await connect(2);
  try {
    await writeGrants(harness, sealed(1, [forged]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1, "the user is asked");
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await grantEvents(harness), [{ event: "grant_invalid", entry: "grant", key: DEMO_KEY, reason: "bad_mac" }], "logged once while the file is unchanged");
    assert.deepEqual((await approvalAnswers(harness))[0], { channel: "dialog", answer: "allow_once" });
  } finally {
    await harness.close();
  }
});

test("an entry added by hand voids the whole file, with or without a MAC of its own", async () => {
  const original = sealed(3, [sign(grantOf(`signer:${SIGNER}|other.exe`))]);
  const copiedMac = { ...grantOf(DEMO_KEY), mac: original.grants[0]?.mac as string };
  for (const added of [grantOf(DEMO_KEY), copiedMac]) {
    const harness = await connect(1);
    try {
      await writeGrants(harness, { ...original, grants: [...original.grants, added] });
      assert.notEqual((await typeText(harness)).isError, true);
      assert.equal(await askedCount(harness), 1, "asked");
      assert.deepEqual(await grantEvents(harness), [fileVoid("bad_mac")]);
    } finally {
      await harness.close();
    }
  }
});

test("a file with entries but no file_mac is void: grant_invalid not_signed", async () => {
  const harness = await connect(1);
  try {
    await writeGrants(harness, { version: 1, generation: 1, grants: [sign(grantOf(DEMO_KEY))], raised: [] });
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1);
    assert.deepEqual(await grantEvents(harness), [fileVoid("not_signed")]);
  } finally {
    await harness.close();
  }
});

test("an empty file without file_mac is void too: it neither lowers a raise nor raises the generation", async () => {
  const harness = await connect(3);
  try {
    await writeGrants(harness, sealed(5, [], [sign(raisedOf(DEMO_KEY, "blocked"))]));
    assert.equal(code(await typeText(harness)), "blocked_app");
    // Written with a file tool: no entries, a newer generation, no seal.
    await writeGrants(harness, { version: 1, generation: 6 });
    assert.equal(code(await typeText(harness)), "blocked_app", "still blocked");
    assert.deepEqual(await grantEvents(harness), [fileVoid("not_signed")]);
    // A signed generation 7 is taken: the unsigned 6 did not move the mark.
    await writeGrants(harness, sealed(7, []));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1, "no longer raised: asked");
    assert.deepEqual(await grantEvents(harness), [fileVoid("not_signed")], "not a rollback");
  } finally {
    await harness.close();
  }
});

test("an unsigned file with a huge generation does not poison the mark: a later signed file and its write still count", async () => {
  const harness = await connect(2);
  try {
    await writeGrants(harness, { version: 1, generation: Number.MAX_SAFE_INTEGER });
    assert.notEqual((await typeText(harness)).isError, true);
    await writeGrants(harness, sealed(2, [sign(grantOf(DEMO_KEY))]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "dialog", answer: "allow_once" },
      { channel: "grant", answer: "allow_always" },
    ]);
    assert.deepEqual(await grantEvents(harness), [fileVoid("not_signed")]);
    const written = JSON.parse(await readFile(join(harness.dir, "grants.json"), "utf8")) as { generation: number };
    assert.equal(written.generation, 3, "the use was written as the next generation");
  } finally {
    await harness.close();
  }
});

test("a grant for an unsigned app (keyed by its path) is never used", async () => {
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: windows(1) } } });
  try {
    await writeGrants(harness, sealed(1, [sign(grantOf("c:\\apps\\demo.exe"))]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1);
    assert.deepEqual(await grantEvents(harness), [{ event: "grant_invalid", entry: "grant", key: "c:\\apps\\demo.exe", reason: "not_signed" }]);
  } finally {
    await harness.close();
  }
});

test("a display name with a control character, U+2028 and a character outside the BMP is signed in the pinned canonical form", async () => {
  const odd = "a\u0001b\u2028c\u{1F600}";
  const grant = grantOf(DEMO_KEY, { display_name: odd });
  // The same text helper/tests/grants_key.rs pins for the real helper: \u0001 escaped, U+2028 and the emoji as UTF-8.
  const pinned = `{"created_at":"2026-01-01T00:00:00.000Z","display_name":"a\\u0001b\u2028c\u{1F600}","key":${JSON.stringify(DEMO_KEY)},"last_used_at":"2026-01-01T00:00:00.000Z","path":"C:\\\\Apps\\\\Demo.exe"}`;
  assert.equal(canonical(grant), pinned);
  const harness = await connect(1);
  try {
    await writeGrants(harness, sealed(1, [{ ...grant, mac: hmac(pinned) }]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 0, "the MAC over the pinned text verifies");
  } finally {
    await harness.close();
  }
});

test("a key that cannot be unlocked voids every entry: grant_key_lost, a new key, and the user is asked", async () => {
  const harness = await connect(2);
  try {
    await writeGrants(harness, sealed(1, [sign(grantOf(DEMO_KEY))], [sign(raisedOf(`signer:${SIGNER}|other.exe`, "blocked"))]), { key: false });
    await writeFile(join(harness.dir, "grants.key"), "not a key this helper can unlock");
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1, "asked: the grant is void");
    assert.deepEqual(await grantEvents(harness), [{ event: "grant_key_lost", entries: 2 }]);
    assert.notEqual(await readFile(join(harness.dir, "grants.key"), "utf8"), "not a key this helper can unlock", "a new key was made");
    const everything = JSON.stringify(await logged(harness, "tool_call", "approval", "grant_invalid", "grant_key_lost"));
    assert.equal(everything.includes(FAKE_GRANT_KEY), false, "the key is not in the log");
  } finally {
    await harness.close();
  }
});

test("a missing key with entries in the file is a lost key too", async () => {
  const harness = await connect(1);
  try {
    await writeGrants(harness, sealed(1, [sign(grantOf(DEMO_KEY))]), { key: false });
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1);
    assert.deepEqual(await grantEvents(harness), [{ event: "grant_key_lost", entries: 1 }]);
  } finally {
    await harness.close();
  }
});

test("a key found new while a used grant is written: nothing is signed under it, the reading is void", async () => {
  const grant = sign(grantOf(DEMO_KEY));
  const file = sealed(1, [grant]);
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: windows(2),
        app_identity: identities(2),
        ask_user: dialogAllows(2),
        // The check's reading verifies; the last_used_at write then finds the key made again.
        grant_mac: [{ result: { macs: [grant.mac, file.file_mac], key_state: "ok" } }, { result: { macs: [grant.mac], key_state: "lost_recreated" } }],
      },
    },
  });
  try {
    await writeGrants(harness, file);
    const written = await readFile(join(harness.dir, "grants.json"), "utf8");
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await readFile(join(harness.dir, "grants.json"), "utf8"), written, "the old entries were not signed again under the new key");
    assert.deepEqual(await grantEvents(harness), [{ event: "grant_key_lost", entries: 1 }]);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1, "the grant is void from then on");
    const methods = ((await harness.requests()) as { method?: string }[]).map((entry) => entry.method).filter((method) => method === "grant_mac");
    assert.equal(methods.length, 2, "the voided reading is kept while the file is unchanged");
  } finally {
    await harness.close();
  }
});

test("an edit to grants.json takes effect at the next approval check of the running server", async () => {
  const harness = await connect(4);
  try {
    await writeGrants(harness, sealed(1, [sign(grantOf(DEMO_KEY, { last_used_at: new Date().toISOString() }))]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 0);
    // Revoked (as the control panel would): asked at the next check.
    await writeGrants(harness, sealed(2, []));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1);
    // Raised to blocked: refused at the next check, although allowed for this session.
    await writeGrants(harness, sealed(3, [], [sign(raisedOf(DEMO_KEY, "blocked"))]));
    const blocked = await typeText(harness);
    assert.equal(code(blocked), "blocked_app");
    assert.match(textContent(blocked), /blocked tier/i);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "grant", answer: "allow_always" },
      { channel: "dialog", answer: "allow_once" },
      { channel: null, answer: "blocked" },
    ]);
  } finally {
    await harness.close();
  }
});

test("deleting a raise from the file does not lower the app: the file is void and the raise last verified stands", async () => {
  const raised = sign(raisedOf(DEMO_KEY, "blocked"));
  const harness = await connect(2);
  try {
    const file = sealed(2, [], [raised]);
    await writeGrants(harness, file);
    assert.equal(code(await typeText(harness)), "blocked_app");
    await writeGrants(harness, { ...file, raised: [] });
    assert.equal(code(await typeText(harness)), "blocked_app", "still blocked");
    assert.equal(await askedCount(harness), 0);
    assert.deepEqual(await grantEvents(harness), [fileVoid("bad_mac")]);
  } finally {
    await harness.close();
  }
});

test("an older copy of the file, or none, after a newer one is a rollback: void, and the raise stands", async () => {
  const raised = sign(raisedOf(DEMO_KEY, "high"));
  const harness = await connect(4);
  try {
    // Generation 3 had a grant; generation 5 raised the app and dropped it.
    const older = sealed(3, [sign(grantOf(DEMO_KEY))]);
    await writeGrants(harness, sealed(5, [], [raised]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await tiers(harness), ["high"]);
    await writeGrants(harness, older);
    assert.notEqual((await typeText(harness)).isError, true);
    // The file removed altogether, then an older empty one put back.
    await rm(join(harness.dir, "grants.json"));
    assert.notEqual((await typeText(harness)).isError, true);
    await writeGrants(harness, sealed(1, []));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await tiers(harness), ["high", "high", "high", "high"], "the old grant is not used and the raise stands");
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "elicitation", answer: "allow_once" },
      { channel: "session", answer: "allow_once" },
      { channel: "session", answer: "allow_once" },
      { channel: "session", answer: "allow_once" },
    ]);
    assert.deepEqual(await grantEvents(harness), [fileVoid("rollback"), fileVoid("rollback"), fileVoid("rollback")]);
  } finally {
    await harness.close();
  }
});

test("a raised tier applies, and makes the app's always-allow grant void", async () => {
  const harness = await connect(2);
  try {
    await writeGrants(harness, sealed(1, [sign(grantOf(DEMO_KEY))], [sign(raisedOf(DEMO_KEY, "high"))]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal(await askedCount(harness), 1, "asked: the grant does not count once the app is raised");
    assert.deepEqual(await tiers(harness), ["high"]);
    assert.deepEqual(await grantEvents(harness), []);
  } finally {
    await harness.close();
  }
});

test("when the helper cannot sign, no grant counts but the raises last verified still do", async () => {
  const raised = sign(raisedOf(DEMO_KEY, "blocked"));
  const file = sealed(1, [], [raised]);
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: windows(2),
        app_identity: identities(2),
        grant_mac: [{ result: { macs: [raised.mac, file.file_mac], key_state: "ok" } }, { error: { code: "internal", message: "A grant MAC could not be computed." } }],
      },
    },
  });
  try {
    await writeGrants(harness, file);
    assert.equal(code(await typeText(harness)), "blocked_app");
    // Changed (a grant added), but the helper fails to verify it: the app stays blocked.
    await writeGrants(harness, sealed(2, [sign(grantOf(DEMO_KEY))], [raised]));
    assert.equal(code(await typeText(harness)), "blocked_app");
    assert.equal(await askedCount(harness), 0);
  } finally {
    await harness.close();
  }
});

test("a raised entry that would not raise the built-in tier is ignored and logged", async () => {
  const edge = { ...demo, app: EDGE, app_name: "Edge" };
  const edgeKey = `signer:${MICROSOFT}|msedge.exe`;
  const harness = await connect(2, edge, MICROSOFT, "msedge.exe");
  try {
    await writeGrants(harness, sealed(1, [], [sign(raisedOf(edgeKey, "normal"))]));
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await tiers(harness), ["high", "high"], "still high-risk");
    assert.deepEqual(await grantEvents(harness), [{ event: "grant_invalid", entry: "raised", key: edgeKey, reason: "not_higher" }]);
  } finally {
    await harness.close();
  }
});

test("a grants.json that cannot be read as one is no grants, logged; the tool goes on", async () => {
  const contents = [
    "{ not json",
    JSON.stringify({ ...sealed(1, [sign(grantOf(DEMO_KEY))]), version: 2 }),
    JSON.stringify([1, 2]),
    JSON.stringify({ ...sealed(1, [sign(grantOf(DEMO_KEY))]), extra: "field" }),
    JSON.stringify({ ...sealed(1, [sign(grantOf(DEMO_KEY))]), generation: 1.5 }),
    JSON.stringify(sealed(-1, [sign(grantOf(DEMO_KEY))])),
    JSON.stringify(sealed(2 ** 53, [sign(grantOf(DEMO_KEY))])),
    // Over 1 MB but otherwise a valid, signed file: not read at all.
    JSON.stringify(sealed(1, [sign(grantOf(DEMO_KEY, { display_name: "x".repeat(1024 * 1024) }))])),
  ];
  for (const content of contents) {
    const harness = await connect(1);
    try {
      await writeGrants(harness, content);
      assert.notEqual((await typeText(harness)).isError, true, content.slice(0, 80));
      assert.equal(await askedCount(harness), 1, content.slice(0, 80));
      assert.deepEqual(await grantEvents(harness), [fileVoid("unreadable")], content.slice(0, 80));
      const methods = ((await harness.requests()) as { method?: string }[]).map((entry) => entry.method);
      assert.equal(methods.includes("grant_mac"), false, content.slice(0, 80));
    } finally {
      await harness.close();
    }
  }
});

test("without a grants.json nothing is asked of the helper's key", async () => {
  const harness = await connect(1);
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    const methods = ((await harness.requests()) as { method?: string }[]).map((entry) => entry.method);
    assert.equal(methods.includes("grant_mac"), false, methods.join(", "));
    assert.deepEqual(await grantEvents(harness), []);
  } finally {
    await harness.close();
  }
});

test("no MCP tool writes grants: the tool list has none for grants or tiers", async () => {
  const harness = await connect(0);
  try {
    const { tools } = await harness.client.listTools();
    for (const tool of tools) {
      assert.doesNotMatch(`${tool.name} ${JSON.stringify(tool.inputSchema)}`, /grant|tier_raise|allow_always/i, tool.name);
    }
  } finally {
    await harness.close();
  }
});

test("the fake helper refuses a grants folder outside the test folders, or none, and writes no key there", async () => {
  // The user's own LOCALAPPDATA, as test/isolate-localappdata.mjs kept it (ticket 100); the one tests see is temporary.
  const local = process.env.COMPUTER_USE_TEST_REAL_LOCALAPPDATA || process.env.LOCALAPPDATA || "C:\\Users\\Default\\AppData\\Local";
  const keyPath = join(local, "deskpass", "grants.key");
  const existed = existsSync(keyPath);
  const outside = await launchScriptedHelper({}, {}, ["--grants-dir", join(local, "deskpass")]);
  const unset = await launchScriptedHelper();
  try {
    for (const helper of [outside.helper, unset.helper]) {
      await assert.rejects(helper.request("grant_mac", { entries: [] }), (error: { code?: unknown }) => error.code === "internal");
    }
    // Ticket 95: no request names the key file.
    await assert.rejects(outside.helper.request("grant_mac", { key_path: keyPath, entries: [] }), (error: { code?: unknown }) => error.code === "invalid_params");
    assert.equal(existsSync(keyPath), existed, "nothing was written there");
  } finally {
    await outside.close();
    await unset.close();
  }
});

// Ticket 115 (follow-up 122): `GrantStore.removeRaise(mac, key, expected)` removes the user's raise
// only while it is still the tier the user confirmed removing. Seam: the store itself over a case's
// folder, its MACs made here with the fake helper's key; no helper is started.

const NOTES_KEY = "c:\\apps\\notes.exe";

/** A GrantStore over a temporary folder holding `content` as grants.json (none when undefined). */
async function removeRaiseCase(content?: unknown) {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const dir = await mkdtemp(join(root, "remove-raise-"));
  await writeFile(join(dir, "grants.key"), FAKE_GRANT_KEY);
  if (content !== undefined) await writeFile(join(dir, "grants.json"), typeof content === "string" ? content : JSON.stringify(content));
  const mac: GrantMac = async ({ entries }: { entries: MacEntry[] }) => ({ macs: entries.map((entry) => hmac(canonical(entry.fields as unknown as Record<string, unknown>))), key_state: "ok" });
  const store = new GrantStore({ dir, record: async () => undefined });
  const read = () => readFile(join(dir, "grants.json"), "utf8");
  const raisedKeys = async () => (JSON.parse(await read()) as { raised: Entry[] }).raised.map((entry) => `${entry.key}=${entry.tier}`);
  return { dir, store, mac, read, raisedKeys, done: () => rm(dir, { recursive: true, force: true }) };
}

test("removeRaise removes the raise when it is exactly the tier expected, and only that app's, signing the file again with the next generation", async () => {
  const c = await removeRaiseCase(sealed(3, [sign(grantOf(DEMO_KEY))], [sign(raisedOf(NOTES_KEY, "high")), sign(raisedOf(DEMO_KEY, "blocked"))]));
  try {
    assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY, "high"), true);
    assert.deepEqual(await c.raisedKeys(), [`${DEMO_KEY}=blocked`]);
    const written = JSON.parse(await c.read()) as { generation: number; grants: Entry[] };
    assert.equal(written.generation, 4);
    assert.equal(written.grants.length, 1, "the grant is untouched");
    const view = await c.store.current(c.mac);
    assert.deepEqual(view.verdict(), { state: "valid", ignored: 0 }, "the file written verifies whole");
    assert.equal((await view.tierFor(NOTES_KEY, { tier: "normal", why: null })).tier, "normal", "back to the built-in tier");
    assert.equal((await view.tierFor(DEMO_KEY, { tier: "normal", why: null })).tier, "blocked");
  } finally {
    await c.done();
  }
});

test("removeRaise of a blocked raise expected blocked removes it; without an expected tier it removes whatever the app has", async () => {
  const c = await removeRaiseCase(sealed(1, [], [sign(raisedOf(NOTES_KEY, "blocked"))]));
  try {
    assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY, "blocked"), true);
    assert.deepEqual(await c.raisedKeys(), []);
  } finally {
    await c.done();
  }
  const any = await removeRaiseCase(sealed(1, [], [sign(raisedOf(NOTES_KEY, "high"))]));
  try {
    assert.equal(await any.store.removeRaise(any.mac, NOTES_KEY), true);
    assert.deepEqual(await any.raisedKeys(), []);
  } finally {
    await any.done();
  }
});

test("removeRaise writes nothing and answers false when the raise is another tier than expected", async () => {
  for (const [held, expected] of [["blocked", "high"], ["high", "blocked"]] as const) {
    const c = await removeRaiseCase(sealed(1, [], [sign(raisedOf(NOTES_KEY, held))]));
    try {
      const before = await c.read();
      assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY, expected), false, `${held} held, ${expected} expected`);
      assert.equal(await c.read(), before, "the file is not rewritten");
    } finally {
      await c.done();
    }
  }
});

test("removeRaise writes nothing and answers false when a stricter entry of the app exists next to the expected one", async () => {
  const c = await removeRaiseCase(sealed(1, [], [sign(raisedOf(NOTES_KEY, "high")), sign(raisedOf(NOTES_KEY, "blocked"))]));
  try {
    const before = await c.read();
    assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY, "high"), false);
    assert.equal(await c.read(), before);
    // Expecting the stricter one removes both: nothing is stricter than blocked.
    assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY, "blocked"), true);
    assert.deepEqual(await c.raisedKeys(), []);
  } finally {
    await c.done();
  }
});

test("removeRaise writes nothing and answers false when the app has no raise, with or without a file", async () => {
  const c = await removeRaiseCase(sealed(1, [sign(grantOf(NOTES_KEY))], [sign(raisedOf(DEMO_KEY, "high"))]));
  try {
    const before = await c.read();
    assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY, "high"), false);
    assert.equal(await c.store.removeRaise(c.mac, NOTES_KEY), false);
    assert.equal(await c.read(), before);
  } finally {
    await c.done();
  }
  const none = await removeRaiseCase();
  try {
    assert.equal(await none.store.removeRaise(none.mac, NOTES_KEY, "high"), false);
    assert.equal(existsSync(join(none.dir, "grants.json")), false, "no file is made");
  } finally {
    await none.done();
  }
});

test("removeRaise leaves a file that does not verify as it is and refuses with GrantFileVoidError, like the other settings writes", async () => {
  const file = sealed(1, [], [sign(raisedOf(NOTES_KEY, "high")), sign(raisedOf(DEMO_KEY, "high"))]);
  const tampered = JSON.stringify({ ...file, raised: [...file.raised].reverse() });
  const noMac = JSON.stringify({ ...file, file_mac: undefined });
  for (const content of [tampered, noMac, "{ not json"]) {
    const c = await removeRaiseCase(content);
    try {
      await assert.rejects(c.store.removeRaise(c.mac, NOTES_KEY, "high"), GrantFileVoidError, content.slice(0, 40));
      assert.equal(await c.read(), content, "the void file is left for the user");
    } finally {
      await c.done();
    }
  }
});

test("removeRaise refuses a key that is not an app's", async () => {
  const c = await removeRaiseCase(sealed(1, [], [sign(raisedOf(NOTES_KEY, "high"))]));
  try {
    await assert.rejects(c.store.removeRaise(c.mac, "", "high"), RangeError);
    assert.deepEqual(await c.raisedKeys(), [`${NOTES_KEY}=high`]);
  } finally {
    await c.done();
  }
});
