import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { readAllowBackground, writeAllowBackground } from "../src/settings-file.ts";
import { connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 146 (seam A, the server with a scripted helper): the settings file's "Allow background operation" switch.
// On (the default, also for a settings file without it) the tools deliver as they were asked; off, every
// element_action, set_value, click and batch step is delivered in the foreground, and the result says so.
// The switch is read at every tool call, so a change made while the server runs counts at the next call.

const NOTE = "The user turned off background operation, so this action was done in the foreground.";

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

const elements = [
  { depth: 0, index: 1, role: "edit", name: "Search", value: "", bounds: { x: 10, y: 10, width: 100, height: 20 }, states: [], actions: ["click", "invoke", "toggle", "set_value"] },
];

const many = (handler: FakeHandler, count = 12): FakeHandler[] => Array.from({ length: count }, () => handler);

type Logged = { method?: string; params?: Record<string, unknown> };
type ToolResult = { content?: unknown; isError?: boolean };

async function connect(): Promise<Harness> {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: many({ result: demo }),
        capture_window: many({ result: picture }),
        get_ui_tree: many({ result: { tree_id: "t1-aaaaa", window_title: "Demo window", elements } }),
        activate_window: many({ result: {} }),
        element_action: many({ result: { state: "on", previous_state: "off" } }),
        set_value: many({ result: { read_back: "x", read_back_matches: true } }),
        click: many({ result: { action: "invoke" } }),
        type_text: many({ result: {} }),
      },
    },
  });
  // The tree and the screenshot every call below refers to.
  await harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "5" } });
  await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
  return harness;
}

async function setSwitch(harness: Harness, allowed: boolean): Promise<void> {
  await writeFile(join(harness.dir, "settings.json"), JSON.stringify({ allow_background: allowed }));
}

type Call = readonly [tool: string, args: Record<string, unknown>, method: string];

const ELEMENT_ACTION: Call = ["computer_element_action", { window_id: "5", element_index: 1, action: "toggle" }, "element_action"];
const SET_VALUE: Call = ["computer_set_value", { window_id: "5", element_index: 1, value: "hi" }, "set_value"];
const CLICK_ELEMENT: Call = ["computer_click", { window_id: "5", element_index: 1 }, "click"];
const CLICK_POINT: Call = ["computer_click", { window_id: "5", x: 20, y: 30 }, "click"];
const ALL_CALLS = [ELEMENT_ACTION, SET_VALUE, CLICK_ELEMENT, CLICK_POINT];

/** One call; the helper requests it made (after the earlier ones), what it sent as `method`, and its text. */
async function call(harness: Harness, [tool, args, method]: Call, extra: Record<string, unknown> = {}) {
  const before = ((await harness.requests()) as Logged[]).length;
  const result = (await harness.client.callTool({ name: tool, arguments: { ...args, ...extra } })) as ToolResult;
  const made = ((await harness.requests()) as Logged[]).slice(before);
  const sent = made.find((entry) => entry.method === method);
  return { result, text: textContent(result), order: made.map((entry) => entry.method ?? ""), mode: sent?.params?.delivery_mode };
}

async function toolCalls(harness: Harness): Promise<Record<string, unknown>[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call");
}

test("the settings file: no file, a file without the switch or with a bad one is on; false is off; a write keeps the file's other settings", async () => {
  const dir = await mkdtemp(join(tmpdir(), "deskpass-switch-"));
  try {
    assert.equal(await readAllowBackground(dir), true, "no file");
    await writeFile(join(dir, "settings.json"), "{}");
    assert.equal(await readAllowBackground(dir), true, "no switch in the file");
    await writeFile(join(dir, "settings.json"), JSON.stringify({ allow_background: "no" }));
    assert.equal(await readAllowBackground(dir), true, "not a boolean");
    await writeFile(join(dir, "settings.json"), JSON.stringify({ allow_background: false }));
    assert.equal(await readAllowBackground(dir), false);
    await writeFile(join(dir, "settings.json"), "{ not json");
    assert.equal(await readAllowBackground(dir, false), false, "a file that cannot be read keeps what was read last");
    assert.equal(await readAllowBackground(dir, true), true);

    await writeFile(join(dir, "settings.json"), JSON.stringify({ other: 7 }));
    await writeAllowBackground(dir, false);
    assert.deepEqual(JSON.parse(await readFile(join(dir, "settings.json"), "utf8")), { other: 7, allow_background: false });
    await writeAllowBackground(dir, true);
    assert.equal(await readAllowBackground(dir), true);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("ticket 146: with no settings file the switch is on: each tool delivers in the background and says nothing about the switch", async () => {
  const harness = await connect();
  try {
    for (const entry of ALL_CALLS) {
      const done = await call(harness, entry);
      assert.notEqual(done.result.isError, true, `${entry[0]}: ${done.text}`);
      assert.equal(done.mode, "background", entry[0]);
      assert.ok(!done.order.includes("activate_window"), done.order.join(", "));
      assert.ok(!done.text.includes(NOTE), done.text);
    }
  } finally {
    await harness.close();
  }
});

test("ticket 146: with the switch off a background request (by default or explicit) is delivered in the foreground, with the one line saying so", async () => {
  const harness = await connect();
  try {
    await setSwitch(harness, false);
    for (const entry of ALL_CALLS) {
      for (const extra of [{}, { delivery_mode: "background" }]) {
        const done = await call(harness, entry, extra);
        const what = `${entry[0]} ${JSON.stringify(extra)}`;
        assert.notEqual(done.result.isError, true, `${what}: ${done.text}`);
        assert.equal(done.mode, "foreground", what);
        assert.ok(done.order.includes("activate_window"), `${what}: ${done.order.join(", ")}`);
        assert.ok(done.order.indexOf("activate_window") < done.order.lastIndexOf(entry[2]), `${what}: ${done.order.join(", ")}`);
        const lines = done.text.split("\n");
        assert.equal(lines.filter((line) => line === NOTE).length, 1, `${what}: ${done.text}`);
        assert.equal(lines.at(-1), NOTE, what);
      }
    }
  } finally {
    await harness.close();
  }
});

test("ticket 146: with the switch off an explicit foreground request needs no note", async () => {
  const harness = await connect();
  try {
    await setSwitch(harness, false);
    for (const entry of ALL_CALLS) {
      const done = await call(harness, entry, { delivery_mode: "foreground" });
      assert.equal(done.mode, "foreground", entry[0]);
      assert.ok(!done.text.includes(NOTE), done.text);
    }
  } finally {
    await harness.close();
  }
});

test("ticket 146: a change of the switch counts at the next call, with no restart", async () => {
  const harness = await connect();
  try {
    assert.equal((await call(harness, ELEMENT_ACTION)).mode, "background");
    await setSwitch(harness, false);
    const off = await call(harness, ELEMENT_ACTION);
    assert.equal(off.mode, "foreground");
    assert.ok(off.text.includes(NOTE));
    await setSwitch(harness, true);
    const on = await call(harness, ELEMENT_ACTION);
    assert.equal(on.mode, "background");
    assert.ok(!on.text.includes(NOTE));
    // A settings file that cannot be read keeps the last switch read.
    await setSwitch(harness, false);
    assert.equal((await call(harness, ELEMENT_ACTION)).mode, "foreground");
    await writeFile(join(harness.dir, "settings.json"), "{ not json");
    assert.equal((await call(harness, ELEMENT_ACTION)).mode, "foreground");
  } finally {
    await harness.close();
  }
});

test("ticket 146: with the switch off the log says foreground for each action", async () => {
  const harness = await connect();
  try {
    await setSwitch(harness, false);
    for (const entry of ALL_CALLS) await call(harness, entry);
    await harness.finish();
    const acted = (await toolCalls(harness)).filter((event) => ["computer_element_action", "computer_set_value", "computer_click"].includes(String(event.tool)));
    assert.equal(acted.length, ALL_CALLS.length);
    for (const event of acted) {
      assert.equal(event.delivery, "foreground", String(event.tool));
      assert.equal(event.result, "ok");
    }
  } finally {
    await harness.close();
  }
});

test("ticket 146: with the switch off each batch step is delivered in the foreground and its entry carries the line; an explicit foreground step does not", async () => {
  const harness = await connect();
  try {
    await setSwitch(harness, false);
    const before = ((await harness.requests()) as Logged[]).length;
    const result = (await harness.client.callTool({
      name: "computer_batch",
      arguments: {
        steps: [
          { action: "element_action", window_id: "5", element_index: 1, element_action: "toggle" },
          { action: "set_value", window_id: "5", element_index: 1, value: "hi", delivery_mode: "background" },
          { action: "click", window_id: "5", element_index: 1, delivery_mode: "foreground" },
        ],
      },
    })) as ToolResult;
    const lines = textContent(result).split("\n");
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 3 of 3 steps");
    assert.ok(lines[1]?.startsWith("[1] element_action: ok "), lines[1]);
    assert.equal(lines[2], NOTE);
    assert.ok(lines[3]?.startsWith("[2] set_value: ok "), lines[3]);
    assert.equal(lines[4], NOTE);
    assert.ok(lines[5]?.startsWith("[3] click: ok "), lines[5]);
    assert.equal(lines.length, 6, lines.join("\n"));
    const made = ((await harness.requests()) as Logged[]).slice(before);
    for (const method of ["element_action", "set_value", "click"]) {
      assert.equal(made.find((entry) => entry.method === method)?.params?.delivery_mode, "foreground", method);
    }
    await harness.finish();
    const steps = (await toolCalls(harness)).filter((event) => event.step !== null);
    assert.deepEqual(steps.map((event) => event.delivery), ["foreground", "foreground", "foreground"]);
  } finally {
    await harness.close();
  }
});

test("ticket 146: with the switch off a foreground-only tool asked for background is still refused as before (tool_needs_foreground)", async () => {
  const harness = await connect();
  try {
    await setSwitch(harness, false);
    const result = (await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "hi", delivery_mode: "background" } })) as ToolResult;
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split(/\r?\n/)[0], "background_unavailable");
    const plain = (await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "hi" } })) as ToolResult;
    assert.notEqual(plain.isError, true, textContent(plain));
    assert.ok(!textContent(plain).includes(NOTE));
  } finally {
    await harness.close();
  }
});
