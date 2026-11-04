import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 142: delivery_mode on the tools that only have a foreground path (seam A: the server with a
// scripted helper). Default and "foreground" are today's behavior; "background" is
// background_unavailable (tool_needs_foreground) before the lock, approval, overlay or any input.

const demo = {
  window_id: "5",
  app: "C:\\Apps\\demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 400,
  height: 300,
  pid: 4242,
};

const picture = {
  image_base64: "QQ==",
  original_width: 400,
  original_height: 300,
  window_x: 0,
  window_y: 0,
  window_width: 400,
  window_height: 300,
};

const RETRY = 'Retry this action with delivery_mode: "foreground"';

interface Case {
  action: string;
  /** The helper request that sends the tool's input. */
  method: string;
  args: Record<string, unknown>;
  answer: FakeHandler;
  /** What the tool answers today. */
  text: string;
}

const CASES: Case[] = [
  { action: "type_text", method: "type_text", args: { text: "hi" }, answer: { result: {} }, text: "Typed text into the target window." },
  { action: "press_key", method: "press_key", args: { key: "ctrl+s" }, answer: { result: {} }, text: "Pressed ctrl+s." },
  { action: "paste_text", method: "paste_text", args: { text: "hi" }, answer: { result: { read_confirmed: true, clipboard_restored: "full" } }, text: "Pasted the text into the target window; the focused element's value now contains it (read_confirmed: true)." },
  { action: "scroll", method: "scroll", args: { x: 20, y: 30, direction: "down" }, answer: { result: {} }, text: "Scrolled down by 3." },
  { action: "drag", method: "drag", args: { from_x: 10, from_y: 10, to_x: 40, to_y: 50 }, answer: { result: {} }, text: "Dragged from" },
];

const many = (handler: FakeHandler, count = 8): FakeHandler[] => Array.from({ length: count }, () => handler);

function serverFor(entries: Case[]) {
  const handlers: Record<string, FakeHandler[]> = {
    get_window: many({ result: demo }),
    capture_window: many({ result: picture }),
    activate_window: many({ result: {} }),
  };
  for (const entry of entries) handlers[entry.method] = many(entry.answer);
  return connectServer({ elicitation: "form", script: { handlers } });
}

async function shoot(harness: Harness) {
  const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
  assert.notEqual(shot.isError, true);
}

/** The helper requests made after the screenshot. */
async function methodsAfterShot(harness: Harness): Promise<string[]> {
  const all = ((await harness.requests()) as { method?: string }[]).map((entry) => String(entry.method)).filter((method) => method !== "ping");
  return all.slice(all.lastIndexOf("capture_window") + 1);
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

async function toolCalls(harness: Harness, tool: string): Promise<Record<string, unknown>[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call" && event.tool === tool);
}

type Tool = { name: string; inputSchema: { properties?: Record<string, { type?: string; enum?: string[] }>; required?: string[] } };

test("ticket 142: the five foreground-only tools offer an optional delivery_mode of background or foreground", async () => {
  const harness = await connectServer();
  try {
    const { tools } = (await harness.client.listTools()) as { tools: Tool[] };
    for (const entry of CASES) {
      const tool = tools.find((candidate) => candidate.name === `computer_${entry.action}`);
      assert.equal(tool?.inputSchema.properties?.delivery_mode?.type, "string", entry.action);
      assert.deepEqual(tool?.inputSchema.properties?.delivery_mode?.enum, ["background", "foreground"], entry.action);
      assert.ok(!tool?.inputSchema.required?.includes("delivery_mode"), entry.action);
      assert.match(String((tool?.inputSchema.properties?.delivery_mode as { description?: string }).description), /Defaults to "foreground"/, entry.action);
    }
  } finally {
    await harness.close();
  }
});

test("ticket 142: without delivery_mode, or with foreground, each tool activates the window and sends its input as before", async () => {
  for (const extra of [{}, { delivery_mode: "foreground" }]) {
    for (const entry of CASES) {
      const harness = await serverFor([entry]);
      try {
        await shoot(harness);
        const result = await harness.client.callTool({ name: `computer_${entry.action}`, arguments: { window_id: "5", ...entry.args, ...extra } });
        assert.notEqual(result.isError, true, `${entry.action}: ${textContent(result)}`);
        assert.ok(textContent(result).startsWith(entry.text), `${entry.action}: ${textContent(result)}`);
        const order = await methodsAfterShot(harness);
        assert.ok(order.includes("activate_window"), `${entry.action}: ${order.join(", ")}`);
        assert.ok(order.includes(entry.method), `${entry.action}: ${order.join(", ")}`);
        assert.ok(order.indexOf("overlay_show") < order.indexOf(entry.method), `${entry.action}: ${order.join(", ")}`);
        const sent = ((await harness.requests()) as { method?: string; params?: Record<string, unknown> }[]).find((request) => request.method === entry.method);
        assert.ok(!("delivery_mode" in (sent?.params ?? {})), `${entry.action}: the helper request is unchanged`);
      } finally {
        await harness.close();
      }
    }
  }
});

test("ticket 142: an explicit background is background_unavailable (tool_needs_foreground): nothing reaches the helper but the earlier screenshot, and nothing is asked", async () => {
  for (const entry of CASES) {
    const harness = await serverFor([entry]);
    try {
      await shoot(harness);
      const asked = harness.elicitations.length;
      const result = await harness.client.callTool({ name: `computer_${entry.action}`, arguments: { window_id: "5", ...entry.args, delivery_mode: "background" } });
      assert.equal(result.isError, true, entry.action);
      assert.equal(code(result), "background_unavailable", `${entry.action}: ${textContent(result)}`);
      assert.ok(textContent(result).includes(RETRY), textContent(result));
      assert.deepEqual(result.structuredContent, { reason: "tool_needs_foreground", recommended: "foreground" }, entry.action);
      assert.deepEqual(await methodsAfterShot(harness), [], `${entry.action}: the helper received a request after the screenshot`);
      assert.equal(harness.elicitations.length, asked, `${entry.action}: no approval question`);
    } finally {
      await harness.close();
    }
  }
});

test("ticket 142: an unknown delivery_mode is invalid_params without quoting it, and nothing reaches the helper", async () => {
  for (const entry of CASES) {
    const harness = await serverFor([entry]);
    try {
      await shoot(harness);
      for (const mode of ["auto", "Background", "", 1, null, true]) {
        const result = await harness.client.callTool({ name: `computer_${entry.action}`, arguments: { window_id: "5", ...entry.args, delivery_mode: mode } });
        assert.equal(code(result), "invalid_params", `${entry.action} ${JSON.stringify(mode)}: ${textContent(result)}`);
        assert.match(textContent(result), /delivery_mode/);
        assert.ok(!textContent(result).includes("auto"), textContent(result));
      }
      assert.deepEqual(await methodsAfterShot(harness), [], entry.action);
    } finally {
      await harness.close();
    }
  }
});

test("ticket 142: the log says foreground for a foreground call and background with its reason for a refused one", async () => {
  for (const entry of CASES) {
    const harness = await serverFor([entry]);
    try {
      await shoot(harness);
      const tool = `computer_${entry.action}`;
      await harness.client.callTool({ name: tool, arguments: { window_id: "5", ...entry.args } });
      await harness.client.callTool({ name: tool, arguments: { window_id: "5", ...entry.args, delivery_mode: "background" } });
      await harness.client.callTool({ name: tool, arguments: { window_id: "5", ...entry.args, delivery_mode: "auto" } });
      await harness.finish();
      const calls = await toolCalls(harness, tool);
      assert.equal(calls.length, 3, entry.action);
      assert.deepEqual([calls[0]?.result, calls[0]?.delivery], ["ok", "foreground"], entry.action);
      assert.deepEqual([calls[1]?.result, calls[1]?.delivery, calls[1]?.reason], ["background_unavailable", "background", "tool_needs_foreground"], entry.action);
      assert.deepEqual([calls[2]?.result, "delivery" in (calls[2] ?? {}), "reason" in (calls[2] ?? {})], ["invalid_params", false, false], entry.action);
    } finally {
      await harness.close();
    }
  }
});

// Batch: each step carries its own delivery_mode, defaulting as the single tool does.

type CallResult = { isError?: boolean; content?: Array<{ type: string; text?: string }>; structuredContent?: unknown };

async function runBatch(harness: Harness, steps: unknown[]) {
  const result = (await harness.client.callTool({ name: "computer_batch", arguments: { steps } })) as CallResult;
  const text = (result.content ?? []).filter((block) => block.type === "text").map((block) => block.text ?? "");
  return { result, lines: (text[0] ?? "").split("\n") };
}

test("ticket 142: a batch step takes delivery_mode as its tool does: foreground steps run, a background step of a foreground-only tool is refused", async () => {
  const harness = await serverFor([CASES[0], CASES[1]]);
  try {
    const { result, lines } = await runBatch(harness, [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return", delivery_mode: "foreground" },
    ]);
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 2 of 2 steps");
    assert.equal(lines[1], "[1] type_text: ok Typed text into the target window.");
    assert.equal(lines[2], "[2] press_key: ok Pressed return.");
    const methods = ((await harness.requests()) as { method?: string }[]).map((entry) => entry.method);
    assert.deepEqual(methods.filter((method) => method === "type_text" || method === "press_key"), ["type_text", "press_key"]);
  } finally {
    await harness.close();
  }
});

test("ticket 142: a batch mixing a background click, foreground steps and a refused background step stops at the refusal by the batch's failure rule", async () => {
  const click: Case = { action: "click", method: "click", args: {}, answer: { result: {} }, text: "" };
  const harness = await serverFor([CASES[0], CASES[1], click]);
  try {
    await shoot(harness);
    const before = (await harness.requests()).length;
    const { result, lines } = await runBatch(harness, [
      { action: "click", window_id: "5", x: 5, y: 5, delivery_mode: "foreground" },
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "ctrl+s", delivery_mode: "background" },
      { action: "type_text", window_id: "5", text: "later" },
    ]);
    assert.equal(result.isError, true);
    assert.equal(lines[0], "background_unavailable");
    assert.equal(lines[1], "stopped at step 3 of 4: background_unavailable");
    assert.equal(lines[2], "[1] click: ok Clicked left 1 time(s) at window pixel (5, 5).");
    assert.equal(lines[3], "[2] type_text: ok Typed text into the target window.");
    assert.match(lines[4] ?? "", /^\[3\] press_key: background_unavailable \(sent: false\) /);
    assert.ok((lines[4] ?? "").includes(RETRY), lines[4]);
    assert.equal(lines[5], "[4] type_text: not run");
    const sent = ((await harness.requests()) as { method?: string }[]).slice(before).map((entry) => entry.method);
    assert.deepEqual(sent.filter((method) => ["click", "type_text", "press_key"].includes(String(method))), ["click", "type_text"]);
  } finally {
    await harness.close();
  }
});

test("ticket 142: a refused first step of a batch sends nothing, and its log event is background_unavailable with delivery and reason", async () => {
  const harness = await serverFor([CASES[3]]);
  try {
    await shoot(harness);
    const { result, lines } = await runBatch(harness, [
      { action: "scroll", window_id: "5", x: 20, y: 30, direction: "down", delivery_mode: "background" },
      { action: "wait", seconds: 0.1 },
    ]);
    assert.equal(result.isError, true);
    assert.equal(lines[1], "stopped at step 1 of 2: background_unavailable");
    assert.deepEqual(await methodsAfterShot(harness), []);
    await harness.finish();
    const calls = await toolCalls(harness, "computer_scroll");
    assert.deepEqual([calls[0]?.step, calls[0]?.result, calls[0]?.delivery, calls[0]?.reason], [1, "background_unavailable", "background", "tool_needs_foreground"]);
  } finally {
    await harness.close();
  }
});

test("ticket 142: an unknown delivery_mode in a batch step rejects the whole batch before any step runs", async () => {
  const harness = await serverFor([CASES[0]]);
  try {
    const { result, lines } = await runBatch(harness, [
      { action: "wait", seconds: 0.1 },
      { action: "type_text", window_id: "5", text: "hi", delivery_mode: "auto" },
    ]);
    assert.equal(result.isError, true);
    assert.equal(lines[0], "invalid_params");
    assert.match(lines.slice(1).join("\n"), /step 2 of 2 \(type_text\).*delivery_mode/);
    assert.deepEqual(((await harness.requests()) as { method?: string }[]).map((entry) => entry.method).filter((method) => method !== "ping"), []);
  } finally {
    await harness.close();
  }
});
