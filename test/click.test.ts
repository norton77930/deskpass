import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { imageToWindow } from "../src/scaler.ts";
import { connectServer, textContent, type Harness } from "./harness.ts";

test("image coordinates convert by the ratio of physical size to image size", () => {
  assert.deepEqual(imageToWindow(100, 50, 400, 200, 200, 100), { x: 200, y: 100 });
  assert.deepEqual(imageToWindow(0, 0, 400, 200, 200, 100), { x: 0, y: 0 });
});

const demo = {
  window_id: "5",
  app: "C:\\Apps\\demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 10,
  y: 20,
  width: 4000,
  height: 2000,
  pid: 4242,
};

function captured(image: string) {
  return {
    image_base64: image,
    original_width: 4000,
    original_height: 2000,
    window_x: 10,
    window_y: 20,
    window_width: 4000,
    window_height: 2000,
  };
}

async function shot(harness: Harness) {
  return harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
}

test("a click uses the latest screenshot to convert image pixels into window pixels", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }, { result: demo }],
        capture_window: [{ result: captured("QQ==") }],
        activate_window: [{ result: {} }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    const pictured = await shot(harness);
    assert.notEqual(pictured.isError, true);
    const image = /image: (\d+)x(\d+)/.exec(textContent(pictured));
    assert.ok(image);
    const imageWidth = Number(image[1]);
    const imageHeight = Number(image[2]);
    const clicked = await harness.client.callTool({
      name: "computer_click",
      arguments: { window_id: "5", x: imageWidth / 2, y: imageHeight / 2, modifiers: ["ctrl"], delivery_mode: "foreground" },
    });
    assert.notEqual(clicked.isError, true);
    const requests = await harness.requests();
    const click = requests.find((entry) => (entry as { method?: string }).method === "click") as {
      params?: { x?: number; y?: number; button?: string; click_count?: number; modifiers?: string[] };
    };
    assert.equal(click.params?.x, Math.round((imageWidth / 2) * 4000 / imageWidth));
    assert.equal(click.params?.y, Math.round((imageHeight / 2) * 2000 / imageHeight));
    assert.equal(click.params?.button, "left");
    assert.equal(click.params?.click_count, 1);
    assert.deepEqual(click.params?.modifiers, ["ctrl"]);
  } finally {
    await harness.close();
  }
});

test("clicking without a screenshot or outside the image does not send input", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }],
        capture_window: [{ result: captured("QQ==") }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    const missing = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 1, y: 1 } });
    assert.equal(textContent(missing).split(/\r?\n/)[0], "no_screenshot");
    const pictured = await shot(harness);
    const image = /image: (\d+)x(\d+)/.exec(textContent(pictured));
    assert.ok(image);
    const outside = await harness.client.callTool({
      name: "computer_click",
      arguments: { window_id: "5", x: Number(image[1]), y: 0 },
    });
    assert.equal(textContent(outside).split(/\r?\n/)[0], "out_of_bounds");
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "click"), false);
  } finally {
    await harness.close();
  }
});

test("element_index together with x and y is rejected before approval or input", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { click: [{ result: {} }] } },
  });
  try {
    const result = await harness.client.callTool({
      name: "computer_click",
      arguments: { window_id: "5", x: 1, y: 1, element_index: 1 },
    });
    assert.equal(textContent(result).split(/\r?\n/)[0], "invalid_params");
    assert.equal(harness.elicitations.length, 0);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "click"), false);
  } finally {
    await harness.close();
  }
});

test("activate and click go through approval", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [
          { result: { ...demo, width: 200, height: 100 } },
          { result: { ...demo, width: 200, height: 100 } },
          { result: { ...demo, width: 200, height: 100 } },
        ],
        capture_window: [{ result: { ...captured("QQ=="), original_width: 200, original_height: 100, window_width: 200, window_height: 100 } }],
        activate_window: [{ result: {} }, { result: {} }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    const activated = await harness.client.callTool({ name: "computer_activate_window", arguments: { window_id: "5" } });
    assert.notEqual(activated.isError, true);
    assert.equal(harness.elicitations.length, 1);
    await shot(harness);
    const clicked = await harness.client.callTool({
      name: "computer_click",
      arguments: { window_id: "5", x: 10, y: 10, button: "right", click_count: 2, delivery_mode: "foreground" },
    });
    assert.notEqual(clicked.isError, true);
    assert.equal(harness.elicitations.length, 1);
    const requests = await harness.requests();
    const click = requests.find((entry) => (entry as { method?: string }).method === "click") as { params?: { button?: string; click_count?: number } };
    assert.equal(click.params?.button, "right");
    assert.equal(click.params?.click_count, 2);
  } finally {
    await harness.close();
  }
});

test("activating a window that another app's window still covers succeeds but names that window", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }],
        activate_window: [
          {
            result: {
              covered_by: [{ window_id: "77", title: "Call with Ada", app: "C:\\Apps\\meet.exe", app_name: "Meet" }],
            },
          },
        ],
      },
    },
  });
  try {
    const activated = await harness.client.callTool({ name: "computer_activate_window", arguments: { window_id: "5" } });
    assert.notEqual(activated.isError, true);
    const message = textContent(activated);
    assert.match(message, /Brought Demo to the foreground/);
    assert.match(message, /still covered/i);
    assert.match(message, /Meet \(C:\\Apps\\meet\.exe\)/);
    assert.match(message, /window_id:? 77\b/);
    assert.match(message, /Call with Ada/);
    assert.match(message, /target_obscured/);
    assert.match(message, /move or close/i);
  } finally {
    await harness.close();
  }
});

test("input at a point another app's window covers is target_obscured naming that window", async () => {
  const obscured = {
    error: {
      code: "target_obscured",
      message: "The point is covered.",
      data: { window_id: "77", title: "Call with Ada", app: "C:\\Apps\\meet.exe", app_name: "Meet" },
    },
  };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }, { result: demo }],
        capture_window: [{ result: captured("QQ==") }],
        activate_window: [{ result: {} }, { result: {} }],
        click: [obscured],
        drag: [obscured],
      },
    },
  });
  try {
    await shot(harness);
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 10, y: 10 } });
    const dragged = await harness.client.callTool({
      name: "computer_drag",
      arguments: { window_id: "5", from_x: 10, from_y: 10, to_x: 20, to_y: 20 },
    });
    for (const result of [clicked, dragged]) {
      assert.equal(result.isError, true);
      const [code, ...rest] = textContent(result).split(/\r?\n/);
      const message = rest.join("\n");
      assert.equal(code, "target_obscured");
      assert.match(message, /Meet/);
      assert.match(message, /C:\\Apps\\meet\.exe/);
      assert.match(message, /Call with Ada/);
      assert.match(message, /window_id:? 77\b/);
      assert.match(message, /no input was sent/i);
      assert.match(message, /move or close/i);
    }
  } finally {
    await harness.close();
  }
});

async function obscuredClick(cover: Record<string, unknown>): Promise<string> {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        capture_window: [{ result: captured("QQ==") }],
        activate_window: [{ result: {} }],
        click: [{ error: { code: "target_obscured", message: "The point is covered.", data: cover } }],
      },
    },
  });
  try {
    await shot(harness);
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 10, y: 10 } });
    assert.equal(clicked.isError, true);
    const [code, ...rest] = textContent(clicked).split(/\r?\n/);
    assert.equal(code, "target_obscured");
    return rest.join("\n");
  } finally {
    await harness.close();
  }
}

const meet = { window_id: "77", title: "Call with Ada", app: "C:\\Apps\\meet.exe", app_name: "Meet" };

test("target_obscured by a window that is not always on top suggests activating the target", async () => {
  const message = await obscuredClick({ ...meet, topmost: false });
  assert.match(message, /Meet/);
  assert.match(message, /window_id:? 77\b/);
  assert.match(message, /no input was sent/i);
  assert.match(message, /computer_activate_window/);
  assert.doesNotMatch(message, /move or close/i);
  assert.doesNotMatch(message, /stays on top/i);
});

test("target_obscured by an always-on-top window keeps the move-or-close advice", async () => {
  const message = await obscuredClick({ ...meet, topmost: true });
  assert.match(message, /stays on top/i);
  assert.match(message, /move or close/i);
  assert.doesNotMatch(message, /computer_activate_window/);
});

test("target_obscured without the topmost field keeps the move-or-close advice", async () => {
  const message = await obscuredClick(meet);
  assert.match(message, /stays on top/i);
  assert.match(message, /move or close/i);
  assert.doesNotMatch(message, /computer_activate_window/);
});

async function activateCoveredBy(covers: Record<string, unknown>[]): Promise<string> {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { get_window: [{ result: demo }], activate_window: [{ result: { covered_by: covers } }] } },
  });
  try {
    const activated = await harness.client.callTool({ name: "computer_activate_window", arguments: { window_id: "5" } });
    assert.notEqual(activated.isError, true);
    return textContent(activated);
  } finally {
    await harness.close();
  }
}

test("activating a window that a non-topmost window still covers asks the user to click the target", async () => {
  const message = await activateCoveredBy([{ ...meet, topmost: false }]);
  assert.match(message, /Brought Demo to the foreground/);
  assert.match(message, /still covered/i);
  assert.match(message, /Meet \(C:\\Apps\\meet\.exe\)/);
  assert.match(message, /window_id:? 77\b/);
  assert.match(message, /target_obscured/);
  assert.match(message, /ask the user to click the target window/i);
  assert.match(message, /bring it forward/i);
  assert.doesNotMatch(message, /stays on top/i);
  assert.doesNotMatch(message, /move or close/i);
});

test("activating a window that an always-on-top window covers keeps the move-or-close advice", async () => {
  const message = await activateCoveredBy([{ ...meet, topmost: true }]);
  assert.match(message, /stays on top/i);
  assert.match(message, /move or close/i);
  assert.doesNotMatch(message, /click the target/i);
});

test("activating a window covered by both kinds gives each its own advice", async () => {
  const message = await activateCoveredBy([
    { ...meet, topmost: true },
    { window_id: "88", title: "Notes", app: "C:\\Apps\\notes.exe", app_name: "Notes", topmost: false },
  ]);
  assert.match(message, /move or close/i);
  assert.match(message, /ask the user to click the target window/i);
});

/** The text of a click whose helper result is `result`. */
async function clickWith(result: unknown) {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        capture_window: [{ result: captured("QQ==") }],
        activate_window: [{ result: {} }],
        click: [{ result }],
      },
    },
  });
  try {
    await shot(harness);
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 10, y: 10, delivery_mode: "foreground" } });
    assert.notEqual(clicked.isError, true, textContent(clicked));
    return textContent(clicked);
  } finally {
    await harness.close();
  }
}

test("a click after which the target closed and another window of its app is in front succeeds and names it", async () => {
  const message = await clickWith({
    foreground_changed: { foreground: { window_id: "33883792", title: "Guest", app: "C:\\Apps\\demo.exe", app_name: "Demo" }, target_closed: true },
  });
  assert.match(message, /^Clicked left 1 time\(s\)/);
  assert.match(message, /The target window closed after the click; the foreground is now another window of Demo \(window_id: 33883792, title: "Guest"\)\./);
  assert.match(message, /Take a screenshot of window_id 33883792 before continuing\./);
});

test("a click after which another app's window took the foreground does not say the click went to that app", async () => {
  const message = await clickWith({
    foreground_changed: { foreground: { window_id: "77", title: "Call with Ada", app: "C:\\Apps\\meet.exe", app_name: "Meet" }, target_closed: false },
  });
  assert.match(message, /After the click, the foreground moved to a window of Meet \(C:\\Apps\\meet\.exe\) \(window_id: 77, title: "Call with Ada"\), a different app\./);
  assert.match(message, /The click went to the target window of Demo, not to Meet\./);
  assert.doesNotMatch(message, /closed/);
  assert.match(message, /Take a screenshot of window_id 77 before continuing\./);
});

test("a click after which the target closed and no listed window is in front says so", async () => {
  const message = await clickWith({ foreground_changed: { foreground: null, target_closed: true } });
  assert.match(message, /The target window closed after the click, and no window that computer_list_windows lists is in the foreground\./);
  assert.match(message, /Call computer_list_windows/);
  assert.doesNotMatch(message, /window_id/);
});

test("a click that left the target in front says nothing about the foreground", async () => {
  const message = await clickWith({});
  assert.match(message, /^Clicked left 1 time\(s\) at window pixel \(\d+, \d+\)\.$/);
});

// Ticket 141: a click at a point (x, y) is delivered in the background by default (seam A, the server
// with a scripted helper): the window is not activated, the helper is told the mode and finds the
// element in the window's own UI tree; a click that needs the pointer is background_unavailable
// (needs_pointer) before any overlay or input, the helper's no_element_at_point reaches the model, and
// delivery_mode "foreground" is the click with the real mouse as before.

const RETRY = 'Retry this action with delivery_mode: "foreground"';

type ToolResult = { content?: unknown; isError?: boolean; structuredContent?: unknown };
type Logged = { method?: string; params?: Record<string, unknown> };

/** The day's `tool_call` events of computer_click in `harness`'s log. */
async function toolCalls(harness: Harness): Promise<Record<string, unknown>[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call" && event.tool === "computer_click");
}

/** One click at a point of the 4000x2000 screenshot after taking it; `window` is what get_window answers for the click. */
async function pointClick(extra: Record<string, unknown>, answer: { result?: unknown; error?: unknown } = { result: { action: "invoke" } }, window: Record<string, unknown> = demo) {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: window }, { result: window }],
        capture_window: [{ result: captured("QQ==") }],
        activate_window: [{ result: {} }],
        click: [answer],
      },
    },
  });
  try {
    await shot(harness);
    const result = (await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 500, y: 250, ...extra } })) as ToolResult;
    await harness.finish();
    const requests = (await harness.requests()) as Logged[];
    const order = requests.map((entry) => entry.method ?? "");
    return { result, text: textContent(result), sent: requests.filter((entry) => entry.method === "click"), order: order.slice(order.lastIndexOf("capture_window") + 1), calls: await toolCalls(harness) };
  } finally {
    await harness.close();
  }
}

function codeOf(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

test("ticket 141: by default a click at a point is delivered in the background: the helper is told so and the window is not activated", async () => {
  const { result, text, sent, order, calls } = await pointClick({});
  assert.notEqual(result.isError, true, text);
  const params = sent[0]?.params;
  assert.equal(params?.delivery_mode, "background");
  assert.equal(params?.button, "left");
  assert.equal(params?.click_count, 1);
  assert.deepEqual(params?.modifiers, []);
  assert.ok(!order.includes("activate_window"), order.join(", "));
  assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("click"), order.join(", "));
  assert.equal(calls[0]?.delivery, "background");
  const explicit = await pointClick({ delivery_mode: "background", button: "left", click_count: 1, modifiers: [] });
  assert.equal(explicit.sent[0]?.params?.delivery_mode, "background");
  assert.ok(!explicit.order.includes("activate_window"), explicit.order.join(", "));
});

test("ticket 141: the result names the pattern the click used on the element at the point and what it read back", async () => {
  const cases: [Record<string, unknown>, RegExp][] = [
    [{ action: "invoke" }, /^Background click \(invoke\): Invoked the element at window pixel \(\d+, \d+\)\. Take a screenshot/],
    [{ action: "toggle", state: "on", previous_state: "off" }, /^Background click \(toggle\): Toggled the element at window pixel \(\d+, \d+\); it now reads on\.$/],
    [{ action: "select", state: "selected" }, /^Background click \(select\): Selected the element at window pixel \(\d+, \d+\); it now reads selected\.$/],
    [{ action: "expand", state: "expanded" }, /^Background click \(expand\): Expanded the element at window pixel \(\d+, \d+\); it now reads expanded\.$/],
  ];
  for (const [answer, expected] of cases) {
    const { result, text } = await pointClick({}, { result: answer });
    assert.notEqual(result.isError, true, text);
    assert.match(text, expected);
    assert.equal(result.structuredContent, undefined);
  }
});

test("ticket 141: a right click, a double click and held modifiers at a point are background_unavailable (needs_pointer): no overlay, activation or input", async () => {
  for (const extra of [{ button: "right" }, { button: "middle" }, { click_count: 2 }, { click_count: 3 }, { modifiers: ["Shift"] }, { button: "right", click_count: 2, modifiers: ["ctrl"] }]) {
    const { result, text, order, sent, calls } = await pointClick(extra);
    assert.equal(result.isError, true);
    assert.equal(codeOf(result), "background_unavailable", `${JSON.stringify(extra)}: ${text}`);
    assert.ok(text.includes(RETRY), text);
    assert.deepEqual(result.structuredContent, { reason: "needs_pointer", recommended: "foreground" });
    assert.equal(sent.length, 0);
    for (const method of ["overlay_show", "activate_window", "click"]) assert.ok(!order.includes(method), `${method}: ${order.join(", ")}`);
    assert.equal(calls[0]?.reason, "needs_pointer");
    assert.equal(calls[0]?.delivery, "background");
  }
});

test("ticket 141: the helper's no_element_at_point reaches the model with its message and its reason", async () => {
  const message = `No element that can be acted on was found at that point of the target window, so the click cannot be delivered in the background; nothing was sent. ${RETRY}.`;
  const { result, text, calls } = await pointClick({}, { error: { code: "background_unavailable", message, data: { reason: "no_element_at_point", recommended: "foreground", input_sent: false } } });
  assert.equal(text, `background_unavailable\n${message}`);
  assert.deepEqual(result.structuredContent, { reason: "no_element_at_point", recommended: "foreground" });
  assert.equal(calls[0]?.result, "background_unavailable");
  assert.equal(calls[0]?.reason, "no_element_at_point");
});

test("ticket 141: a minimized window in background delivery is background_unavailable (minimized): no overlay, activation or input", async () => {
  const { result, text, order, sent } = await pointClick({}, undefined, { ...demo, minimized: true });
  assert.equal(codeOf(result), "background_unavailable", text);
  assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
  assert.equal(sent.length, 0);
  for (const method of ["overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
});

test("ticket 141: delivery_mode foreground clicks at the point with the real mouse as before: activation first, then the click", async () => {
  const { result, text, sent, order, calls } = await pointClick({ delivery_mode: "foreground", button: "right", click_count: 2, modifiers: ["Shift"] }, { result: {} });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /^Clicked right 2 time\(s\) at window pixel \(\d+, \d+\)\.$/);
  assert.equal(sent[0]?.params?.delivery_mode, "foreground");
  assert.ok(order.lastIndexOf("activate_window") < order.lastIndexOf("click"), order.join(", "));
  assert.equal(result.structuredContent, undefined);
  assert.equal(calls[0]?.delivery, "foreground");
});

test("ticket 141: an unknown delivery_mode on a click at a point is invalid_params before approval, overlay or input, without quoting it", async () => {
  for (const mode of ["auto", "Background", "", 1, null, true]) {
    const { result, text, order } = await pointClick({ delivery_mode: mode });
    assert.equal(codeOf(result), "invalid_params", `${JSON.stringify(mode)}: ${text}`);
    assert.match(text, /delivery_mode/);
    assert.ok(!text.includes("auto"), text);
    for (const method of ["click", "overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
  }
});

test("ticket 141: a foreground the helper gave back, a failed restore and the wait for the user are in the result, its structuredContent and the log", async () => {
  const restored = await pointClick({}, { result: { action: "toggle", state: "on", previous_state: "off", foreground_restored: true, waited_for_idle_ms: 1200 } });
  assert.notEqual(restored.result.isError, true);
  assert.match(restored.text, /took the foreground during the action; it was given back to the window that had it/);
  assert.match(restored.text, /waited 1200 ms \(waited_for_idle_ms\) for the user to stop typing and release the mouse/);
  assert.deepEqual(restored.result.structuredContent, { foreground_restored: true, waited_for_idle_ms: 1200 });
  assert.equal(restored.calls[0]?.foreground_restored, true);
  assert.equal(restored.calls[0]?.waited_for_idle_ms, 1200);
  const failed = await pointClick({}, { result: { action: "invoke", foreground_changed: { foreground: { window_id: "42", title: "Demo", app: demo.app, app_name: "Demo" }, target_closed: false }, restore_failed: true } });
  assert.notEqual(failed.result.isError, true);
  assert.match(failed.text, /The click completed, but the target app took the foreground from the user's window and it could not be given back \(the foreground is now window_id 42\)\. Do not repeat the action\./);
  assert.deepEqual(failed.result.structuredContent, { restore_failed: true });
  assert.equal(failed.calls[0]?.restore_failed, true);
});
