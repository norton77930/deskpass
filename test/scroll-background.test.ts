import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Reference-gaps G5 (ticket 09), seam A (the server with a scripted helper): computer_scroll is delivered in the
// background by default, through the helper's ScrollPattern path, with the mechanisms a background click has
// (no activation, the overlay, the stale-screenshot and minimized checks, the restore and the wait for the user,
// the user's switch). An explicit "foreground" is the wheel as before.

const NOTE = "The user turned off background operation, so this action was done in the foreground.";
const RETRY = 'Retry this action with delivery_mode: "foreground"';

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

type Logged = { method?: string; params?: Record<string, unknown> };
type ToolResult = { content?: unknown; isError?: boolean; structuredContent?: unknown };

const many = (handler: FakeHandler, count = 8): FakeHandler[] => Array.from({ length: count }, () => handler);

/** A server whose helper answers `scroll` with `answer`; `window` is what get_window answers after the screenshot. */
async function connect(answer: FakeHandler = { result: {} }, window: Record<string, unknown> = demo): Promise<Harness> {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, ...many({ result: window })],
        capture_window: [{ result: picture }],
        activate_window: many({ result: {} }),
        scroll: many(answer),
      },
    },
  });
  const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
  assert.notEqual(shot.isError, true);
  return harness;
}

/** One scroll call after the screenshot: the result, and the helper requests it made. */
async function scroll(harness: Harness, extra: Record<string, unknown> = {}, tool = "computer_scroll") {
  const before = ((await harness.requests()) as Logged[]).length;
  const args = { window_id: "5", x: 20, y: 30, direction: "down", ...extra };
  const result = (await harness.client.callTool({ name: tool, arguments: args })) as ToolResult;
  const made = ((await harness.requests()) as Logged[]).slice(before);
  return { result, text: textContent(result), made, order: made.map((entry) => entry.method ?? ""), sent: made.find((entry) => entry.method === "scroll") };
}

async function toolCalls(harness: Harness): Promise<Record<string, unknown>[]> {
  await harness.finish();
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call" && event.tool === "computer_scroll");
}

const code = (text: string) => text.split(/\r?\n/)[0] ?? "";

// The pin: what an explicit foreground scroll does today, which must not change.

test("an explicit foreground scroll is the wheel as before: overlay, activation, then a scroll request without delivery_mode", async () => {
  const harness = await connect();
  try {
    const { result, text, order, sent } = await scroll(harness, { delivery_mode: "foreground" });
    assert.notEqual(result.isError, true, text);
    assert.equal(text, "Scrolled down by 3.");
    assert.equal(result.structuredContent, undefined);
    assert.ok(order.indexOf("overlay_show") < order.indexOf("activate_window"), order.join(", "));
    assert.ok(order.indexOf("activate_window") < order.indexOf("scroll"), order.join(", "));
    assert.deepEqual(sent?.params, { window_id: "5", expected_app: demo.app, x: 20, y: 30, direction: "down", amount: 3 });
    const further = await scroll(harness, { delivery_mode: "foreground", direction: "left", amount: 7 });
    assert.equal(further.text, "Scrolled left by 7.");
    assert.deepEqual(further.sent?.params, { window_id: "5", expected_app: demo.app, x: 20, y: 30, direction: "left", amount: 7 });
    const calls = await toolCalls(harness);
    assert.deepEqual(calls.map((call) => [call.result, call.delivery]), [["ok", "foreground"], ["ok", "foreground"]]);
  } finally {
    await harness.close();
  }
});

test("an explicit foreground scroll still names a foreground that moved", async () => {
  const harness = await connect({ result: { foreground_changed: { foreground: null, target_closed: false } } });
  try {
    const { text } = await scroll(harness, { delivery_mode: "foreground" });
    assert.equal(
      text,
      "Scrolled down by 3. After the scroll, the target window no longer has the foreground, and no window that computer_list_windows lists has it. Call computer_list_windows to see which windows are open before continuing.",
    );
  } finally {
    await harness.close();
  }
});

// The new default.

test("by default a scroll is delivered in the background: the helper is told so and the window is not activated", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 3, percent_before: 0, percent_after: 15, moved: true } });
  try {
    for (const extra of [{}, { delivery_mode: "background" }]) {
      const { result, text, order, sent } = await scroll(harness, extra);
      assert.notEqual(result.isError, true, text);
      assert.ok(!order.includes("activate_window"), order.join(", "));
      assert.ok(order.indexOf("overlay_show") < order.indexOf("scroll"), order.join(", "));
      assert.deepEqual(sent?.params, { window_id: "5", expected_app: demo.app, x: 20, y: 30, direction: "down", amount: 3, delivery_mode: "background" });
    }
    const calls = await toolCalls(harness);
    assert.deepEqual(calls.map((call) => [call.result, call.delivery]), [["ok", "background"], ["ok", "background"]]);
  } finally {
    await harness.close();
  }
});

test("a background scroll passes direction and amount on, and the coordinate as a window pixel", async () => {
  const harness = await connect({ result: { steps: 5, percent_before: 40, percent_after: 20, moved: true } });
  try {
    const { sent } = await scroll(harness, { x: 100, y: 150, direction: "right", amount: 5 });
    assert.deepEqual(sent?.params, { window_id: "5", expected_app: demo.app, x: 100, y: 150, direction: "right", amount: 5, delivery_mode: "background" });
  } finally {
    await harness.close();
  }
});

test("the result reports the scroll percentages before and after, and says a small increment is not a wheel notch", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 3, percent_before: 0, percent_after: 15.84, moved: true } });
  try {
    const { result, text } = await scroll(harness);
    assert.notEqual(result.isError, true, text);
    assert.match(text, /^Scrolled down in the background by 3 small increments at window pixel \(20, 30\): the vertical scroll position went from 0% to 15\.8%\./);
    assert.match(text, /not the same distance as a wheel notch/);
    assert.deepEqual(result.structuredContent, { moved: true, scroll_percent_before: 0, scroll_percent_after: 15.8 });
    const horizontal = await scroll(harness, { direction: "left" });
    assert.match(horizontal.text, /^Scrolled left in the background by 3 small increments at window pixel \(20, 30\): the horizontal scroll position went from 0% to 15\.8%\./);
  } finally {
    await harness.close();
  }
});

test("a scroll the helper did through posted wheel messages says wheel notches, not small increments", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 3, via: "wheel_message", percent_before: 0, percent_after: 15.84, moved: true } });
  try {
    const { result, text } = await scroll(harness);
    assert.notEqual(result.isError, true, text);
    assert.match(text, /^Scrolled down in the background by 3 wheel notches at window pixel \(20, 30\): the vertical scroll position went from 0% to 15\.8%\./);
    assert.doesNotMatch(text, /increment/);
    assert.match(text, /screenshot/);
    const one = await connect({ result: { action: "scroll", steps: 1, via: "wheel_message", percent_before: 0, percent_after: 5, moved: true } });
    try {
      assert.match((await scroll(one, { amount: 1 })).text, /by 1 wheel notch at/);
    } finally {
      await one.close();
    }
    const short = await connect({ result: { action: "scroll", steps: 12, via: "wheel_message", percent_before: 0, percent_after: 30, moved: true } });
    try {
      assert.match((await scroll(short, { amount: 50 })).text, /Only 12 of the 50 requested notches were done/);
    } finally {
      await short.close();
    }
  } finally {
    await harness.close();
  }
});

test("an element already at the end is a success with moved: false, not a refusal", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 3, percent_before: 100, percent_after: 100, moved: false } });
  try {
    const { result, text } = await scroll(harness);
    assert.notEqual(result.isError, true, text);
    assert.match(text, /already at the end/);
    assert.match(text, /scroll position is 100%/);
    assert.deepEqual(result.structuredContent, { moved: false, scroll_percent_before: 100, scroll_percent_after: 100 });
    const calls = await toolCalls(harness);
    assert.equal(calls[0]?.result, "ok");
  } finally {
    await harness.close();
  }
});

test("a scroll whose position could not be read back still succeeds and says so", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 3 } });
  try {
    const { result, text } = await scroll(harness);
    assert.notEqual(result.isError, true, text);
    assert.match(text, /could not be read back/);
    assert.match(text, /screenshot/);
    assert.equal(result.structuredContent, undefined);
  } finally {
    await harness.close();
  }
});

test("a scroll the time allowed cut short says how many of the increments were done", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 12, percent_before: 0, percent_after: 30, moved: true } });
  try {
    const { text } = await scroll(harness, { amount: 50 });
    assert.match(text, /by 12 small increments/);
    assert.match(text, /12 of the 50 requested/);
  } finally {
    await harness.close();
  }
});

test("the helper's no_scroll_pattern reaches the model with its reason and a foreground retry", async () => {
  const message = `Nothing at that point of the target window scrolls in that direction, so the scroll cannot be delivered in the background; nothing was sent. ${RETRY}.`;
  const harness = await connect({ error: { code: "background_unavailable", message, data: { reason: "no_scroll_pattern", recommended: "foreground", input_sent: false } } });
  try {
    const { result, text } = await scroll(harness);
    assert.equal(result.isError, true);
    assert.equal(text, `background_unavailable\n${message}`);
    assert.deepEqual(result.structuredContent, { reason: "no_scroll_pattern", recommended: "foreground" });
    const calls = await toolCalls(harness);
    assert.deepEqual([calls[0]?.result, calls[0]?.delivery, calls[0]?.reason], ["background_unavailable", "background", "no_scroll_pattern"]);
  } finally {
    await harness.close();
  }
});

test("a minimized window is background_unavailable (minimized) before the overlay, activation or any scroll", async () => {
  const harness = await connect({ result: {} }, { ...demo, minimized: true });
  try {
    const { result, text, order } = await scroll(harness);
    assert.equal(code(text), "background_unavailable", text);
    assert.ok(text.includes(RETRY), text);
    assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
    for (const method of ["overlay_show", "activate_window", "scroll"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("a window resized since the screenshot is stale_screenshot in the background too, and nothing is sent", async () => {
  const harness = await connect({ result: {} }, { ...demo, width: 500 });
  try {
    const { text, order } = await scroll(harness);
    assert.equal(code(text), "stale_screenshot", text);
    for (const method of ["overlay_show", "activate_window", "scroll"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("a point outside the screenshot and a missing screenshot are refused before anything is sent", async () => {
  const harness = await connect();
  try {
    const outside = await scroll(harness, { x: 5000, y: 5 });
    assert.equal(code(outside.text), "out_of_bounds", outside.text);
    assert.deepEqual(outside.order, []);
    const unknown = await scroll(harness, { screenshot_id: "ss_99" });
    assert.equal(code(unknown.text), "no_screenshot", unknown.text);
    assert.deepEqual(unknown.order, []);
  } finally {
    await harness.close();
  }
});

test("a bad direction, a bad amount and an unknown delivery_mode are invalid_params and nothing is sent to scroll", async () => {
  const harness = await connect();
  try {
    for (const extra of [{ direction: "sideways" }, { amount: 0 }, { amount: 51 }, { amount: 2.5 }, { delivery_mode: "auto" }, { delivery_mode: "" }]) {
      const { text, order } = await scroll(harness, extra);
      assert.equal(code(text), "invalid_params", `${JSON.stringify(extra)}: ${text}`);
      assert.ok(!order.includes("scroll") && !order.includes("activate_window"), order.join(", "));
    }
  } finally {
    await harness.close();
  }
});

test("a foreground the helper gave back, a failed restore and the wait for the user are in the result, its structuredContent and the log", async () => {
  const moved = { action: "scroll", steps: 3, percent_before: 10, percent_after: 30, moved: true };
  const restored = await connect({ result: { ...moved, foreground_restored: true, waited_for_idle_ms: 900 } });
  try {
    const { result, text } = await scroll(restored);
    assert.notEqual(result.isError, true, text);
    assert.match(text, /took the foreground during the action; it was given back to the window that had it/);
    assert.match(text, /waited 900 ms \(waited_for_idle_ms\) for the user to stop typing and release the mouse/);
    assert.deepEqual(result.structuredContent, { moved: true, scroll_percent_before: 10, scroll_percent_after: 30, foreground_restored: true, waited_for_idle_ms: 900 });
    const calls = await toolCalls(restored);
    assert.equal(calls[0]?.foreground_restored, true);
    assert.equal(calls[0]?.waited_for_idle_ms, 900);
  } finally {
    await restored.close();
  }
  const failed = await connect({
    result: { ...moved, foreground_changed: { foreground: { window_id: "42", title: "Demo", app: demo.app, app_name: "Demo" }, target_closed: false }, restore_failed: true },
  });
  try {
    const { result, text } = await scroll(failed);
    assert.notEqual(result.isError, true, text);
    assert.match(text, /The scroll completed, but the target app took the foreground from the user's window and it could not be given back \(the foreground is now window_id 42\)\. Do not repeat the action\./);
    assert.deepEqual(result.structuredContent, { moved: true, scroll_percent_before: 10, scroll_percent_after: 30, restore_failed: true });
    assert.equal((await toolCalls(failed))[0]?.restore_failed, true);
  } finally {
    await failed.close();
  }
});

test("a foreground that another window holds after a background scroll is named, as for a click", async () => {
  const harness = await connect({
    result: { action: "scroll", steps: 3, percent_before: 10, percent_after: 30, moved: true, foreground_changed: { foreground: null, target_closed: false } },
  });
  try {
    const { text } = await scroll(harness);
    assert.match(text, /After the scroll, the target window no longer has the foreground/);
  } finally {
    await harness.close();
  }
});

// The user's switch.

test("with the switch off a default or background scroll is the wheel in the foreground, with the one line saying so", async () => {
  const harness = await connect();
  try {
    await writeFile(join(harness.dir, "settings.json"), JSON.stringify({ allow_background: false }));
    for (const extra of [{}, { delivery_mode: "background" }]) {
      const { result, text, order, sent } = await scroll(harness, extra);
      assert.notEqual(result.isError, true, text);
      assert.ok(order.indexOf("activate_window") < order.indexOf("scroll"), order.join(", "));
      assert.deepEqual(sent?.params, { window_id: "5", expected_app: demo.app, x: 20, y: 30, direction: "down", amount: 3 });
      assert.equal(text.split("\n").at(-1), NOTE);
      assert.equal(text.split("\n")[0], "Scrolled down by 3.");
    }
    const explicit = await scroll(harness, { delivery_mode: "foreground" });
    assert.ok(!explicit.text.includes(NOTE), explicit.text);
    const calls = await toolCalls(harness);
    assert.deepEqual(calls.map((call) => call.delivery), ["foreground", "foreground", "foreground"]);
  } finally {
    await harness.close();
  }
});

// Batch.

test("a batch scroll step takes the tool's default: background, with no activation", async () => {
  const harness = await connect({ result: { action: "scroll", steps: 2, percent_before: 0, percent_after: 8, moved: true } });
  try {
    const before = ((await harness.requests()) as Logged[]).length;
    const result = (await harness.client.callTool({
      name: "computer_batch",
      arguments: {
        steps: [
          { action: "scroll", window_id: "5", x: 20, y: 30, direction: "down", amount: 2 },
          { action: "scroll", window_id: "5", x: 20, y: 30, direction: "up", delivery_mode: "foreground" },
        ],
      },
    })) as ToolResult;
    const lines = textContent(result).split("\n");
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 2 of 2 steps");
    assert.match(lines[1] ?? "", /^\[1\] scroll: ok Scrolled down in the background by 2 small increments/);
    assert.match(lines[2] ?? "", /^\[2\] scroll: ok Scrolled up by 3\./);
    const made = ((await harness.requests()) as Logged[]).slice(before);
    const methods = made.map((entry) => entry.method);
    const firstScroll = methods.indexOf("scroll");
    assert.ok(!methods.slice(0, firstScroll).includes("activate_window"), methods.join(", "));
    const scrolls = made.filter((entry) => entry.method === "scroll");
    assert.equal(scrolls[0]?.params?.delivery_mode, "background");
    assert.ok(!("delivery_mode" in (scrolls[1]?.params ?? {})));
    assert.ok(methods.indexOf("activate_window") > firstScroll, "only the foreground step activates");
  } finally {
    await harness.close();
  }
});

test("a batch scroll step the helper refuses with no_scroll_pattern stops the batch like any failed step", async () => {
  const message = `Nothing at that point of the target window scrolls in that direction, so the scroll cannot be delivered in the background; nothing was sent. ${RETRY}.`;
  const harness = await connect({ error: { code: "background_unavailable", message, data: { reason: "no_scroll_pattern", recommended: "foreground", input_sent: false } } });
  try {
    const result = (await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "scroll", window_id: "5", x: 20, y: 30, direction: "down" }, { action: "wait", seconds: 0.1 }] },
    })) as ToolResult;
    const lines = textContent(result).split("\n");
    assert.equal(result.isError, true);
    assert.equal(lines[1], "stopped at step 1 of 2: background_unavailable");
    assert.match(lines[2] ?? "", /^\[1\] scroll: background_unavailable \(sent: false\) /);
    assert.equal(lines[3], "[2] wait: not run");
  } finally {
    await harness.close();
  }
});

// Tool descriptions.

type Listed = { name: string; description?: string; inputSchema: { properties?: Record<string, { description?: string; enum?: string[] }> } };

test("the descriptions say scroll defaults to background, that a notch and an increment differ, and the batch lists scroll among the background steps", async () => {
  const harness = await connectServer();
  try {
    const tools = (await harness.client.listTools()).tools as Listed[];
    const scrollTool = tools.find((tool) => tool.name === "computer_scroll");
    const text = scrollTool?.description ?? "";
    assert.match(text, /delivery_mode[^.]*defaults? to "background"/i);
    assert.match(text, /small increment/i);
    assert.match(text, /wheel notch/i);
    assert.match(text, /not (be )?the same distance/i);
    assert.match(text, /\bno_scroll_pattern\b/);
    assert.match(text, /\bminimized\b/);
    assert.match(text, /moved: false/);
    assert.match(text, /percent/i);
    assert.match(text, /use background first/i);
    assert.doesNotMatch(text, /tool_needs_foreground|has no background path/);
    const mode = scrollTool?.inputSchema.properties?.delivery_mode;
    assert.match(String(mode?.description), /Defaults to "background"/);
    assert.deepEqual(mode?.enum, ["background", "foreground"]);
    const batch = tools.find((tool) => tool.name === "computer_batch")?.description ?? "";
    assert.match(batch, /click, scroll, element_action and set_value background/);
  } finally {
    await harness.close();
  }
});
