import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 62: computer_click with element_index (seam A, the server with a scripted helper).

const demo = {
  window_id: "5",
  app: "C:\\Apps\\demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 10,
  y: 20,
  width: 400,
  height: 300,
  pid: 4242,
};
const other = { ...demo, window_id: "6", title: "Other" };

const elements = [
  { depth: 0, index: 1, role: "edit", name: "Search", bounds: { x: 10, y: 10, width: 100, height: 20 }, states: [], actions: ["click"] },
  { depth: 0, role: "text", name: "Label", states: [], actions: [] },
  { depth: 0, index: 2, role: "button", name: "OK", bounds: { x: 10, y: 40, width: 60, height: 20 }, states: [], actions: ["click", "invoke"] },
  { depth: 0, index: 3, role: "button", name: "Cancel", bounds: { x: 80, y: 40, width: 60, height: 20 }, states: [], actions: ["click", "invoke"] },
];

function treeResult(treeId: string) {
  return { result: { tree_id: treeId, window_title: "Demo window", elements } };
}

type Logged = { method?: string; params?: Record<string, unknown> };

async function logged(harness: Harness, method: string): Promise<Logged[]> {
  return ((await harness.requests()) as Logged[]).filter((entry) => entry.method === method);
}

function readTree(harness: Harness, windowId = "5") {
  return harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: windowId } });
}

function click(harness: Harness, extra: Record<string, unknown>) {
  return harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", ...extra } });
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

/** A server whose helper answers `get_window` with `windows` in turn and reads the trees named. */
function serverWith(handlers: Record<string, FakeHandler[]>) {
  return connectServer({ elicitation: "form", script: { handlers } });
}

test("computer_click requires only window_id and offers element_index and tree_id", async () => {
  const harness = await connectServer();
  try {
    const listed = await harness.client.listTools();
    const tool = listed.tools.find((entry) => entry.name === "computer_click");
    const schema = tool?.inputSchema as { properties?: Record<string, { type?: string }>; required?: string[] };
    assert.deepEqual(schema.required, ["window_id"]);
    assert.equal(schema.properties?.element_index?.type, "integer");
    assert.equal(schema.properties?.tree_id?.type, "string");
    assert.match(tool?.description ?? "", /element_index/);
  } finally {
    await harness.close();
  }
});

test("element_index with x or y, or neither, is invalid_params before approval or input", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    const read = await readTree(harness);
    assert.notEqual(read.isError, true, textContent(read));
    for (const extra of [{ element_index: 1, x: 1, y: 1 }, { element_index: 1, x: 1 }, { element_index: 1, y: 1 }, {}]) {
      const result = await click(harness, extra);
      assert.equal(code(result), "invalid_params", `${JSON.stringify(extra)}: ${textContent(result)}`);
      assert.match(textContent(result), /element_index/);
    }
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    assert.equal((await logged(harness, "click")).length, 0);
    assert.equal((await logged(harness, "get_window")).length, 1, "no window was looked at for the refused clicks");
  } finally {
    await harness.close();
  }
});

test("element_index or tree_id of the wrong type is invalid_params", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    for (const extra of [{ element_index: 0 }, { element_index: 1.5 }, { element_index: "2" }, { element_index: 1, tree_id: 7 }]) {
      const result = await click(harness, extra);
      assert.equal(code(result), "invalid_params", `${JSON.stringify(extra)}: ${textContent(result)}`);
    }
    assert.equal((await logged(harness, "click")).length, 0);
  } finally {
    await harness.close();
  }
});

test("a window whose tree was never read is no_ui_tree, asking for a tree first", async () => {
  const harness = await serverWith({});
  try {
    const result = await click(harness, { element_index: 1 });
    assert.equal(code(result), "no_ui_tree", textContent(result));
    assert.match(textContent(result), /computer_get_ui_tree/);
    assert.match(textContent(result), /include_ui_tree/);
    assert.equal(harness.elicitations.length, 0);
    assert.equal((await logged(harness, "click")).length, 0);
  } finally {
    await harness.close();
  }
});

test("a click by element sends the element of the window's latest tree, without a screenshot or a size check", async () => {
  // The window was resized since the tree was read: positions are read live, so no stale_screenshot.
  const resized = { ...demo, width: 640, height: 480 };
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }, { result: resized }],
    get_ui_tree: [treeResult("t1-aaaaa"), treeResult("t2-aaaaa")],
    activate_window: [{ result: {} }],
    click: [{ result: {} }],
  });
  try {
    await readTree(harness);
    await readTree(harness);
    // Ticket 140: the real mouse is delivery_mode "foreground"; the default is background.
    const result = await click(harness, { element_index: 2, button: "right", click_count: 2, modifiers: ["Shift"], screenshot_id: "ss_99", delivery_mode: "foreground" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /^Clicked right 2 time\(s\) on element \[2\] of tree t2-aaaaa\./);
    const [sent] = await logged(harness, "click");
    assert.deepEqual(sent?.params, {
      window_id: "5",
      expected_app: demo.app,
      element: { tree_id: "t2-aaaaa", index: 2 },
      button: "right",
      click_count: 2,
      modifiers: ["shift"],
      delivery_mode: "foreground",
    });
    const order = ((await harness.requests()) as Logged[]).map((entry) => entry.method);
    assert.ok(order.lastIndexOf("activate_window") < order.lastIndexOf("click"), order.join(", "));
    assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("click"), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("a tree_id picks that tree; one of another window is invalid_params", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }, { result: other }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa"), treeResult("t2-aaaaa"), treeResult("t3-aaaaa")],
    activate_window: [{ result: {} }],
    click: [{ result: {} }],
  });
  try {
    await readTree(harness);
    await readTree(harness);
    await readTree(harness, "6");
    const picked = await click(harness, { element_index: 1, tree_id: "t1-aaaaa" });
    assert.notEqual(picked.isError, true, textContent(picked));
    const [sent] = await logged(harness, "click");
    assert.deepEqual(sent?.params?.element, { tree_id: "t1-aaaaa", index: 1 });

    const foreign = await click(harness, { element_index: 1, tree_id: "t3-aaaaa" });
    assert.equal(code(foreign), "invalid_params", textContent(foreign));
    assert.match(textContent(foreign), /t3-aaaaa/);
    assert.match(textContent(foreign), /window_id 6/);
    assert.equal((await logged(harness, "click")).length, 1);
  } finally {
    await harness.close();
  }
});

test("an element_index past the tree is invalid_params naming the tree's range", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    const result = await click(harness, { element_index: 4 });
    assert.equal(code(result), "invalid_params", textContent(result));
    assert.match(textContent(result), /1 to 3/);
    assert.match(textContent(result), /t1-aaaaa/);
    assert.equal((await logged(harness, "click")).length, 0);
  } finally {
    await harness.close();
  }
});

test("a tree with no numbered elements says it has none", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }],
    get_ui_tree: [{ result: { tree_id: "t1-aaaaa", window_title: "Demo", elements: [{ depth: 0, role: "text", name: "Label", states: [], actions: [] }] } }],
  });
  try {
    await readTree(harness);
    const result = await click(harness, { element_index: 1 });
    assert.equal(code(result), "invalid_params", textContent(result));
    assert.match(textContent(result), /no numbered elements/);
  } finally {
    await harness.close();
  }
});

test("a tree dropped for newer ones is no_ui_tree", async () => {
  const windows = Array.from({ length: 6 }, () => ({ result: demo }));
  const harness = await serverWith({
    get_window: windows,
    get_ui_tree: Array.from({ length: 6 }, (_, number) => treeResult(`t${number + 1}-aaaaa`)),
  });
  try {
    for (let read = 0; read < 6; read += 1) await readTree(harness);
    const result = await click(harness, { element_index: 1, tree_id: "t1-aaaaa" });
    assert.equal(code(result), "no_ui_tree", textContent(result));
    assert.match(textContent(result), /t1-aaaaa/);
    assert.match(textContent(result), /computer_get_ui_tree/);
    assert.equal((await logged(harness, "click")).length, 0);
  } finally {
    await harness.close();
  }
});

test("after the helper restarts, the trees the old one read are no_ui_tree", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { exit: true, stderr: "helper went away" }],
    get_ui_tree: [treeResult("t1-aaaaa")],
  });
  try {
    await readTree(harness);
    const crashed = await click(harness, { element_index: 1 });
    assert.equal(code(crashed), "helper_crashed", textContent(crashed));
    const after = await click(harness, { element_index: 1 });
    assert.equal(code(after), "no_ui_tree", textContent(after));
    assert.equal((await logged(harness, "click")).length, 0);
  } finally {
    await harness.close();
  }
});

/** The text of a click by element in foreground delivery (ticket 140) whose helper `click` answers `answer`. */
async function clickAnswered(answer: FakeHandler): Promise<string> {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    click: [answer],
  });
  try {
    await readTree(harness);
    return textContent(await click(harness, { element_index: 1, delivery_mode: "foreground" }));
  } finally {
    await harness.close();
  }
}

test("a tree the helper no longer has is no_ui_tree with the server's advice", async () => {
  const text = await clickAnswered({ error: { code: "no_ui_tree", message: "unknown tree" } });
  assert.equal(text.split("\n")[0], "no_ui_tree");
  assert.match(text, /t1-aaaaa/);
  assert.match(text, /computer_get_ui_tree/);
});

test("the helper's element_not_found, input_failed and offscreen reach the model as they are", async () => {
  const gone = "The element no longer exists or no longer belongs to the window, so no input was sent.";
  assert.equal(await clickAnswered({ error: { code: "element_not_found", message: gone } }), `element_not_found\n${gone}`);
  const hidden = "The element is not in the visible part of the window; use computer_element_action with scroll_into_view first, then read a new tree or click it again.";
  assert.equal(await clickAnswered({ error: { code: "input_failed", message: hidden } }), `input_failed\n${hidden}`);
  const off = "The element is outside every screen, so no input was sent.";
  assert.equal(await clickAnswered({ error: { code: "offscreen", message: off } }), `offscreen\n${off}`);
});

test("a click by element reports an unread release and a foreground change like a click at a point", async () => {
  const text = await clickAnswered({
    result: { read_confirmed: false, foreground_changed: { foreground: { window_id: "77", title: "Saved", app: demo.app, app_name: "Demo" }, target_closed: false } },
  });
  assert.match(text, /^Clicked left 1 time\(s\) on element \[1\] of tree t1-aaaaa\./);
  assert.match(text, /had not read it after 0\.3 seconds/);
  assert.match(text, /the foreground moved to another window of Demo \(window_id: 77/);
});

test("a window that now belongs to an app the user declines gets no click by element", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: (message) => (message.includes("other.exe") ? { action: "decline" } : { action: "accept", content: { decision: "allow" } }),
    script: { handlers: { get_window: [{ result: demo }, { result: { ...demo, app: "C:\\Apps\\other.exe", app_name: "Other" } }], get_ui_tree: [treeResult("t1-aaaaa")] } },
  });
  try {
    await readTree(harness);
    const result = await click(harness, { element_index: 1 });
    assert.equal(code(result), "approval_declined", textContent(result));
    assert.equal((await logged(harness, "click")).length, 0);
  } finally {
    await harness.close();
  }
});

// Ticket 140: delivery_mode on a click by element (seam A). Background is the default: the window
// is not activated, the helper is told the mode and does the click through a pattern; a click that
// needs the pointer is background_unavailable (needs_pointer) before any overlay or input.

const RETRY = 'Retry this action with delivery_mode: "foreground"';

type ToolResult = { content?: unknown; isError?: boolean; structuredContent?: unknown };

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

/** The helper requests after the tree was read (reading it shows the overlay too). */
async function methods(harness: Harness): Promise<string[]> {
  const all = ((await harness.requests()) as Logged[]).map((entry) => entry.method ?? "");
  return all.slice(all.lastIndexOf("get_ui_tree") + 1);
}

/** One click by element on a read tree; `window` is what get_window answers when the click looks the window up. */
async function clickOnce(extra: Record<string, unknown>, answer: FakeHandler = { result: { action: "invoke" } }, window: Record<string, unknown> = demo) {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: window }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    click: [answer],
  });
  try {
    await readTree(harness);
    const result = (await click(harness, { element_index: 2, ...extra })) as ToolResult;
    await harness.finish();
    return { result, text: textContent(result), sent: await logged(harness, "click"), order: await methods(harness), calls: await toolCalls(harness) };
  } finally {
    await harness.close();
  }
}

test("ticket 140: delivery_mode is offered as background or foreground and is optional", async () => {
  const harness = await connectServer();
  try {
    const tool = (await harness.client.listTools()).tools.find((entry) => entry.name === "computer_click");
    const schema = tool?.inputSchema as { properties?: Record<string, { type?: string; enum?: string[] }>; required?: string[] };
    assert.equal(schema.properties?.delivery_mode?.type, "string");
    assert.deepEqual(schema.properties?.delivery_mode?.enum, ["background", "foreground"]);
    assert.ok(!schema.required?.includes("delivery_mode"));
  } finally {
    await harness.close();
  }
});

test("ticket 140: by default a click by element is delivered in the background: the helper is told so and the window is not activated", async () => {
  const { result, text, sent, order } = await clickOnce({});
  assert.notEqual(result.isError, true, text);
  assert.deepEqual(sent[0]?.params, {
    window_id: "5",
    expected_app: demo.app,
    element: { tree_id: "t1-aaaaa", index: 2 },
    button: "left",
    click_count: 1,
    modifiers: [],
    delivery_mode: "background",
  });
  assert.ok(!order.includes("activate_window"), order.join(", "));
  assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("click"), order.join(", "));
  const explicit = await clickOnce({ delivery_mode: "background", button: "left", click_count: 1, modifiers: [] });
  assert.equal(explicit.sent[0]?.params?.delivery_mode, "background");
  assert.ok(!explicit.order.includes("activate_window"), explicit.order.join(", "));
});

test("ticket 140: the result names the pattern the click used and what it read back", async () => {
  const cases: [Record<string, unknown>, RegExp][] = [
    [{ action: "invoke" }, /^Background click \(invoke\): Invoked element \[2\] of tree t1-aaaaa\. Take a screenshot/],
    [{ action: "toggle", state: "on", previous_state: "off" }, /^Background click \(toggle\): Toggled element \[2\] of tree t1-aaaaa; it now reads on\.$/],
    [{ action: "select", state: "selected" }, /^Background click \(select\): Selected element \[2\] of tree t1-aaaaa; it now reads selected\.$/],
    [{ action: "expand", state: "expanded" }, /^Background click \(expand\): Expanded element \[2\] of tree t1-aaaaa; it now reads expanded\.$/],
    [{ action: "collapse", state: "collapsed" }, /^Background click \(collapse\): Collapsed element \[2\] of tree t1-aaaaa; it now reads collapsed\.$/],
    [{ action: "toggle", state: "off", previous_state: "off" }, /still reads "off"/],
  ];
  for (const [answer, expected] of cases) {
    const { result, text } = await clickOnce({}, { result: answer });
    assert.notEqual(result.isError, true, text);
    assert.match(text, expected);
    assert.equal(result.structuredContent, undefined);
  }
});

test("ticket 140: a right click, a double click and held modifiers are background_unavailable (needs_pointer): no overlay, activation or input", async () => {
  for (const extra of [{ button: "right" }, { button: "middle" }, { click_count: 2 }, { click_count: 3 }, { modifiers: ["Shift"] }, { button: "right", click_count: 2, modifiers: ["ctrl"] }]) {
    const { result, text, order, sent, calls } = await clickOnce(extra);
    assert.equal(result.isError, true);
    assert.equal(code(result), "background_unavailable", `${JSON.stringify(extra)}: ${text}`);
    assert.ok(text.includes(RETRY), text);
    assert.deepEqual(result.structuredContent, { reason: "needs_pointer", recommended: "foreground" });
    assert.equal(sent.length, 0);
    for (const method of ["overlay_show", "activate_window", "click"]) assert.ok(!order.includes(method), `${method}: ${order.join(", ")}`);
    assert.equal(calls[0]?.result, "background_unavailable");
    assert.equal(calls[0]?.reason, "needs_pointer");
    assert.equal(calls[0]?.delivery, "background");
  }
});

test("ticket 140: in foreground delivery a right click, a double click and modifiers go ahead with the real mouse as today", async () => {
  const { result, text, sent, order } = await clickOnce({ button: "right", click_count: 2, modifiers: ["Shift"], delivery_mode: "foreground" }, { result: {} });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /^Clicked right 2 time\(s\) on element \[2\] of tree t1-aaaaa\./);
  assert.equal(sent[0]?.params?.delivery_mode, "foreground");
  assert.ok(order.lastIndexOf("activate_window") < order.lastIndexOf("click"), order.join(", "));
});

test("ticket 140: the helper's no_click_pattern reaches the model with its message and its reason", async () => {
  const message = `The element supports none of Invoke, Toggle, SelectionItem or ExpandCollapse, so a click on it cannot be delivered in the background; nothing was sent. ${RETRY}.`;
  const { result, text, calls } = await clickOnce({}, { error: { code: "background_unavailable", message, data: { reason: "no_click_pattern", recommended: "foreground", input_sent: false } } });
  assert.equal(text, `background_unavailable\n${message}`);
  assert.deepEqual(result.structuredContent, { reason: "no_click_pattern", recommended: "foreground" });
  assert.equal(calls[0]?.reason, "no_click_pattern");
});

test("ticket 140: a minimized window in background delivery is background_unavailable (minimized): no overlay, activation or input", async () => {
  const { result, text, order, sent } = await clickOnce({}, undefined, { ...demo, minimized: true });
  assert.equal(code(result), "background_unavailable", text);
  assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
  assert.equal(sent.length, 0);
  for (const method of ["overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
  const foreground = await clickOnce({ delivery_mode: "foreground" }, { result: {} }, { ...demo, minimized: true });
  assert.notEqual(foreground.result.isError, true, foreground.text);
});

test("ticket 140: an unknown delivery_mode is invalid_params before approval, overlay or input, without quoting it", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    for (const mode of ["auto", "Background", "", 1, null, true]) {
      const result = await click(harness, { element_index: 1, delivery_mode: mode });
      assert.equal(code(result), "invalid_params", `${JSON.stringify(mode)}: ${textContent(result)}`);
      assert.match(textContent(result), /delivery_mode/);
      assert.ok(!textContent(result).includes("auto"), textContent(result));
    }
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    const order = await methods(harness);
    for (const method of ["click", "overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("ticket 140: a foreground the helper gave back is foreground_restored in the result, its structuredContent and the log", async () => {
  const { result, text, calls } = await clickOnce({}, { result: { action: "toggle", state: "on", previous_state: "off", foreground_restored: true } });
  assert.notEqual(result.isError, true);
  assert.match(text, /^Background click \(toggle\): Toggled element \[2\] of tree t1-aaaaa; it now reads on\./);
  assert.match(text, /took the foreground during the action; it was given back to the window that had it/);
  assert.deepEqual(result.structuredContent, { foreground_restored: true });
  assert.equal(calls[0]?.foreground_restored, true);
  assert.equal(calls[0]?.delivery, "background");
});

test("ticket 140: a restore that failed says the click completed and must not be repeated, in the result, structuredContent and log", async () => {
  const answer = { result: { action: "invoke", foreground_changed: { foreground: { window_id: "42", title: "Demo", app: demo.app, app_name: "Demo" }, target_closed: false }, restore_failed: true } };
  const { result, text, calls } = await clickOnce({}, answer);
  assert.notEqual(result.isError, true);
  assert.match(text, /The click completed, but the target app took the foreground from the user's window and it could not be given back \(the foreground is now window_id 42\)\. Do not repeat the action\./);
  assert.deepEqual(result.structuredContent, { restore_failed: true });
  assert.equal(calls[0]?.restore_failed, true);
});

test("ticket 140: an app taking the foreground after a background click is reported as for other input", async () => {
  const { text } = await clickOnce({}, { result: { action: "invoke", foreground_changed: { foreground: { window_id: "77", title: "Saved", app: demo.app, app_name: "Demo" }, target_closed: false } } });
  assert.match(text, /After the click, the foreground moved to another window of Demo \(window_id: 77/);
});

test("ticket 140: how long a background click waited for the user is in the result, its structuredContent and the log", async () => {
  const { result, text, calls } = await clickOnce({}, { result: { action: "invoke", waited_for_idle_ms: 1200 } });
  assert.match(text, /waited 1200 ms \(waited_for_idle_ms\) for the user to stop typing and release the mouse/);
  assert.deepEqual(result.structuredContent, { waited_for_idle_ms: 1200 });
  assert.equal(calls[0]?.waited_for_idle_ms, 1200);
  const foreground = await clickOnce({ delivery_mode: "foreground" }, { result: {} });
  assert.equal(foreground.result.structuredContent, undefined);
  assert.equal("waited_for_idle_ms" in (foreground.calls[0] ?? {}), false);
});

test("ticket 140: each click's log event says its delivery", async () => {
  assert.equal((await clickOnce({})).calls[0]?.delivery, "background");
  assert.equal((await clickOnce({ delivery_mode: "foreground" }, { result: {} })).calls[0]?.delivery, "foreground");
});
