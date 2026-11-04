import assert from "node:assert/strict";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, repoRoot, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 65: computer_element_action (seam A, the server with a scripted helper).

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

const ACTIONS = ["invoke", "toggle", "expand", "collapse", "select", "scroll_into_view", "focus"];

const elements = [
  { depth: 0, index: 1, role: "check box", name: "Remember me", bounds: { x: 10, y: 10, width: 100, height: 20 }, states: ["off"], actions: ["click", "toggle", "focus"] },
  { depth: 0, index: 2, role: "combo box", name: "Colour", bounds: { x: 10, y: 40, width: 100, height: 20 }, states: ["collapsed"], actions: ["click", "expand"] },
  { depth: 0, index: 3, role: "button", name: "OK", bounds: { x: 10, y: 70, width: 60, height: 20 }, states: [], actions: ["click", "invoke"] },
];

function treeResult(treeId: string, lines: unknown[] = elements) {
  return { result: { tree_id: treeId, window_title: "Demo window", elements: lines } };
}

type Logged = { method?: string; params?: Record<string, unknown> };

async function logged(harness: Harness, method: string): Promise<Logged[]> {
  return ((await harness.requests()) as Logged[]).filter((entry) => entry.method === method);
}

function readTree(harness: Harness) {
  return harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "5" } });
}

function act(harness: Harness, extra: Record<string, unknown>) {
  return harness.client.callTool({ name: "computer_element_action", arguments: { window_id: "5", ...extra } });
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

function serverWith(handlers: Record<string, FakeHandler[]>) {
  return connectServer({ elicitation: "form", script: { handlers } });
}

/** The text of `action` on element `index` whose helper `element_action` answers `answer`, and what was sent. */
async function actAnswered(action: string, answer: FakeHandler, index = 1, extra: Record<string, unknown> = {}): Promise<{ text: string; sent: Logged[]; elicitations: string[]; order: string[] }> {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    element_action: [answer],
  });
  try {
    await readTree(harness);
    const text = textContent(await act(harness, { element_index: index, action, ...extra }));
    const order = ((await harness.requests()) as Logged[]).map((entry) => entry.method ?? "");
    return { text, sent: await logged(harness, "element_action"), elicitations: [...harness.elicitations], order };
  } finally {
    await harness.close();
  }
}

test("computer_element_action requires window_id, element_index and action, lists the seven actions, and offers tree_id", async () => {
  const harness = await connectServer();
  try {
    const listed = await harness.client.listTools();
    const tool = listed.tools.find((entry) => entry.name === "computer_element_action");
    const schema = tool?.inputSchema as { properties?: Record<string, { type?: string; enum?: string[] }>; required?: string[] };
    assert.deepEqual(schema.required, ["window_id", "element_index", "action"]);
    assert.equal(schema.properties?.element_index?.type, "integer");
    assert.deepEqual(schema.properties?.action?.enum, ACTIONS);
    assert.equal(schema.properties?.tree_id?.type, "string");
    assert.equal(listed.tools.length, 19);
  } finally {
    await harness.close();
  }
});

test("an action outside the list, or a missing element_index, is invalid_params before approval or input", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    for (const extra of [{ element_index: 1, action: "explode" }, { element_index: 1, action: "click" }, { element_index: 1 }, { element_index: 1, action: 3 }, { action: "toggle" }, { element_index: 9, action: "toggle" }]) {
      const result = await act(harness, extra);
      assert.equal(code(result), "invalid_params", `${JSON.stringify(extra)}: ${textContent(result)}`);
    }
    const unknown = textContent(await act(harness, { element_index: 1, action: "explode" }));
    assert.match(unknown, /invoke, toggle, expand, collapse, select, scroll_into_view, focus/);
    assert.ok(!unknown.includes("explode"), "the refusal quotes the model's text back");
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    assert.equal((await logged(harness, "element_action")).length, 0);
    assert.equal((await logged(harness, "get_window")).length, 1, "no window was looked at for the refused calls");
  } finally {
    await harness.close();
  }
});

test("a window whose tree was never read is no_ui_tree, without asking", async () => {
  const harness = await serverWith({});
  try {
    const result = await act(harness, { element_index: 1, action: "toggle" });
    assert.equal(code(result), "no_ui_tree", textContent(result));
    assert.match(textContent(result), /computer_get_ui_tree/);
    assert.equal(harness.elicitations.length, 0);
  } finally {
    await harness.close();
  }
});

test("a tree of another window is invalid_params and a tree the helper no longer has is no_ui_tree", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    element_action: [{ error: { code: "no_ui_tree", message: "unknown tree" } }],
  });
  try {
    await readTree(harness);
    const other = await harness.client.callTool({ name: "computer_element_action", arguments: { window_id: "6", element_index: 1, action: "toggle", tree_id: "t1-aaaaa" } });
    assert.equal(code(other), "invalid_params", textContent(other));
    const gone = await act(harness, { element_index: 1, action: "toggle", tree_id: "t1-aaaaa" });
    assert.equal(code(gone), "no_ui_tree", textContent(gone));
    assert.match(textContent(gone), /t1-aaaaa/);
  } finally {
    await harness.close();
  }
});

test("in foreground delivery the action uses the app's approval, shows the overlay and activates before the helper acts, and sends the element top-level", async () => {
  const { text, sent, elicitations, order } = await actAnswered("toggle", { result: { state: "on", previous_state: "off" } }, 1, { delivery_mode: "foreground" });
  assert.equal(elicitations.length, 1, `only the tree read asked: ${elicitations.join(" | ")}`);
  assert.deepEqual(sent[0]?.params, { window_id: "5", expected_app: demo.app, tree_id: "t1-aaaaa", index: 1, action: "toggle", delivery_mode: "foreground" });
  assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("element_action"), order.join(", "));
  assert.ok(order.lastIndexOf("activate_window") < order.lastIndexOf("element_action"), order.join(", "));
  assert.equal(text, "Toggled element [1] of tree t1-aaaaa; it now reads on.");
});

test("in foreground delivery scroll_into_view and focus show the overlay and activate first too", async () => {
  for (const action of ["scroll_into_view", "focus"]) {
    const { order } = await actAnswered(action, { result: action === "focus" ? { state: "focused" } : {} }, 1, { delivery_mode: "foreground" });
    assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("element_action"), `${action}: ${order.join(", ")}`);
    assert.ok(order.lastIndexOf("activate_window") < order.lastIndexOf("element_action"), `${action}: ${order.join(", ")}`);
  }
});

test("each read-back state is written into the result", async () => {
  const cases: [string, Record<string, unknown>, string][] = [
    ["expand", { state: "expanded" }, "Expanded element [1] of tree t1-aaaaa; it now reads expanded."],
    ["collapse", { state: "collapsed" }, "Collapsed element [1] of tree t1-aaaaa; it now reads collapsed."],
    ["select", { state: "selected" }, "Selected element [1] of tree t1-aaaaa; it now reads selected."],
    ["focus", { state: "focused" }, "Focused element [1] of tree t1-aaaaa; it now reads focused."],
    ["toggle", { state: "indeterminate", previous_state: "on" }, "Toggled element [1] of tree t1-aaaaa; it now reads indeterminate."],
  ];
  for (const [action, result, expected] of cases) {
    const { text } = await actAnswered(action, { result }, 1, action === "focus" ? { delivery_mode: "foreground" } : {});
    assert.equal(text, expected);
  }
});

test("invoke and scroll_into_view have no state and ask for a screenshot", async () => {
  const invoked = await actAnswered("invoke", { result: {} }, 3);
  assert.match(invoked.text, /^Invoked element \[3\] of tree t1-aaaaa\./);
  assert.match(invoked.text, /Take a screenshot/);
  const scrolled = await actAnswered("scroll_into_view", { result: {} });
  assert.match(scrolled.text, /^Scrolled element \[1\] of tree t1-aaaaa into view\./);
  assert.match(scrolled.text, /Take a screenshot/);
});

test("a read-back other than expected still succeeds, says what was read and asks for a screenshot", async () => {
  const expand = await actAnswered("expand", { result: { state: "collapsed" } });
  assert.notEqual(expand.text.split("\n")[0], "input_failed");
  assert.match(expand.text, /^Expanded element \[1\] of tree t1-aaaaa, but it now reads "collapsed", not "expanded"/);
  assert.match(expand.text, /Take a screenshot/);
  const focus = await actAnswered("focus", { result: { state: "not focused" } }, 1, { delivery_mode: "foreground" });
  assert.match(focus.text, /it now reads "not focused", not "focused"/);
  const select = await actAnswered("select", { result: { state: "not selected" } });
  assert.match(select.text, /it now reads "not selected", not "selected"/);
  // A toggle that did not change the state.
  const toggle = await actAnswered("toggle", { result: { state: "off", previous_state: "off" } });
  assert.match(toggle.text, /^Toggled element \[1\] of tree t1-aaaaa, but it still reads "off"/);
  assert.match(toggle.text, /Take a screenshot/);
});

test("a state that is missing or not one the server knows is reported as not read back, without the helper's text", async () => {
  for (const result of [{}, { state: "wobbly <b>" }, { state: 7 }, { state: "on" }]) {
    const { text } = await actAnswered("expand", { result });
    assert.match(text, /^Expanded element \[1\] of tree t1-aaaaa, but its state could not be read back\. Take a screenshot/, `${JSON.stringify(result)}: ${text}`);
    assert.ok(!text.includes("wobbly"), text);
  }
});

test("not_supported lists the element's supported actions from the helper's data", async () => {
  const { text } = await actAnswered("expand", {
    error: { code: "not_supported", message: "This element does not support expand; it supports toggle, focus.", data: { supported_actions: ["toggle", "focus"] } },
  });
  assert.equal(text.split("\n")[0], "not_supported");
  assert.match(text, /element \[1\] of tree t1-aaaaa does not support expand/);
  assert.match(text, /It supports: toggle, focus\./);

  const none = await actAnswered("toggle", { error: { code: "not_supported", message: "x", data: { supported_actions: [] } } }, 3);
  assert.match(none.text, /does not support toggle/);
  assert.match(none.text, /none of computer_element_action's actions/);
});

test("supported actions the server does not know are internal, not passed to the model", async () => {
  const { text } = await actAnswered("expand", { error: { code: "not_supported", message: "x", data: { supported_actions: ["toggle", "launch_missiles"] } } });
  assert.equal(text.split("\n")[0], "internal");
  assert.ok(!text.includes("launch_missiles"), text);
});

test("a disabled element, a timeout and element_not_found reach the model as the helper words them", async () => {
  const cases = [
    { code: "not_supported", message: "This element is disabled, so toggle was not done." },
    { code: "input_failed", message: "The app did not respond to the UI Automation request within 3 seconds; it may still perform the action later. Take a screenshot before retrying." },
    { code: "input_failed", message: "The app took the action, but did not answer the UI Automation request for the element's new state within 3 seconds. Do not repeat the action: take a screenshot to check its result." },
    { code: "element_not_found", message: "The element no longer exists." },
    { code: "app_changed", message: "The window now belongs to C:\\Apps\\other.exe, not the approved app." },
  ];
  for (const error of cases) {
    const { text } = await actAnswered("toggle", { error });
    assert.equal(text, `${error.code}\n${error.message}`);
  }
});

test("a foreground change after the action is reported as for other input", async () => {
  const { text } = await actAnswered("invoke", {
    result: { foreground_changed: { foreground: { window_id: "77", title: "Saved", app: demo.app, app_name: "Demo" }, target_closed: false } },
  }, 3);
  assert.match(text, /^Invoked element \[3\] of tree t1-aaaaa\./);
  assert.match(text, /After the invoke action, the foreground moved to another window of Demo \(window_id: 77/);
});

test("in foreground delivery Esc right after the window is activated stops the call before the helper acts", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {}, eventsAfter: [{ event: "stopped" }] }],
    element_action: [{ result: { state: "on" } }],
  });
  try {
    await readTree(harness);
    const result = await act(harness, { element_index: 1, action: "toggle", delivery_mode: "foreground" });
    assert.equal(code(result), "interrupted", textContent(result));
    assert.equal((await logged(harness, "element_action")).length, 0);
  } finally {
    await harness.close();
  }
});

// Ticket 70: a stop between the action and its read-back is answered with the helper's own text
// (then the `stopped` event): the model is told the action was taken and not to repeat it.
const STOPPED_AFTER_ACTION =
  "The user pressed Esc and stopped Deskpass after the action was taken, so its result was not read back. Do not repeat the action; once the user lets you continue, take a screenshot to check its result. Do not call these tools again now: end this turn and tell the user.";

test("a stop after the action tells the model it was taken and not to repeat it", async () => {
  const { text, sent } = await actAnswered("toggle", { error: { code: "interrupted", message: STOPPED_AFTER_ACTION }, eventsAfter: [{ event: "stopped", data: { generation: 1 } }] });
  assert.equal(text, `interrupted\n${STOPPED_AFTER_ACTION}`);
  assert.equal(sent.length, 1);
});

test("a window that now belongs to an app the user declines is not acted on", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: (message) => (message.includes("other.exe") ? { action: "decline" } : { action: "accept", content: { decision: "allow" } }),
    script: { handlers: { get_window: [{ result: demo }, { result: { ...demo, app: "C:\\Apps\\other.exe", app_name: "Other" } }], get_ui_tree: [treeResult("t1-aaaaa")] } },
  });
  try {
    await readTree(harness);
    const result = await act(harness, { element_index: 1, action: "toggle" });
    assert.equal(code(result), "approval_declined", textContent(result));
    assert.equal((await logged(harness, "element_action")).length, 0);
  } finally {
    await harness.close();
  }
});

test("a tree whose element lists an action the server does not know is internal, not shown", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa", [{ ...elements[0], actions: ["click", "format_disk"] }])],
  });
  try {
    const result = await readTree(harness);
    assert.equal(code(result), "internal", textContent(result));
    assert.ok(!textContent(result).includes("format_disk"), textContent(result));
  } finally {
    await harness.close();
  }
});

// Ticket 138: delivery_mode (seam A). Background is the default: the target is not activated, the
// helper is told the mode, and a refusal is background_unavailable with { reason, recommended }.

const RETRY = 'Retry this action with delivery_mode: "foreground"';

type ToolResult = { content?: unknown; isError?: boolean; structuredContent?: unknown };

/** The day's `tool_call` events of computer_element_action in `harness`'s log. */
async function toolCalls(harness: Harness): Promise<Record<string, unknown>[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call" && event.tool === "computer_element_action");
}

/** The helper requests after the tree was read (reading it shows the overlay too). */
async function methods(harness: Harness): Promise<string[]> {
  const all = ((await harness.requests()) as Logged[]).map((entry) => entry.method ?? "");
  return all.slice(all.lastIndexOf("get_ui_tree") + 1);
}

/** One element action on a read tree; `window` is what get_window answers when the action looks the window up. */
async function actOnce(extra: Record<string, unknown>, answer: FakeHandler = { result: { state: "on", previous_state: "off" } }, window: Record<string, unknown> = demo) {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: window }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    element_action: [answer],
  });
  try {
    await readTree(harness);
    const result = (await act(harness, { element_index: 1, ...extra })) as ToolResult;
    await harness.finish();
    return { result, text: textContent(result), sent: await logged(harness, "element_action"), order: await methods(harness), calls: await toolCalls(harness) };
  } finally {
    await harness.close();
  }
}

test("ticket 138: delivery_mode is offered as background or foreground and is optional", async () => {
  const harness = await connectServer();
  try {
    const tool = (await harness.client.listTools()).tools.find((entry) => entry.name === "computer_element_action");
    const schema = tool?.inputSchema as { properties?: Record<string, { type?: string; enum?: string[] }>; required?: string[] };
    assert.equal(schema.properties?.delivery_mode?.type, "string");
    assert.deepEqual(schema.properties?.delivery_mode?.enum, ["background", "foreground"]);
    assert.ok(!schema.required?.includes("delivery_mode"));
  } finally {
    await harness.close();
  }
});

test("ticket 138: by default the action is delivered in the background: the helper is told so and the window is not activated", async () => {
  const { text, sent, order, result } = await actOnce({ action: "toggle" });
  assert.notEqual(result.isError, true, text);
  assert.equal(text, "Toggled element [1] of tree t1-aaaaa; it now reads on.");
  assert.deepEqual(sent[0]?.params, { window_id: "5", expected_app: demo.app, tree_id: "t1-aaaaa", index: 1, action: "toggle", delivery_mode: "background" });
  assert.ok(!order.includes("activate_window"), order.join(", "));
  assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("element_action"), order.join(", "));
});

test("ticket 138: an explicit background is the same as the default", async () => {
  const { sent, order } = await actOnce({ action: "invoke", delivery_mode: "background" });
  assert.equal(sent[0]?.params?.delivery_mode, "background");
  assert.ok(!order.includes("activate_window"), order.join(", "));
});

test("ticket 138: an unknown delivery_mode is invalid_params before approval, overlay or input, without quoting it", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    for (const mode of ["auto", "Background", "", 1, null, true]) {
      const result = await act(harness, { element_index: 1, action: "toggle", delivery_mode: mode });
      assert.equal(code(result), "invalid_params", `${JSON.stringify(mode)}: ${textContent(result)}`);
      assert.match(textContent(result), /delivery_mode/);
      assert.ok(!textContent(result).includes("auto"), textContent(result));
    }
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    const order = await methods(harness);
    for (const method of ["element_action", "overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("ticket 138: focus in background delivery is background_unavailable (focus_needs_foreground): no overlay, activation or input", async () => {
  const { result, text, order, sent } = await actOnce({ action: "focus" });
  assert.equal(result.isError, true);
  assert.equal(code(result), "background_unavailable", text);
  assert.ok(text.includes(RETRY), text);
  assert.deepEqual(result.structuredContent, { reason: "focus_needs_foreground", recommended: "foreground" });
  assert.equal(sent.length, 0);
  for (const method of ["overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
});

test("ticket 138: a minimized window in background delivery is background_unavailable (minimized): no overlay, activation or input", async () => {
  const { result, text, order, sent } = await actOnce({ action: "toggle" }, undefined, { ...demo, minimized: true });
  assert.equal(code(result), "background_unavailable", text);
  assert.ok(text.includes(RETRY), text);
  assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
  assert.equal(sent.length, 0);
  for (const method of ["overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
});

test("ticket 138: in foreground delivery a minimized window and focus go ahead as today", async () => {
  const minimized = await actOnce({ action: "toggle", delivery_mode: "foreground" }, undefined, { ...demo, minimized: true });
  assert.notEqual(minimized.result.isError, true, minimized.text);
  assert.ok(minimized.order.includes("activate_window"));
  const focus = await actOnce({ action: "focus", delivery_mode: "foreground" }, { result: { state: "focused" } });
  assert.equal(focus.text, "Focused element [1] of tree t1-aaaaa; it now reads focused.");
});

test("ticket 138: the helper's background_unavailable reaches the model with its message and its reason", async () => {
  const message = `The target window is minimized, so the action cannot be delivered in the background and nothing was sent. ${RETRY}.`;
  const { result, text } = await actOnce({ action: "toggle" }, { error: { code: "background_unavailable", message, data: { reason: "minimized", recommended: "foreground", input_sent: false } } });
  assert.equal(text, `background_unavailable\n${message}`);
  assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
});

test("ticket 138: dialog_open in background delivery reaches the model as today", async () => {
  const { text } = await actOnce({ action: "toggle" }, { error: { code: "dialog_open", message: "x", data: { window_id: "99", title: "Save As" } } });
  assert.match(text, /^dialog_open\nThe target window has an open dialog \(window_id: 99, title: "Save As"\)/);
});

test("ticket 138: an app taking the foreground after a background action is reported as for other input", async () => {
  const { text } = await actOnce({ action: "invoke" }, { result: { foreground_changed: { foreground: { window_id: "77", title: "Saved", app: demo.app, app_name: "Demo" }, target_closed: false } } });
  assert.match(text, /After the invoke action, the foreground moved to another window of Demo \(window_id: 77/);
});

test("ticket 138: a foreground the helper gave back is foreground_restored in the result, its structuredContent and the log", async () => {
  const { result, text, calls } = await actOnce({ action: "toggle" }, { result: { state: "on", previous_state: "off", foreground_restored: true } });
  assert.notEqual(result.isError, true);
  assert.match(text, /^Toggled element \[1\] of tree t1-aaaaa; it now reads on\./);
  assert.match(text, /took the foreground during the action; it was given back to the window that had it/);
  assert.deepEqual(result.structuredContent, { foreground_restored: true });
  assert.equal(calls[0]?.foreground_restored, true);
  assert.equal("restore_failed" in (calls[0] ?? {}), false);
});

test("ticket 138: a restore that failed tells the model the user's window was taken and the action completed, in the result, structuredContent and log", async () => {
  const answer = { result: { state: "on", previous_state: "off", foreground_changed: { foreground: { window_id: "42", title: "Demo", app: demo.app, app_name: "Demo" }, target_closed: false }, restore_failed: true } };
  const { result, text, calls } = await actOnce({ action: "toggle" }, answer);
  assert.notEqual(result.isError, true);
  assert.match(text, /^Toggled element \[1\] of tree t1-aaaaa; it now reads on\./);
  assert.match(text, /The toggle action completed, but the target app took the foreground from the user's window and it could not be given back \(the foreground is now window_id 42\)\. Do not repeat the action\./);
  assert.doesNotMatch(text, /the foreground moved to/);
  assert.deepEqual(result.structuredContent, { restore_failed: true });
  assert.equal(calls[0]?.restore_failed, true);
  assert.equal("foreground_restored" in (calls[0] ?? {}), false);
});

test("ticket 138: an action whose foreground nobody took has neither field", async () => {
  const { result, calls } = await actOnce({ action: "toggle" });
  assert.equal(result.structuredContent, undefined);
  assert.equal("foreground_restored" in (calls[0] ?? {}), false);
  assert.equal("restore_failed" in (calls[0] ?? {}), false);
});

test("ticket 138: each action's log event says its delivery; a refused background action logs background_unavailable with its reason", async () => {
  const background = await actOnce({ action: "toggle" });
  assert.equal(background.calls.length, 1);
  assert.equal(background.calls[0]?.delivery, "background");
  assert.equal(background.calls[0]?.result, "ok");
  const foreground = await actOnce({ action: "toggle", delivery_mode: "foreground" });
  assert.equal(foreground.calls[0]?.delivery, "foreground");
  const refused = await actOnce({ action: "focus" });
  assert.equal(refused.calls[0]?.result, "background_unavailable");
  assert.equal(refused.calls[0]?.reason, "focus_needs_foreground");
  assert.equal(refused.calls[0]?.delivery, "background");
  assert.deepEqual(refused.calls[0]?.app, { path: demo.app, key: demo.app.toLowerCase(), tier: "normal" });
  const fromHelper = await actOnce({ action: "toggle" }, { error: { code: "background_unavailable", message: "m", data: { reason: "minimized", recommended: "foreground" } } });
  assert.equal(fromHelper.calls[0]?.result, "background_unavailable");
  assert.equal(fromHelper.calls[0]?.reason, "minimized");
  const invalid = await actOnce({ action: "toggle", delivery_mode: "auto" });
  assert.equal(invalid.calls[0]?.result, "invalid_params");
  assert.equal("reason" in (invalid.calls[0] ?? {}), false);
});

test("ticket 138: background delivery keeps the elevated, blocked-tier and approval refusals before any overlay or input", async () => {
  const cases: [Record<string, unknown>, string][] = [
    [{ ...demo, elevated: true, helper_elevated: false }, "elevated_target"],
    [{ ...demo, app: "C:\\Windows\\System32\\consent.exe", app_name: "Consent" }, "blocked_app"],
  ];
  for (const [window, expected] of cases) {
    const { result, text, order } = await actOnce({ action: "toggle", delivery_mode: "background" }, undefined, window);
    assert.equal(code(result), expected, text);
    for (const method of ["overlay_show", "activate_window", "element_action"]) assert.ok(!order.includes(method), `${expected}: ${order.join(", ")}`);
  }
  const harness = await connectServer({
    elicitation: "form",
    onElicit: (message) => (message.includes("other.exe") ? { action: "decline" } : { action: "accept", content: { decision: "allow" } }),
    script: { handlers: { get_window: [{ result: demo }, { result: { ...demo, app: "C:\\Apps\\other.exe", app_name: "Other" } }], get_ui_tree: [treeResult("t1-aaaaa")] } },
  });
  try {
    await readTree(harness);
    const result = await act(harness, { element_index: 1, action: "toggle", delivery_mode: "background" });
    assert.equal(code(result), "approval_declined", textContent(result));
    const order = await methods(harness);
    for (const method of ["overlay_show", "element_action"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("ticket 138: background delivery sends the expected_app check to the helper and its app_changed reaches the model", async () => {
  const { text, sent } = await actOnce({ action: "toggle" }, { error: { code: "app_changed", message: "The window now belongs to C:\\Apps\\other.exe, not the approved app." } });
  assert.equal(sent[0]?.params?.expected_app, demo.app);
  assert.equal(text, "app_changed\nThe window now belongs to C:\\Apps\\other.exe, not the approved app.");
});

test("ticket 138: while another session holds the operator lock, a background element action is in_use and nothing reaches the target", async () => {
  const lockDir = join(repoRoot(), "test-tmp", `lock-${Date.now()}-element-action-background`);
  await mkdir(lockDir, { recursive: true });
  const other = { pid: 70002, startedAt: "2026-09-24T02:00:00.000Z" };
  const alive = { result: { alive: true, started_ms: Date.parse(other.startedAt) } };
  // The tree is read under this session's lock, which it releases when idle; then another session takes it.
  const harness = await connectServer({
    elicitation: "form",
    lockDir,
    owner: { pid: 70001, startedAt: "2026-09-24T01:00:00.000Z" },
    lockIdleMs: 150,
    overlayIdleMs: 60_000,
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], get_ui_tree: [treeResult("t1-aaaaa")], process_info: [alive, alive, alive] } },
  });
  try {
    assert.notEqual((await readTree(harness)).isError, true);
    const until = Date.now() + 5_000;
    while (await readFile(join(lockDir, "session.lock"), "utf8").then(() => true, () => false)) {
      if (Date.now() > until) assert.fail("the lock was not released when idle");
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    await writeFile(join(lockDir, "session.lock"), JSON.stringify(other));
    const result = await act(harness, { element_index: 1, action: "toggle", delivery_mode: "background" });
    assert.equal(code(result), "in_use", textContent(result));
    const order = await methods(harness);
    for (const method of ["overlay_show", "activate_window", "element_action"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
    await rm(lockDir, { recursive: true, force: true });
  }
});

// Ticket 149: the helper waits for the user to stop typing and release the mouse before a
// background action, and says how long (`waited_for_idle_ms`, `idle_wait_capped` at the 3 s cap).

test("ticket 149: how long a background action waited for the user is in the result, its structuredContent and the log", async () => {
  const { result, text, calls } = await actOnce({ action: "toggle" }, { result: { state: "on", previous_state: "off", waited_for_idle_ms: 1200 } });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /^Toggled element \[1\] of tree t1-aaaaa; it now reads on\. /);
  assert.match(text, /waited 1200 ms \(waited_for_idle_ms\) for the user to stop typing and release the mouse/);
  assert.doesNotMatch(text, /went ahead anyway/);
  assert.deepEqual(result.structuredContent, { waited_for_idle_ms: 1200 });
  assert.equal(calls[0]?.waited_for_idle_ms, 1200);
  assert.equal("idle_wait_capped" in (calls[0] ?? {}), false);
});

test("ticket 149: a background action that did not wait carries waited_for_idle_ms 0 and its text is unchanged", async () => {
  const { result, text, calls } = await actOnce({ action: "toggle" }, { result: { state: "on", previous_state: "off", waited_for_idle_ms: 0 } });
  assert.equal(text, "Toggled element [1] of tree t1-aaaaa; it now reads on.");
  assert.deepEqual(result.structuredContent, { waited_for_idle_ms: 0 });
  assert.equal(calls[0]?.waited_for_idle_ms, 0);
  assert.equal("idle_wait_capped" in (calls[0] ?? {}), false);
});

test("ticket 149: a wait cut off at the cap says the action went ahead anyway and logs idle_wait_capped, alongside the restore", async () => {
  const { result, text, calls } = await actOnce({ action: "toggle" }, { result: { state: "on", previous_state: "off", foreground_restored: true, waited_for_idle_ms: 3004, idle_wait_capped: true } });
  assert.match(text, /waited 3004 ms \(waited_for_idle_ms\) for the user to stop typing and release the mouse; they had not stopped after 3 seconds, so it went ahead anyway\./);
  assert.match(text, /it was given back to the window that had it/);
  assert.deepEqual(result.structuredContent, { foreground_restored: true, waited_for_idle_ms: 3004, idle_wait_capped: true });
  assert.equal(calls[0]?.waited_for_idle_ms, 3004);
  assert.equal(calls[0]?.idle_wait_capped, true);
  assert.equal(calls[0]?.foreground_restored, true);
});

test("ticket 149: a foreground action does not wait: its request, result and log are as before", async () => {
  const { result, text, sent, calls } = await actOnce({ action: "toggle", delivery_mode: "foreground" });
  assert.deepEqual(sent[0]?.params, { window_id: "5", expected_app: demo.app, tree_id: "t1-aaaaa", index: 1, action: "toggle", delivery_mode: "foreground" });
  assert.equal(text, "Toggled element [1] of tree t1-aaaaa; it now reads on.");
  assert.equal(result.structuredContent, undefined);
  assert.equal("waited_for_idle_ms" in (calls[0] ?? {}), false);
  assert.equal("idle_wait_capped" in (calls[0] ?? {}), false);
});
