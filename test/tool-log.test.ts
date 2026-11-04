import assert from "node:assert/strict";
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { SessionLock, defaultLockDir } from "../src/lock.ts";
import { defaultLogDir } from "../src/log.ts";
import { connectServer, repoRoot, retryingSpawn, type Harness, type HarnessOptions } from "./harness.ts";

// Ticket 89: every tool call (and every batch step) writes one `tool_call` event to the day's
// JSONL log; `action_summary` goes into it and nowhere else (seam one: the MCP client, the server
// and the scripted helper, the log folder in a temp directory).

const TITLE = "Quarterly-Report-TITLE-7Q";

const demo = {
  window_id: "5",
  app: "C:\\Apps\\Demo.exe",
  app_name: "Demo",
  title: TITLE,
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 200,
  height: 100,
  pid: 4242,
};

const windows = (count: number) => Array.from({ length: count }, () => ({ result: demo }));

const ACTION_TOOLS = [
  "computer_click",
  "computer_drag",
  "computer_scroll",
  "computer_move_mouse",
  "computer_type_text",
  "computer_paste_text",
  "computer_press_key",
  "computer_set_value",
  "computer_element_action",
  "computer_activate_window",
  "computer_launch_app",
  "computer_read_clipboard",
];

function day(time: number): string {
  const date = new Date(time);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

const DAY_MS = 24 * 60 * 60 * 1000;

async function tempDir(): Promise<string> {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  return mkdtemp(join(root, "log-"));
}

/** Today's `tool_call` events (ticket 90's `approval` events share the file). */
async function logLines(dir: string): Promise<Record<string, unknown>[]> {
  const raw = await readFile(join(dir, `${day(Date.now())}.jsonl`), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call");
}

async function allLogText(dir: string): Promise<string> {
  const names = await readdir(dir);
  const texts = await Promise.all(names.map((name) => readFile(join(dir, name), "utf8")));
  return texts.join("\n");
}

/** Every file under `dir` (relative path) with its content and modification time. */
async function snapshot(dir: string): Promise<Record<string, { content: string; mtimeMs: number }>> {
  const files: Record<string, { content: string; mtimeMs: number }> = {};
  for (const entry of await readdir(dir, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile()) continue;
    const file = join(entry.parentPath, entry.name);
    files[file.slice(dir.length + 1)] = { content: await readFile(file, "utf8"), mtimeMs: (await stat(file)).mtimeMs };
  }
  return files;
}

async function withHarness(options: HarnessOptions, body: (harness: Harness, logDir: string) => Promise<void>) {
  const harness = await connectServer(options);
  const logDir = options.logDir ?? join(harness.dir, "logs");
  try {
    await body(harness, logDir);
  } finally {
    await harness.close();
  }
}

test("every action tool takes an optional action_summary described as what and to whom, without reasons or typed content", async () => {
  await withHarness({}, async (harness) => {
    const { tools } = await harness.client.listTools();
    for (const name of ACTION_TOOLS) {
      const tool = tools.find((entry) => entry.name === name);
      assert.ok(tool, name);
      const property = tool.inputSchema.properties?.action_summary as { type?: string; description?: string } | undefined;
      assert.equal(property?.type, "string", name);
      assert.match(property?.description ?? "", /what this step does and to whom/i, name);
      assert.match(property?.description ?? "", /no reasons/i, name);
      assert.match(property?.description ?? "", /no passwords or typed content/i, name);
      assert.equal(tool.inputSchema.required?.includes("action_summary") ?? false, false, name);
    }
  });
});

test("a tool call writes one tool_call event with the time, session, tool, step, app, action_summary, result and duration", async () => {
  await withHarness({ elicitation: "form", script: { handlers: { get_window: windows(1) } } }, async (harness, logDir) => {
    const typed = await harness.client.callTool({
      name: "computer_type_text",
      arguments: { window_id: "5", text: "hello", action_summary: "Type a greeting into Demo's editor" },
    });
    assert.notEqual(typed.isError, true);
    const listed = await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
    assert.notEqual(listed.isError, true);
    const refused = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "nosuchkey" } });
    assert.equal(refused.isError, true);

    const [first, second, third, ...rest] = await logLines(logDir);
    assert.equal(rest.length, 0);
    assert.equal(first?.event, "tool_call");
    assert.ok(typeof first?.time === "string" && !Number.isNaN(Date.parse(first.time)), String(first?.time));
    assert.ok(typeof first?.session === "string" && first.session.length > 0);
    assert.equal(first?.tool, "computer_type_text");
    assert.equal(first?.step, null);
    assert.deepEqual(first?.app, { path: demo.app, key: "c:\\apps\\demo.exe", tier: "normal" });
    assert.equal(first?.action_summary, "Type a greeting into Demo's editor");
    assert.equal(first?.result, "ok");
    assert.ok(typeof first?.duration_ms === "number" && first.duration_ms >= 0);

    assert.equal(second?.tool, "computer_wait");
    assert.equal(second?.session, first?.session);
    assert.equal(second?.app, null);
    assert.equal(second?.action_summary, null);
    assert.equal(second?.result, "ok");

    assert.equal(third?.tool, "computer_press_key");
    assert.equal(third?.result, "invalid_params");
    assert.equal(third?.app, null);
  });
});

test("the log never holds typed or pasted text, the keys pressed, or the window title", async () => {
  const typedSecret = "Hunter2-TYPED-SECRET-91";
  const pastedSecret = "Correct-Horse-PASTED-SECRET-17";
  await withHarness({ elicitation: "form", script: { handlers: { get_window: windows(3) } } }, async (harness, logDir) => {
    for (const call of [
      { name: "computer_type_text", arguments: { window_id: "5", text: typedSecret } },
      { name: "computer_paste_text", arguments: { window_id: "5", text: pastedSecret } },
      { name: "computer_press_key", arguments: { window_id: "5", key: "ctrl+alt+F11" } },
    ]) {
      const result = await harness.client.callTool(call);
      assert.notEqual(result.isError, true, call.name);
    }
    const text = await allLogText(logDir);
    assert.equal((await logLines(logDir)).length, 3);
    for (const secret of [typedSecret, pastedSecret, "F11", "ctrl+alt", TITLE]) {
      assert.equal(text.includes(secret), false, `the log holds ${secret}`);
    }
  });
});

test("a batch writes one event per step that ran, numbered, with its own action_summary, and one for the batch", async () => {
  await withHarness({ elicitation: "form", script: { handlers: { get_window: windows(2) } } }, async (harness, logDir) => {
    const result = await harness.client.callTool({
      name: "computer_batch",
      arguments: {
        steps: [
          { action: "press_key", window_id: "5", key: "ctrl+a", action_summary: "Select all in Demo" },
          { action: "wait", seconds: 0.1 },
          { action: "press_key", window_id: "5", key: "Delete", action_summary: "Delete the selection in Demo" },
        ],
      },
    });
    assert.notEqual(result.isError, true);
    const events = await logLines(logDir);
    const steps = events.filter((event) => event.step !== null);
    assert.deepEqual(
      steps.map((event) => [event.step, event.tool, event.action_summary, event.result]),
      [
        [1, "computer_press_key", "Select all in Demo", "ok"],
        [2, "computer_wait", null, "ok"],
        [3, "computer_press_key", "Delete the selection in Demo", "ok"],
      ],
    );
    assert.deepEqual(steps[0]?.app, { path: demo.app, key: "c:\\apps\\demo.exe", tier: "normal" });
    assert.equal(steps[1]?.app, null);
    const batch = events.filter((event) => event.step === null);
    assert.deepEqual(
      batch.map((event) => [event.tool, event.result]),
      [["computer_batch", "ok"]],
    );
  });
});

test("action_summary loses newlines and control characters and is cut to 200 characters, in a tool and a batch step", async () => {
  const long = `Click\nthe\r\tSave\u0007 button\u2028in Demo ${"x".repeat(300)}`;
  const cleaned = `ClicktheSave buttonin Demo ${"x".repeat(300)}`.slice(0, 200);
  await withHarness({ elicitation: "form", script: { handlers: { get_window: windows(2) } } }, async (harness, logDir) => {
    const single = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return", action_summary: long } });
    assert.notEqual(single.isError, true);
    const batch = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "press_key", window_id: "5", key: "Return", action_summary: long }] },
    });
    assert.notEqual(batch.isError, true);
    const summaries = (await logLines(logDir)).filter((event) => event.tool === "computer_press_key").map((event) => event.action_summary);
    assert.deepEqual(summaries, [cleaned, cleaned]);
  });
});

test("action_summary is in neither the elicitation message nor any request to the helper, ask_user included", async () => {
  const summary = "SUMMARY-MARKER-Approve-me-please";
  await withHarness({ elicitation: "form", script: { handlers: { get_window: windows(1) } } }, async (harness) => {
    const result = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return", action_summary: summary } });
    assert.notEqual(result.isError, true);
    assert.equal(harness.elicitations.length, 1);
    assert.equal(harness.elicitations[0]?.includes(summary), false);
    assert.equal(JSON.stringify(await harness.requests()).includes(summary), false);
  });
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [{ result: { answer: "allow" } }] } } }, async (harness) => {
    const result = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return", action_summary: summary } });
    assert.notEqual(result.isError, true);
    const requests = (await harness.requests()) as { method?: string }[];
    assert.equal(requests.filter((request) => request.method === "ask_user").length, 1);
    assert.equal(JSON.stringify(requests).includes(summary), false);
  });
});

test("at start the server deletes log files older than 30 days, then the oldest while the total is over 10 MB", async () => {
  const logDir = await tempDir();
  const now = Date.now();
  const mb = (n: number) => "x".repeat(n * 1024 * 1024);
  await writeFile(join(logDir, `${day(now - 31 * DAY_MS)}.jsonl`), "old\n");
  await writeFile(join(logDir, `${day(now - 29 * DAY_MS)}.jsonl`), "kept\n");
  await writeFile(join(logDir, `${day(now - 5 * DAY_MS)}.jsonl`), mb(4));
  await writeFile(join(logDir, `${day(now - 4 * DAY_MS)}.jsonl`), mb(4));
  await writeFile(join(logDir, `${day(now - 3 * DAY_MS)}.jsonl`), mb(4));
  await writeFile(join(logDir, "notes.txt"), "not a log");
  try {
    const harness = await connectServer({ logDir });
    await harness.close();
    const left = (await readdir(logDir)).sort();
    // 12 MB of logs: the oldest go first, the 29-day-old file before the 4 MB ones.
    assert.deepEqual(left, [`${day(now - 4 * DAY_MS)}.jsonl`, `${day(now - 3 * DAY_MS)}.jsonl`, "notes.txt"].sort());
  } finally {
    await rm(logDir, { recursive: true, force: true });
  }
});

test("the first write of each day deletes the log files that have grown older than 30 days", async () => {
  const logDir = await tempDir();
  let clock = Date.now();
  const start = clock;
  try {
    await withHarness({ logDir, logClock: () => clock }, async (harness) => {
      // 30 days old today, 31 tomorrow.
      const aging = `${day(start - 30 * DAY_MS)}.jsonl`;
      await writeFile(join(logDir, aging), "aging\n");
      await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
      assert.equal((await readdir(logDir)).includes(aging), true);
      clock = start + DAY_MS;
      await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
      const left = await readdir(logDir);
      assert.equal(left.includes(aging), false);
      assert.equal(left.includes(`${day(clock)}.jsonl`), true);
    });
  } finally {
    await rm(logDir, { recursive: true, force: true });
  }
});

test("a log that cannot be written fails no tool and is reported on stderr once", async () => {
  const dir = await tempDir();
  const blocker = join(dir, "not-a-folder");
  await writeFile(blocker, "a file where the log folder should be");
  const errors: string[] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => {
    errors.push(args.map(String).join(" "));
  };
  try {
    await withHarness({ logDir: join(blocker, "logs") }, async (harness) => {
      for (let i = 0; i < 3; i += 1) {
        const result = await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
        assert.notEqual(result.isError, true);
      }
    });
  } finally {
    console.error = original;
    await rm(dir, { recursive: true, force: true });
  }
  assert.equal(errors.filter((line) => /log/i.test(line)).length, 1, errors.join("\n"));
});

test("by default the log is written under %LOCALAPPDATA%\\deskpass\\logs", async () => {
  const local = await tempDir();
  const saved = process.env.LOCALAPPDATA;
  process.env.LOCALAPPDATA = local;
  try {
    await withHarness({ logDir: undefined }, async (harness) => {
      await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
    });
    const file = join(local, "deskpass", "logs", `${day(Date.now())}.jsonl`);
    assert.ok((await stat(file)).size > 0);
  } finally {
    if (saved === undefined) delete process.env.LOCALAPPDATA;
    else process.env.LOCALAPPDATA = saved;
    await rm(local, { recursive: true, force: true });
  }
});

test("ticket 110: the default folder is %LOCALAPPDATA%\\deskpass, and an old computer-use-mcp folder there is left untouched", async () => {
  const local = await tempDir();
  const saved = process.env.LOCALAPPDATA;
  process.env.LOCALAPPDATA = local;
  const old = join(local, "computer-use-mcp");
  const oldFiles: Record<string, string> = {
    "session.lock": JSON.stringify({ pid: process.pid, startedAt: new Date().toISOString() }),
    "grants.json": "{}",
    "grants.key": "old key",
    [join("logs", `${day(Date.now())}.jsonl`)]: '{"event":"old"}\n',
  };
  try {
    await mkdir(join(old, "logs"), { recursive: true });
    for (const [name, content] of Object.entries(oldFiles)) await writeFile(join(old, name), content);
    const before = await snapshot(old);
    const fresh = join(local, "deskpass");
    assert.equal(defaultLockDir(), fresh);
    assert.equal(defaultLogDir(), join(fresh, "logs"));
    // The server's own defaults for the log and the grants folder (not the harness's); the helper is told the new folder.
    const helperArgs: string[][] = [];
    const inner = retryingSpawn();
    const spawn = ((command: string, args: readonly string[], options: object) => {
      helperArgs.push([...args]);
      return inner(command, args, options);
    }) as never;
    await withHarness({ logDir: undefined, grantsDir: undefined, spawn }, async (harness) => {
      await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
      await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    });
    assert.ok(helperArgs.length > 0, "the helper was started");
    for (const args of helperArgs) assert.equal(args[args.indexOf("--grants-dir") + 1], fresh, args.join(" "));
    assert.ok((await stat(join(fresh, "logs", `${day(Date.now())}.jsonl`))).size > 0);
    // The session lock goes to the new folder too, whatever the old one holds.
    const lock = new SessionLock(defaultLockDir(), async () => ({ state: "missing" }));
    assert.deepEqual(await lock.acquire(), { acquired: true });
    assert.ok((await stat(join(fresh, "session.lock"))).size > 0);
    await lock.release();
    assert.deepEqual(await snapshot(old), before, "the old folder was changed");
  } finally {
    if (saved === undefined) delete process.env.LOCALAPPDATA;
    else process.env.LOCALAPPDATA = saved;
    await rm(local, { recursive: true, force: true });
  }
});
