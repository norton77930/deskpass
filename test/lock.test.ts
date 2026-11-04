import assert from "node:assert/strict";
import { mkdir, readdir, readFile, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { SessionLock, type ProcessStart } from "../src/lock.ts";
import { connectServer, repoRoot, textContent, type FakeHandler } from "./harness.ts";

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

// Session identities are made up: the (fake) helper answers every process question.
const first = { pid: 70001, startedAt: "2026-09-24T01:00:00.000Z" };
const second = { pid: 70002, startedAt: "2026-09-24T02:00:00.000Z" };

const screenshotHandlers = (count = 1) => ({
  get_window: Array.from({ length: count }, () => ({ result: demo })),
  capture_window: Array.from({ length: count }, () => ({ result: captured })),
});

async function lockDirFor(name: string): Promise<string> {
  const dir = join(repoRoot(), "test-tmp", `lock-${Date.now()}-${name}`);
  await mkdir(dir, { recursive: true });
  return dir;
}

/** The lock folder's entries once no temporary file of a lastUsedAt write is left, or as they are after 2 seconds. */
async function settledEntries(dir: string): Promise<string[]> {
  const deadline = Date.now() + 2_000;
  let entries = await readdir(dir);
  while (entries.some((name) => name.endsWith(".tmp")) && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
    entries = await readdir(dir);
  }
  return entries;
}

/** The holder a lock file names; its `lastUsedAt` (ticket 135) must be a time. */
function holderOf(raw: string): { pid: unknown; startedAt: unknown } {
  const { pid, startedAt, lastUsedAt, ...rest } = JSON.parse(raw) as Record<string, unknown>;
  assert.ok(typeof lastUsedAt === "string" && !Number.isNaN(Date.parse(lastUsedAt)), `lastUsedAt is not a time: ${raw}`);
  assert.deepEqual(rest, {});
  return { pid, startedAt };
}

function methodsOf(requests: unknown[]): string[] {
  return requests.map((entry) => String((entry as { method?: string }).method));
}

/** A lock file left by `holder`, and a session whose helper answers `answer` about that holder. */
async function contend(name: string, answer: FakeHandler) {
  const lockDir = await lockDirFor(name);
  const lockFile = join(lockDir, "session.lock");
  await writeFile(lockFile, JSON.stringify(first));
  const harness = await connectServer({
    elicitation: "form",
    lockDir,
    owner: second,
    script: { handlers: { ...screenshotHandlers(), process_info: [answer] } },
  });
  const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
  const asked = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "process_info") as Array<{ params?: { pid?: number } }>;
  return { harness, result, lockFile, asked };
}

test("a second session is told the computer is in use and only asks the helper about the holder", async () => {
  const lockDir = await lockDirFor("race");
  const holder = await connectServer({
    elicitation: "form",
    lockDir,
    owner: first,
    script: { handlers: screenshotHandlers() },
  });
  const other = await connectServer({
    elicitation: "form",
    lockDir,
    owner: second,
    script: {
      handlers: {
        ...screenshotHandlers(),
        list_windows: [{ result: { windows: [demo] } }],
        process_info: [{ result: { alive: true, started_ms: Date.parse(first.startedAt) } }],
      },
    },
  });
  try {
    const acquired = await holder.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(acquired.isError, true);
    const blocked = await other.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(blocked.isError, true);
    assert.equal(textContent(blocked).split(/\r?\n/)[0], "in_use");
    assert.equal(other.elicitations.length, 0);
    // Starting the helper for the holder check sends its version ping; nothing else is sent.
    const requests = (await other.requests()).filter((entry) => (entry as { method?: string }).method !== "ping");
    assert.deepEqual(methodsOf(requests), ["process_info"]);
    assert.equal((requests[0] as { params?: { pid?: number } }).params?.pid, first.pid);
    const listed = await other.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(listed.isError, true);
    const waited = await other.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
    assert.notEqual(waited.isError, true);
  } finally {
    await holder.close();
    await other.close();
  }
});

test("the lock can be taken after the holder closes", async () => {
  const lockDir = await lockDirFor("release");
  const holder = await connectServer({ elicitation: "form", lockDir, owner: first, script: { handlers: screenshotHandlers() } });
  try {
    assert.notEqual((await holder.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } })).isError, true);
  } finally {
    await holder.close();
  }
  const next = await connectServer({ elicitation: "form", lockDir, owner: second, script: { handlers: screenshotHandlers() } });
  try {
    const result = await next.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(result.isError, true);
    assert.equal(next.elicitations.length, 1);
  } finally {
    await next.close();
  }
});

test("a lock whose holder process no longer exists is taken over", async () => {
  const { harness, result, lockFile, asked } = await contend("gone", { result: { alive: false } });
  try {
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal(asked[0]?.params?.pid, first.pid);
    assert.deepEqual(holderOf(await readFile(lockFile, "utf8")), second);
  } finally {
    await harness.close();
  }
});

test("a lock whose holder pid now belongs to a process started at another time is taken over", async () => {
  const { harness, result, lockFile } = await contend("reused", { result: { alive: true, started_ms: Date.parse(first.startedAt) + 60_000 } });
  try {
    assert.notEqual(result.isError, true, textContent(result));
    assert.deepEqual(holderOf(await readFile(lockFile, "utf8")), second);
  } finally {
    await harness.close();
  }
});

test("a running holder whose start time cannot be read keeps the lock, and the message says why and where the lock file is", async () => {
  const { harness, result, lockFile } = await contend("unknown", { result: { alive: true, started_ms: null, reason: "Access is denied." } });
  try {
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "in_use");
    assert.match(text, /Access is denied\./);
    assert.ok(text.includes(lockFile), `the message does not name the lock file: ${text}`);
    assert.ok(text.includes(String(first.pid)), text);
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual(JSON.parse(await readFile(lockFile, "utf8")), first);
  } finally {
    await harness.close();
  }
});

test("when the helper cannot answer about the holder, the call fails with the helper's error and the lock is left alone", async () => {
  const { harness, result, lockFile } = await contend("helper-error", { error: { code: "internal", message: "process_info broke" } });
  try {
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split(/\r?\n/)[0], "internal");
    assert.deepEqual(JSON.parse(await readFile(lockFile, "utf8")), first);
  } finally {
    await harness.close();
  }
});

test("a lock file that was just created and is still empty is not taken over", async () => {
  const lockDir = await lockDirFor("empty-fresh");
  const lockFile = join(lockDir, "session.lock");
  await writeFile(lockFile, "");
  const harness = await connectServer({ elicitation: "form", lockDir, owner: second, script: { handlers: screenshotHandlers() } });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "in_use");
    assert.ok(text.includes(lockFile), `the message does not name the lock file: ${text}`);
    assert.equal(harness.elicitations.length, 0);
    assert.equal(await readFile(lockFile, "utf8"), "");
  } finally {
    await harness.close();
  }
});

test("a lock file that is unreadable and old is taken over", async () => {
  const lockDir = await lockDirFor("corrupt-old");
  const lockFile = join(lockDir, "session.lock");
  await writeFile(lockFile, "{\"pid\":");
  const longAgo = new Date(Date.now() - 60_000);
  await utimes(lockFile, longAgo, longAgo);
  const harness = await connectServer({ elicitation: "form", lockDir, owner: second, script: { handlers: screenshotHandlers() } });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(result.isError, true, textContent(result));
    assert.deepEqual(holderOf(await readFile(lockFile, "utf8")), second);
    // Ticket 135: lastUsedAt is written after the call returns, through a temporary file renamed over the lock.
    assert.deepEqual(await settledEntries(lockDir), ["session.lock"]);
  } finally {
    await harness.close();
  }
});

test("a session that judged a lock stale does not delete the lock another session took over meanwhile", async () => {
  const lockDir = await lockDirFor("takeover-race");
  const lockFile = join(lockDir, "session.lock");
  await writeFile(lockFile, JSON.stringify(first));
  const third = { pid: 70003, startedAt: "2026-09-24T03:00:00.000Z" };
  // While this session asks about the stale holder, a faster session removes it and takes the lock.
  const probe = async (pid: number): Promise<ProcessStart> => {
    if (pid === first.pid) {
      await writeFile(lockFile, JSON.stringify(third));
      return { state: "missing" };
    }
    if (pid === third.pid) return { state: "running", startedAt: new Date(third.startedAt) };
    return { state: "missing" };
  };
  const lock = new SessionLock(lockDir, probe, second);
  const outcome = await lock.acquire();
  assert.equal(outcome.acquired, false);
  assert.deepEqual(JSON.parse(await readFile(lockFile, "utf8")), third);
  assert.deepEqual(await readdir(lockDir), ["session.lock"]);
});

test("a session records its own start time as the helper reports it", async () => {
  const lockDir = await lockDirFor("own");
  const startedMs = Date.parse("2026-09-24T03:04:05.678Z");
  const harness = await connectServer({
    elicitation: "form",
    lockDir,
    script: { handlers: { ...screenshotHandlers(), process_info: [{ result: { alive: true, started_ms: startedMs } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(result.isError, true, textContent(result));
    const asked = (await harness.requests()).find((entry) => (entry as { method?: string }).method === "process_info") as { params?: { pid?: number } };
    assert.equal(asked?.params?.pid, process.pid);
    assert.deepEqual(holderOf(await readFile(join(lockDir, "session.lock"), "utf8")), { pid: process.pid, startedAt: new Date(startedMs).toISOString() });
  } finally {
    await harness.close();
  }
});
