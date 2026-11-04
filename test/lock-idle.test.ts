import assert from "node:assert/strict";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { LOCK_IDLE_MS } from "../src/config.ts";
import { SessionLock, type ProcessStart } from "../src/lock.ts";
import { connectServer, repoRoot, textContent } from "./harness.ts";

// Ticket 135: a session that makes no tool call for LOCK_IDLE_MS releases the lock, and takes it again on its next call.

const demo = {
  window_id: "5",
  app: "C:\\Apps\\demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 80,
  height: 40,
  pid: 4242,
};

const captured = {
  image_base64: "QQ==",
  original_width: 80,
  original_height: 40,
  window_x: 0,
  window_y: 0,
  window_width: 80,
  window_height: 40,
};

const first = { pid: 70001, startedAt: "2026-09-24T01:00:00.000Z" };
const second = { pid: 70002, startedAt: "2026-09-24T02:00:00.000Z" };
const IDLE_MS = 150;

const screenshots = (count: number, delayMs?: number[]) => ({
  get_window: Array.from({ length: count }, () => ({ result: demo })),
  capture_window: Array.from({ length: count }, (_, index) => ({ result: captured, ...(delayMs?.[index] ? { delayMs: delayMs[index] } : {}) })),
});

async function lockDirFor(name: string): Promise<string> {
  const dir = join(repoRoot(), "test-tmp", `lock-idle-${Date.now()}-${name}`);
  await mkdir(dir, { recursive: true });
  return dir;
}

async function readLock(lockDir: string): Promise<Record<string, unknown> | null> {
  try {
    return JSON.parse(await readFile(join(lockDir, "session.lock"), "utf8")) as Record<string, unknown>;
  } catch {
    return null;
  }
}

async function waitFor(condition: () => Promise<boolean>, what: string, limitMs = 5_000): Promise<void> {
  const until = Date.now() + limitMs;
  while (!(await condition())) {
    if (Date.now() > until) assert.fail(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

function methodsOf(requests: unknown[]): string[] {
  return requests.map((entry) => String((entry as { method?: string }).method));
}

/** The local time `iso` is shown as in the in_use message. */
function localTime(iso: string): string {
  const date = new Date(iso);
  const two = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
}

test("the idle window is 2 minutes", () => {
  assert.equal(LOCK_IDLE_MS, 120_000);
});

test("an idle session releases the lock as its exit does (overlay and Esc hook ended) and keeps its helper", async () => {
  const lockDir = await lockDirFor("release");
  const harness = await connectServer({ elicitation: "form", lockDir, owner: first, lockIdleMs: IDLE_MS, overlayIdleMs: 60_000, script: { handlers: screenshots(1) } });
  try {
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(shot.isError, true, textContent(shot));
    const held = await readLock(lockDir);
    assert.equal(held?.pid, first.pid);
    assert.equal(held?.startedAt, first.startedAt);
    assert.equal(typeof held?.lastUsedAt, "string");
    await waitFor(async () => (await readLock(lockDir)) === null, "the idle release");
    const methods = methodsOf(await harness.requests());
    assert.ok(methods.includes("overlay_hide"), methods.join(" "));
    assert.ok(methods.includes("session_end"), methods.join(" "));
    assert.ok(methods.lastIndexOf("session_end") > methods.lastIndexOf("capture_window"), methods.join(" "));
    // The helper stays: it was started (pinged) once.
    assert.equal(methods.filter((method) => method === "ping").length, 1, methods.join(" "));
    assert.deepEqual(await readdir(lockDir), []);
  } finally {
    await harness.close();
  }
});

test("a running computer_wait or computer_batch is never cut short, and the idle window runs from the end of the last call", async () => {
  const lockDir = await lockDirFor("running");
  // Wider than IDLE_MS: the gap between two calls of this test must stay inside the window on a busy machine.
  const idleMs = 3_000;
  const harness = await connectServer({ elicitation: "form", lockDir, owner: first, lockIdleMs: idleMs, overlayIdleMs: 60_000, script: { handlers: screenshots(2, [0, 2 * idleMs]) } });
  try {
    assert.notEqual((await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } })).isError, true);
    const before = Date.parse(String((await readLock(lockDir))?.lastUsedAt));
    const waited = await harness.client.callTool({ name: "computer_wait", arguments: { seconds: (2 * idleMs) / 1000 } });
    assert.notEqual(waited.isError, true, textContent(waited));
    assert.equal((await readLock(lockDir))?.pid, first.pid, "released during computer_wait");
    // Written just after the result (not waited for): the end of the call is recorded.
    await waitFor(async () => Date.parse(String((await readLock(lockDir))?.lastUsedAt)) > before, "lastUsedAt updated at the end of the call");
    const batch = await harness.client.callTool({ name: "computer_batch", arguments: { steps: [{ action: "screenshot", window_id: "5" }] } });
    assert.notEqual(batch.isError, true, textContent(batch));
    assert.equal((await readLock(lockDir))?.pid, first.pid, "released during computer_batch");
    assert.ok(!methodsOf(await harness.requests()).includes("session_end"), "the session was ended while a call ran");
    await waitFor(async () => (await readLock(lockDir)) === null, "the idle release after the batch");
  } finally {
    await harness.close();
  }
});

test("after an idle release the same session takes the lock again on its next call, without asking again", async () => {
  const lockDir = await lockDirFor("again");
  const harness = await connectServer({ elicitation: "form", lockDir, owner: first, lockIdleMs: IDLE_MS, overlayIdleMs: 60_000, script: { handlers: screenshots(2) } });
  try {
    assert.notEqual((await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } })).isError, true);
    await waitFor(async () => (await readLock(lockDir)) === null, "the idle release");
    const again = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(again.isError, true, textContent(again));
    const record = await readLock(lockDir);
    assert.equal(record?.pid, first.pid);
    assert.equal(record?.startedAt, first.startedAt);
    assert.equal(harness.elicitations.length, 1, "the approval of this session was asked again");
    const methods = methodsOf(await harness.requests());
    assert.equal(methods.filter((method) => method === "session_start").length, 2, methods.join(" "));
  } finally {
    await harness.close();
  }
});

test("after an idle release, a call while another session holds the lock is in_use and says who holds it and since when", async () => {
  const lockDir = await lockDirFor("other");
  const idle = await connectServer({
    elicitation: "form",
    lockDir,
    owner: first,
    lockIdleMs: IDLE_MS,
    overlayIdleMs: 60_000,
    script: { handlers: { ...screenshots(2), process_info: [{ result: { alive: true, started_ms: Date.parse(second.startedAt) } }] } },
  });
  const other = await connectServer({ elicitation: "form", lockDir, owner: second, overlayIdleMs: 60_000, script: { handlers: screenshots(1) } });
  try {
    assert.notEqual((await idle.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } })).isError, true);
    await waitFor(async () => (await readLock(lockDir)) === null, "the idle release");
    const taken = await other.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(taken.isError, true, textContent(taken));
    const blocked = await idle.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(blocked.isError, true);
    const text = textContent(blocked);
    assert.equal(text.split(/\r?\n/)[0], "in_use");
    assert.ok(text.includes(second.startedAt), text);
    assert.ok(text.includes(localTime(second.startedAt)), text);
    assert.match(text, /last tool call ended \d+ seconds? ago/);
    assert.match(text, /2 minutes/);
    assert.match(text, /Report this to the user/);
    assert.equal((await readLock(lockDir))?.pid, second.pid);
  } finally {
    await idle.close();
    await other.close();
  }
});

/** A lock file of `record`, and a SessionLock of `second` whose probe says `first` is running, at time `nowMs`. */
async function contender(name: string, record: Record<string, unknown>, nowMs: number) {
  const lockDir = await lockDirFor(name);
  await writeFile(join(lockDir, "session.lock"), JSON.stringify(record));
  const probe = async (pid: number): Promise<ProcessStart> => (pid === first.pid ? { state: "running", startedAt: new Date(first.startedAt) } : { state: "missing" });
  return { lockDir, lock: new SessionLock(lockDir, probe, second, { now: () => nowMs }) };
}

test("the in_use message names the holder's start (local time and ISO), its last use, and the automatic release", async () => {
  const now = Date.parse("2026-09-24T01:30:00.000Z");
  const { lock } = await contender("message", { ...first, lastUsedAt: new Date(now - 45_000).toISOString() }, now);
  const outcome = await lock.acquire();
  assert.equal(outcome.acquired, false);
  const message = outcome.acquired ? "" : outcome.message;
  assert.ok(message.includes(first.startedAt), message);
  assert.ok(message.includes(`${localTime(first.startedAt)} local time`), message);
  assert.ok(message.includes(String(first.pid)), message);
  assert.ok(message.includes("last tool call ended 45 seconds ago"), message);
  assert.ok(message.includes("makes no tool call for 2 minutes"), message);
  assert.ok(message.includes("in about 1 minute 15 seconds"), message);
  assert.match(message, /Report this to the user/);
});

test("a holder idle past the window is said to be about to release it", async () => {
  const now = Date.parse("2026-09-24T01:30:00.000Z");
  const { lock } = await contender("overdue", { ...first, lastUsedAt: new Date(now - 3 * 60_000 - 5_000).toISOString() }, now);
  const outcome = await lock.acquire();
  const message = outcome.acquired ? "" : outcome.message;
  assert.ok(message.includes("last tool call ended 3 minutes 5 seconds ago"), message);
  assert.match(message, /shortly/);
});

test("an old-format lock (no lastUsedAt) of a live holder is still in_use, and the message says its last use is not known", async () => {
  const now = Date.parse("2026-09-24T01:30:00.000Z");
  const { lock, lockDir } = await contender("old-format", first, now);
  const outcome = await lock.acquire();
  assert.equal(outcome.acquired, false);
  const message = outcome.acquired ? "" : outcome.message;
  assert.ok(message.includes(first.startedAt), message);
  assert.match(message, /does not say when it last made a tool call/);
  assert.match(message, /Report this to the user/);
  assert.deepEqual(await readLock(lockDir), first);
});

test("an old-format lock whose holder is gone is taken over", async () => {
  const lockDir = await lockDirFor("old-format-gone");
  await writeFile(join(lockDir, "session.lock"), JSON.stringify(first));
  const now = Date.parse("2026-09-24T03:00:00.000Z");
  const lock = new SessionLock(lockDir, async () => ({ state: "missing" }), second, { now: () => now });
  assert.deepEqual(await lock.acquire(), { acquired: true });
  assert.deepEqual(await readLock(lockDir), { ...second, lastUsedAt: new Date(now).toISOString() });
});

test("touch rewrites lastUsedAt of our own lock in place and leaves no other file", async () => {
  const lockDir = await lockDirFor("touch");
  let now = Date.parse("2026-09-24T03:00:00.000Z");
  const lock = new SessionLock(lockDir, async () => ({ state: "missing" }), second, { now: () => now });
  assert.deepEqual(await lock.acquire(), { acquired: true });
  now += 30_000;
  await lock.touch();
  assert.deepEqual(await readLock(lockDir), { ...second, lastUsedAt: new Date(now).toISOString() });
  assert.deepEqual(await readdir(lockDir), ["session.lock"]);
  assert.equal(lock.isHeld(), true);
});

test("touch does not overwrite a lock file that names another session, and the lock counts as lost", async () => {
  const lockDir = await lockDirFor("touch-other");
  const lock = new SessionLock(lockDir, async () => ({ state: "missing" }), second, { now: () => Date.now() });
  assert.deepEqual(await lock.acquire(), { acquired: true });
  await writeFile(join(lockDir, "session.lock"), JSON.stringify(first));
  await lock.touch();
  assert.deepEqual(await readLock(lockDir), first);
  assert.equal(lock.isHeld(), false);
  assert.deepEqual(await readdir(lockDir), ["session.lock"]);
});

test("release after an idle release is a no-op and leaves another session's lock alone", async () => {
  const lockDir = await lockDirFor("release-twice");
  const lock = new SessionLock(lockDir, async () => ({ state: "missing" }), second);
  assert.deepEqual(await lock.acquire(), { acquired: true });
  await lock.release();
  await writeFile(join(lockDir, "session.lock"), JSON.stringify(first));
  await lock.release();
  assert.deepEqual(await readLock(lockDir), first);
});
