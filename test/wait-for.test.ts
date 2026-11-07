import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { INTERRUPTED_MESSAGE } from "../src/helper-client.ts";
import { pollUntil } from "../src/wait-for.ts";
import { connectServer, textContent, type FakeHandler, type Harness, type HarnessOptions } from "./harness.ts";

// Reference-gaps ticket 03 (G2, window conditions): computer_wait_for waits for a window to appear or
// to close. Through the MCP tool call with the scripted helper: a fake is not UI Automation, so these
// prove the server's logic (what it asks the helper, when it is satisfied, what it returns).

const listed = (title: string, extra: Record<string, unknown> = {}) => ({
  window_id: "7",
  app: "C:\\Apps\\Notes.exe",
  app_name: "Notes",
  title,
  minimized: false,
  elevated: false,
  ...extra,
});
const noMatch = listed("Inbox", { window_id: "3" });
const reading = (...windows: Record<string, unknown>[]): FakeHandler => ({ result: { windows } });
const failing = (code: string, message = "The read failed."): FakeHandler => ({ error: { code, message } });
const closed: FakeHandler = failing("window_not_found", "The window is no longer open.");

interface Logged {
  method?: string;
  at?: number;
}

const waitFor = (harness: Harness, args: Record<string, unknown>) => harness.client.callTool({ name: "computer_wait_for", arguments: args });
const code = (result: { content?: unknown }) => textContent(result).split(/\r?\n/)[0] ?? "";
const requests = async (harness: Harness) => (await harness.requests()) as Logged[];
/** What reached the helper, besides the ping every start begins with and `warm`'s list_apps. */
const methods = async (harness: Harness) => (await requests(harness)).map((entry) => entry.method ?? "").filter((method) => method !== "ping" && method !== "list_apps");
/** Starts the helper (a first request takes a few hundred ms), so the timings measured after it are the wait's own. Its answer is an error, which is ignored. */
const warm = async (harness: Harness) => void (await harness.client.callTool({ name: "computer_list_apps", arguments: {} }));
const metAfter = (result: { content?: unknown }) => Number(/^met_after_ms: (\d+)$/m.exec(textContent(result))?.[1]);

async function withServer<T>(options: HarnessOptions, body: (harness: Harness) => Promise<T>): Promise<T> {
  const harness = await connectServer(options);
  try {
    return await body(harness);
  } finally {
    await harness.close();
  }
}

test("computer_wait_for is listed with the window and element conditions, and computer_wait is as it was", async () => {
  await withServer({}, async (harness) => {
    const tools = (await harness.client.listTools()).tools;
    const tool = tools.find((entry) => entry.name === "computer_wait_for");
    assert.ok(tool, "computer_wait_for is listed");
    const properties = tool.inputSchema.properties as Record<string, { enum?: string[]; minimum?: number; maximum?: number; maxLength?: number }>;
    assert.deepEqual(Object.keys(properties).sort(), ["condition", "text", "timeout_seconds", "window_id"]);
    assert.deepEqual(properties.condition?.enum, ["element_appears", "element_gone", "window_appears", "window_gone"]);
    assert.deepEqual([properties.timeout_seconds?.minimum, properties.timeout_seconds?.maximum], [0.1, 30]);
    assert.equal(properties.text?.maxLength, 200);
    assert.deepEqual(tool.inputSchema.required, ["condition"]);
    const words = [
      "wait_timeout",
      "no input of any kind",
      "300 ms",
      "interrupted",
      "The window conditions do not require approval or the operator lock",
      "The element conditions require approval",
      "window_not_found",
      "password fields",
      "stay valid",
      "could not confirm",
    ];
    for (const word of words) assert.ok((tool.description ?? "").includes(word), word);

    const plain = tools.find((entry) => entry.name === "computer_wait");
    assert.equal(plain?.description, "Wait between 0.1 and 10 seconds. Does not require approval or the operator lock.");
    assert.deepEqual(plain?.inputSchema, {
      type: "object",
      properties: { seconds: { type: "number", description: "How long to wait, from 0.1 to 10 seconds." } },
      required: ["seconds"],
    });
  });
});

test("parameters that do not fit are invalid_params before anything reaches the helper, and the text is not quoted back", async () => {
  const cases: Record<string, unknown>[] = [
    {},
    { condition: "window_appears" },
    { condition: "window_appears", text: "" },
    { condition: "window_appears", text: 5 },
    { condition: "window_appears", text: "x".repeat(201) },
    { condition: "window_gone" },
    { condition: "window_gone", window_id: "" },
    { condition: "window_gone", window_id: 7 },
    { condition: "window_appears", text: "Save", timeout_seconds: 0.05 },
    { condition: "window_appears", text: "Save", timeout_seconds: 31 },
    { condition: "window_appears", text: "Save", timeout_seconds: "5" },
    { condition: "window_gone", window_id: "7", timeout_seconds: Number.NaN },
    { condition: "window_appears_somewhere", text: "SECRET-WORD" },
  ];
  await withServer({}, async (harness) => {
    for (const args of cases) {
      const result = await waitFor(harness, args);
      assert.equal(result.isError, true, JSON.stringify(args));
      assert.equal(code(result), "invalid_params", JSON.stringify(args));
      assert.ok(!textContent(result).includes("SECRET-WORD"), "the model's text is quoted back");
    }
    assert.deepEqual(await methods(harness), []);
  });
});

test("window_appears is met by the first listed window whose title holds the text, ignoring case, and returns its info and id", async () => {
  const dialog = listed("Save As", { window_id: "9", owner_window_id: "7", minimized: true });
  const script = { handlers: { list_windows: [reading(noMatch), reading(noMatch, listed("Notes - Untitled"), dialog)] } };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "window_appears", text: "SAVE as" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /^window_id: 9$/m);
    assert.match(textContent(result), /^title: "Save As"$/m);
    assert.match(textContent(result), /^owner_window_id: 7$/m);
    assert.match(textContent(result), /^minimized: true$/m, "the same view as computer_list_windows: minimized windows count");
    assert.deepEqual(result.structuredContent, { condition: "window_appears", met_after_ms: metAfter(result), window_id: "9" });
    assert.ok(metAfter(result) >= 300, `met_after_ms ${metAfter(result)} is after the 300 ms gap`);
    assert.deepEqual(await methods(harness), ["list_windows", "list_windows"], "only reads, and no get_window or approval");
    assert.equal(harness.elicitations.length, 0);
  });
});

test("a window that is already there is met at once", async () => {
  await withServer({ script: { handlers: { list_windows: [reading(listed("Save As"))] } } }, async (harness) => {
    await warm(harness);
    const result = await waitFor(harness, { condition: "window_appears", text: "save" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.deepEqual(await methods(harness), ["list_windows"], "one read, no gap waited");
  });
});

test("the next read starts 300 ms after the previous one ended, not 300 ms after it started", async () => {
  const slow: FakeHandler = { delayMs: 250, ...reading(noMatch) };
  await withServer({ script: { handlers: { list_windows: Array.from({ length: 20 }, () => slow) } } }, async (harness) => {
    await warm(harness);
    // Long enough for two reads on a machine under load.
    const result = await waitFor(harness, { condition: "window_appears", text: "never", timeout_seconds: 4 });
    assert.equal(code(result), "wait_timeout");
    const arrivals = (await requests(harness)).filter((entry) => entry.method === "list_windows").map((entry) => entry.at ?? 0);
    assert.ok(arrivals.length >= 2, `reads: ${arrivals.length}`);
    for (let i = 1; i < arrivals.length; i += 1) {
      // The read took 250 ms; a fixed 300 ms interval would give about 300.
      assert.ok(arrivals[i]! - arrivals[i - 1]! >= 540, `gap ${arrivals[i]! - arrivals[i - 1]!} ms`);
    }
  });
});

test("a timeout is wait_timeout naming the condition and the time waited, says no input was sent, and shows no window title", async () => {
  const script = { handlers: { list_windows: [reading(listed("Private Inbox (3)"))] } };
  await withServer({ script }, async (harness) => {
    const started = Date.now();
    const result = await waitFor(harness, { condition: "window_appears", text: "Save As", timeout_seconds: 0.5 });
    const took = Date.now() - started;
    assert.equal(result.isError, true);
    assert.equal(code(result), "wait_timeout");
    const text = textContent(result);
    assert.match(text, /window_appears/);
    assert.match(text, /"Save As"/, "the caller's own text");
    assert.match(text, /0\.\d seconds|\b[01] seconds?/);
    assert.match(text, /no input was sent/i);
    assert.ok(!text.includes("Private Inbox"), "a window title other than the caller's text");
    // The suite runs many files at once, so only a loose upper bound: the default 10 s would not pass it.
    assert.ok(took >= 450 && took < 8000, `took ${took} ms`);
  });
});

test("window_gone: a read that failed, or found the window open but not listed, does not satisfy it; window_not_found does", async () => {
  const script = {
    handlers: {
      get_window: [failing("internal"), failing("helper_timeout", "Helper did not respond within 10000ms."), failing("window_not_operable", "The window is open but cloaked."), { result: { window_id: "7" } }, closed],
    },
  };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "window_gone", window_id: "7" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /window_gone/);
    assert.ok(metAfter(result) >= 1200, `met_after_ms ${metAfter(result)}: five reads, four gaps`);
    assert.deepEqual(result.structuredContent, { condition: "window_gone", met_after_ms: metAfter(result) });
    assert.deepEqual(await methods(harness), ["get_window", "get_window", "get_window", "get_window", "get_window"]);
    const asked = ((await harness.requests()) as { method?: string; params?: unknown }[]).filter((entry) => entry.method === "get_window").map((entry) => entry.params);
    assert.ok(asked.every((params) => JSON.stringify(params) === '{"window_id":"7"}'), "each read names only the window");
  });
});

test("window_gone that only ever fails to read times out and says it could not be confirmed", async () => {
  const script = { handlers: { get_window: Array.from({ length: 40 }, () => failing("internal", "SECRET-DETAIL")) } };
  await withServer({ script }, async (harness) => {
    await warm(harness);
    const result = await waitFor(harness, { condition: "window_gone", window_id: "7", timeout_seconds: 2 });
    assert.equal(code(result), "wait_timeout");
    assert.match(textContent(result), /window_gone/);
    assert.match(textContent(result), /could not be confirmed|not confirmed/i);
    assert.match(textContent(result), /no input was sent/i);
  });
});

test("window_gone for a handle the helper rejects fails at once as invalid_params", async () => {
  const script = { handlers: { get_window: [failing("invalid_params", "window_id is not a window handle.")] } };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "window_gone", window_id: "abc", timeout_seconds: 5 });
    assert.equal(code(result), "invalid_params");
    assert.deepEqual(await methods(harness), ["get_window"]);
  });
});

test("a stop during the gap between polls is interrupted at once, not at the timeout, and no further read is made", async () => {
  const script = { handlers: { list_windows: [{ ...reading(noMatch), eventsAfter: [{ event: "stopped", data: { generation: 1 } }] }] } };
  await withServer({ script }, async (harness) => {
    await warm(harness);
    const started = Date.now();
    const result = await waitFor(harness, { condition: "window_appears", text: "never", timeout_seconds: 30 });
    const took = Date.now() - started;
    assert.equal(code(result), "interrupted");
    assert.ok(textContent(result).includes(INTERRUPTED_MESSAGE));
    // How fast the sleep notices a stop is pinned on pollUntil below; here it is only that the timeout was not waited for.
    assert.ok(took < 15_000, `took ${took} ms, not 30 s`);
    assert.deepEqual(await methods(harness), ["list_windows"]);
  });
});

test("the pause between reads notices a stop within a few tens of ms, however long the pause is", async () => {
  let stopped = false;
  setTimeout(() => (stopped = true), 100);
  const started = performance.now();
  const outcome = await pollUntil({ timeoutMs: 30_000, gapMs: 5_000, interrupted: () => stopped, check: async () => ({ pending: undefined }) });
  const took = performance.now() - started;
  assert.equal(outcome.kind, "interrupted");
  // A pause slept whole would be 5 s; the margin is for a loaded machine, not for the slices (25 ms).
  assert.ok(took < 2500, `took ${took} ms`);
});

test("a read that hangs is not waited for past the timeout", async () => {
  const started = performance.now();
  const outcome = await pollUntil({ timeoutMs: 200, gapMs: 300, interrupted: () => false, check: () => new Promise<never>(() => undefined) });
  assert.equal(outcome.kind, "timeout");
  assert.ok(performance.now() - started < 5000);
});

test("a stop that arrives while a read is in flight is interrupted too", async () => {
  const script = { handlers: { list_windows: [{ delayMs: 30, events: [{ event: "stopped" }], ...reading(noMatch) }] } };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "window_appears", text: "never", timeout_seconds: 30 });
    assert.equal(code(result), "interrupted");
    assert.deepEqual(await methods(harness), ["list_windows"]);
  });
});

test("while input is stopped, a wait_for is interrupted before its first read", async () => {
  const script = { handlers: { list_windows: [{ events: [{ event: "stopped" }], ...reading() }] } };
  await withServer({ script }, async (harness) => {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const result = await waitFor(harness, { condition: "window_appears", text: "never", timeout_seconds: 30 });
    assert.equal(code(result), "interrupted");
    assert.deepEqual(await methods(harness), ["list_windows"], "no read for the wait_for");
  });
});

interface BatchResult {
  isError?: boolean;
  content?: { type: string; text?: string }[];
}

const batch = async (harness: Harness, steps: unknown[]) => {
  const result = (await harness.client.callTool({ name: "computer_batch", arguments: { steps } })) as BatchResult;
  return { result, lines: textContent(result).split(/\r?\n/) };
};

test("as a batch step it succeeds and the batch goes on; a timeout stops the batch like any failed step", async () => {
  await withServer({ script: { handlers: { list_windows: [reading(listed("Save As")), reading(noMatch), reading(noMatch)] } } }, async (harness) => {
    const ok = await batch(harness, [{ action: "wait_for", condition: "window_appears", text: "save" }, { action: "wait", seconds: 0.1 }]);
    assert.equal(ok.lines[0], "completed 2 of 2 steps", ok.lines.join("\n"));
    assert.match(ok.lines[1] ?? "", /^\[1\] wait_for: ok /);

    const stopped = await batch(harness, [
      { action: "wait_for", condition: "window_appears", text: "never", timeout_seconds: 0.2 },
      { action: "wait", seconds: 0.1 },
    ]);
    assert.equal(stopped.result.isError, true);
    assert.equal(stopped.lines[0], "wait_timeout");
    assert.equal(stopped.lines[1], "stopped at step 1 of 2: wait_timeout");
    assert.match(stopped.lines[2] ?? "", /^\[1\] wait_for: wait_timeout \(sent: false\) /);
    assert.equal(stopped.lines[3], "[2] wait: not run");
  });
});

test("a wait_for step cannot run past the batch's 60-second limit: the time left bounds it", async () => {
  // The first reading of the clock sets the limit (at 60 000 ms); every later one is 300 ms before it.
  let readings = 0;
  const now = () => (readings++ === 0 ? 0 : 59_700);
  const script = { handlers: { list_windows: Array.from({ length: 5 }, () => reading(noMatch)) } };
  await withServer({ now, script }, async (harness) => {
    await warm(harness);
    const started = Date.now();
    const { result, lines } = await batch(harness, [{ action: "list_windows" }, { action: "wait_for", condition: "window_appears", text: "never", timeout_seconds: 30 }]);
    const took = Date.now() - started;
    assert.equal(result.isError, true);
    assert.equal(lines[1], "stopped at step 2 of 2: wait_timeout");
    assert.match(lines.find((line) => line.startsWith("[2] wait_for")) ?? "", /batch's 60-second limit/, "says the batch's limit ended the wait");
    assert.ok(took < 15_000, `took ${took} ms, not 30 s`);
  });
});

test("a batch with a wait_for step that does not fit is refused as a whole before any step runs", async () => {
  await withServer({}, async (harness) => {
    for (const step of [
      { action: "wait_for", condition: "window_appears", text: "x", timeout_seconds: 31 },
      { action: "wait_for", condition: "window_appears" },
      { action: "wait_for", condition: "element_gone", text: "x" },
    ]) {
      const { result, lines } = await batch(harness, [{ action: "list_windows" }, step]);
      assert.equal(result.isError, true, JSON.stringify(step));
      assert.equal(lines[0], "invalid_params", JSON.stringify(step));
      assert.match(lines[1] ?? "", /step 2 of 2 \(wait_for\)/, JSON.stringify(step));
    }
    assert.deepEqual(await methods(harness), []);
  });
});

test("the log records the call with its result and nothing the caller or the windows said", async () => {
  await withServer({ script: { handlers: { list_windows: [reading(listed("Private Inbox (3)"))] } } }, async (harness) => {
    await waitFor(harness, { condition: "window_appears", text: "CALLER-TEXT", timeout_seconds: 0.2 });
    await harness.finish();
    const dir = join(harness.dir, "logs");
    const raw = (await Promise.all((await readdir(dir)).map((name) => readFile(join(dir, name), "utf8")))).join("\n");
    const events = raw.split("\n").filter(Boolean).map((line) => JSON.parse(line) as Record<string, unknown>);
    const call = events.find((event) => event.event === "tool_call" && event.tool === "computer_wait_for");
    assert.ok(call, raw);
    assert.equal(call.result, "wait_timeout");
    assert.equal(call.app, null);
    assert.ok(!raw.includes("CALLER-TEXT") && !raw.includes("Private Inbox"));
  });
});
