import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 64: computer_set_value (seam A, the server with a scripted helper).

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

const elements = [
  { depth: 0, index: 1, role: "edit", name: "Search", value: "", bounds: { x: 10, y: 10, width: 100, height: 20 }, states: [], actions: ["click", "set_value"] },
  { depth: 0, index: 2, role: "edit", name: "Password", bounds: { x: 10, y: 40, width: 100, height: 20 }, states: ["password"], actions: ["click", "set_value"] },
  { depth: 0, index: 3, role: "button", name: "OK", bounds: { x: 10, y: 70, width: 60, height: 20 }, states: [], actions: ["click", "invoke"] },
];

function treeResult(treeId: string) {
  return { result: { tree_id: treeId, window_title: "Demo window", elements } };
}

type Logged = { method?: string; params?: Record<string, unknown> };

async function logged(harness: Harness, method: string): Promise<Logged[]> {
  return ((await harness.requests()) as Logged[]).filter((entry) => entry.method === method);
}

function readTree(harness: Harness) {
  return harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "5" } });
}

function setValue(harness: Harness, extra: Record<string, unknown>) {
  return harness.client.callTool({ name: "computer_set_value", arguments: { window_id: "5", ...extra } });
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

function serverWith(handlers: Record<string, FakeHandler[]>) {
  return connectServer({ elicitation: "form", script: { handlers } });
}

/** The value the tests send: it must never come back in any text. */
const SENT = "S3cret-值-7781";

/** The text of a set_value of SENT on element 1 whose helper `set_value` answers `answer`, and what was sent. */
async function setAnswered(answer: FakeHandler, index = 1, extra: Record<string, unknown> = {}): Promise<{ text: string; sent: Logged[]; elicitations: string[] }> {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    set_value: [answer],
  });
  try {
    await readTree(harness);
    const text = textContent(await setValue(harness, { element_index: index, value: SENT, ...extra }));
    return { text, sent: await logged(harness, "set_value"), elicitations: [...harness.elicitations] };
  } finally {
    await harness.close();
  }
}

test("computer_set_value requires window_id, element_index and value and offers tree_id", async () => {
  const harness = await connectServer();
  try {
    const listed = await harness.client.listTools();
    const tool = listed.tools.find((entry) => entry.name === "computer_set_value");
    const schema = tool?.inputSchema as { properties?: Record<string, { type?: string }>; required?: string[] };
    assert.deepEqual(schema.required, ["window_id", "element_index", "value"]);
    assert.equal(schema.properties?.element_index?.type, "integer");
    assert.equal(schema.properties?.value?.type, "string");
    assert.equal(schema.properties?.tree_id?.type, "string");
    assert.match(tool?.description ?? "", /computer_type_text/);
  } finally {
    await harness.close();
  }
});

test("the description suggests clicking and typing only for an element without a value, not for a read-only field", async () => {
  const harness = await connectServer();
  try {
    const listed = await harness.client.listTools();
    const description = listed.tools.find((entry) => entry.name === "computer_set_value")?.description ?? "";
    const sentences = description.split(/(?<=\.)\s+/);
    const suggesting = sentences.filter((sentence) => /computer_type_text/.test(sentence));
    assert.equal(suggesting.length, 1, description);
    assert.match(suggesting[0] ?? "", /without a value/);
    assert.doesNotMatch(suggesting[0] ?? "", /read-only/);
    assert.match(description, /read-only/);
  } finally {
    await harness.close();
  }
});

test("a value longer than 100 000 characters is invalid_params before approval or input; one of exactly 100 000 is sent", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    set_value: [{ result: { read_back: "x", read_back_length: 100_000, read_back_matches: true } }],
  });
  try {
    await readTree(harness);
    // Counted in characters, as the read-back is: 100 000 Chinese characters are 100 000, not 200 000 bytes.
    const tooLong = await setValue(harness, { element_index: 1, value: "值".repeat(100_001) });
    assert.equal(code(tooLong), "invalid_params", textContent(tooLong).slice(0, 300));
    assert.match(textContent(tooLong), /100000 characters/);
    assert.ok(textContent(tooLong).length < 1000, "the refusal quotes the value");
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    assert.equal((await logged(harness, "set_value")).length, 0);

    const longest = await setValue(harness, { element_index: 1, value: "值".repeat(100_000) });
    assert.notEqual(longest.isError, true, textContent(longest).slice(0, 300));
    assert.equal((await logged(harness, "set_value")).length, 1);
  } finally {
    await harness.close();
  }
});

test("a value that is not text, or a missing element_index, is invalid_params before approval or input", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    for (const extra of [{ element_index: 1 }, { element_index: 1, value: 5 }, { value: "x" }, { element_index: 0, value: "x" }, { element_index: 9, value: "x" }]) {
      const result = await setValue(harness, extra);
      assert.equal(code(result), "invalid_params", `${JSON.stringify(extra)}: ${textContent(result)}`);
    }
    assert.match(textContent(await setValue(harness, { element_index: 9, value: "x" })), /element_index 1 to 3/);
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    assert.equal((await logged(harness, "set_value")).length, 0);
    assert.equal((await logged(harness, "get_window")).length, 1, "no window was looked at for the refused calls");
  } finally {
    await harness.close();
  }
});

test("a window whose tree was never read is no_ui_tree, without asking", async () => {
  const harness = await serverWith({});
  try {
    const result = await setValue(harness, { element_index: 1, value: SENT });
    assert.equal(code(result), "no_ui_tree", textContent(result));
    assert.match(textContent(result), /computer_get_ui_tree/);
    assert.equal(harness.elicitations.length, 0);
    assert.equal((await logged(harness, "set_value")).length, 0);
  } finally {
    await harness.close();
  }
});

test("in foreground delivery set_value uses the app's approval without asking again and sends the element and value top-level after activation", async () => {
  const { text, sent, elicitations } = await setAnswered({ result: { read_back: SENT, read_back_matches: true } }, 1, { delivery_mode: "foreground" });
  assert.equal(elicitations.length, 1, `only the tree read asked: ${elicitations.join(" | ")}`);
  assert.deepEqual(sent[0]?.params, { window_id: "5", expected_app: demo.app, tree_id: "t1-aaaaa", index: 1, value: SENT, delivery_mode: "foreground" });
  assert.equal(text, "Set the value of element [1] of tree t1-aaaaa; it reads back as sent.");
});

test("in foreground delivery the overlay and activation come before the value is set", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    set_value: [{ result: { read_back: "x", read_back_matches: true } }],
  });
  try {
    await readTree(harness);
    const result = await setValue(harness, { element_index: 1, value: "x", tree_id: "t1-aaaaa", delivery_mode: "foreground" });
    assert.notEqual(result.isError, true, textContent(result));
    const order = ((await harness.requests()) as Logged[]).map((entry) => entry.method);
    assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("set_value"), order.join(", "));
    assert.ok(order.lastIndexOf("activate_window") < order.lastIndexOf("set_value"), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("a different read-back still succeeds, quoting the field escaped and cut, and asks for a screenshot", async () => {
  const { text } = await setAnswered({ result: { read_back: 'Line "one"\nline two', read_back_matches: false } });
  assert.notEqual(text.split("\n")[0], "input_failed");
  assert.match(text, /now reads "Line \\"one\\"\\nline two", not what was sent/);
  assert.match(text, /formatted or ignored/);
  assert.match(text, /Take a screenshot/);
  assert.ok(!text.includes(SENT), text);

  const long = "x".repeat(200);
  const cut = await setAnswered({ result: { read_back: long, read_back_length: 1000, read_back_matches: false } });
  assert.match(cut.text, new RegExp(`now reads "${long}…\\(1000 chars\\)"`));
});

test("a password field says it was set and cannot be read back, without the value", async () => {
  const { text } = await setAnswered({ result: { password: true } }, 2);
  assert.equal(text, "Set element [2] of tree t1-aaaaa: the value was set; password fields cannot be read back.");
  assert.ok(!text.includes(SENT), text);
});

test("a field that could not be read back says so and asks for a screenshot", async () => {
  const { text } = await setAnswered({ result: {} });
  assert.match(text, /could not be read back/);
  assert.match(text, /Take a screenshot/);
});

test("the helper's not_supported, read-only, timeout and element_not_found reach the model as they are, without the value", async () => {
  const cases = [
    { code: "not_supported", message: "This element does not accept a value directly. Click it and use computer_type_text instead." },
    { code: "not_supported", message: "This element is read-only, so its value cannot be set." },
    { code: "input_failed", message: "The app did not respond to the UI Automation request within 3 seconds; it may still perform the action later. Take a screenshot before retrying." },
    { code: "element_not_found", message: "The element no longer exists." },
  ];
  for (const error of cases) {
    const { text } = await setAnswered({ error });
    assert.equal(text, `${error.code}\n${error.message}`);
    assert.ok(!text.includes(SENT), text);
  }
});

test("ticket 92: a password field is password_field, asking the model to have the user type it, without the value", async () => {
  const { text } = await setAnswered({ error: { code: "password_field", message: "The element is a password field, so no input was sent.", data: { input_sent: false } } }, 2);
  assert.equal(
    text,
    "password_field\nThe keyboard focus or the element is a password field, so no input was sent: Deskpass never types, pastes or sets passwords. " +
      "Ask the user to type it themselves and continue once they say it is done. Navigation keys such as Tab still work, to move the focus out of the field.",
  );
  assert.ok(!text.includes(SENT), text);
});

test("a tree the helper no longer has is no_ui_tree with the server's advice", async () => {
  const { text } = await setAnswered({ error: { code: "no_ui_tree", message: "unknown tree" } });
  assert.equal(text.split("\n")[0], "no_ui_tree");
  assert.match(text, /t1-aaaaa/);
  assert.match(text, /computer_get_ui_tree/);
});

test("a foreground change after the value is set is reported as for other input", async () => {
  const { text } = await setAnswered({
    result: { read_back: SENT, read_back_matches: true, foreground_changed: { foreground: { window_id: "77", title: "Saved", app: demo.app, app_name: "Demo" }, target_closed: false } },
  });
  assert.match(text, /^Set the value of element \[1\] of tree t1-aaaaa; it reads back as sent\./);
  assert.match(text, /After the value change, the foreground moved to another window of Demo \(window_id: 77/);
});

test("in foreground delivery Esc right after the window is activated stops the call before the value is sent", async () => {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: demo }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {}, eventsAfter: [{ event: "stopped" }] }],
    set_value: [{ result: { read_back: SENT, read_back_matches: true } }],
  });
  try {
    await readTree(harness);
    const result = await setValue(harness, { element_index: 1, value: SENT, delivery_mode: "foreground" });
    assert.equal(code(result), "interrupted", textContent(result));
    assert.equal((await logged(harness, "set_value")).length, 0);
  } finally {
    await harness.close();
  }
});

// Ticket 70: the helper answers a stop after SetValue with its own text (then the `stopped`
// event); the model is told the value was set and not to set it again.
const STOPPED_AFTER_ACTION =
  "The user pressed Esc and stopped Deskpass after the action was taken, so its result was not read back. Do not repeat the action; once the user lets you continue, take a screenshot to check its result. Do not call these tools again now: end this turn and tell the user.";

test("a stop after the value was set tells the model it was set and not to repeat it", async () => {
  const { text } = await setAnswered({ error: { code: "interrupted", message: STOPPED_AFTER_ACTION }, eventsAfter: [{ event: "stopped", data: { generation: 1 } }] });
  assert.equal(text, `interrupted\n${STOPPED_AFTER_ACTION}`);
  assert.ok(!text.includes(SENT));
});

test("a window that now belongs to an app the user declines gets no value", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: (message) => (message.includes("other.exe") ? { action: "decline" } : { action: "accept", content: { decision: "allow" } }),
    script: { handlers: { get_window: [{ result: demo }, { result: { ...demo, app: "C:\\Apps\\other.exe", app_name: "Other" } }], get_ui_tree: [treeResult("t1-aaaaa")] } },
  });
  try {
    await readTree(harness);
    const result = await setValue(harness, { element_index: 1, value: SENT });
    assert.equal(code(result), "approval_declined", textContent(result));
    assert.ok(!textContent(result).includes(SENT));
    assert.equal((await logged(harness, "set_value")).length, 0);
  } finally {
    await harness.close();
  }
});

test("the helper's app_changed reaches the model without the value", async () => {
  const { text } = await setAnswered({ error: { code: "app_changed", message: "The window now belongs to C:\\Apps\\other.exe, not the approved app." } });
  assert.equal(text.split("\n")[0], "app_changed");
  assert.ok(!text.includes(SENT), text);
});

// Ticket 139: delivery_mode (seam A). Background is the default, as for computer_element_action
// (ticket 138): the target is not activated, the helper is told the mode, and the refusals are the same.

const RETRY = 'Retry this action with delivery_mode: "foreground"';

type ToolResult = { content?: unknown; isError?: boolean; structuredContent?: unknown };

/** The day's `tool_call` events of computer_set_value in `harness`'s log. */
async function toolCalls(harness: Harness): Promise<Record<string, unknown>[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call" && event.tool === "computer_set_value");
}

/** The helper requests after the tree was read (reading it shows the overlay too). */
async function methods(harness: Harness): Promise<string[]> {
  const all = ((await harness.requests()) as Logged[]).map((entry) => entry.method ?? "");
  return all.slice(all.lastIndexOf("get_ui_tree") + 1);
}

/** One set_value of SENT on a read tree; `window` is what get_window answers when the call looks the window up. */
async function setOnce(extra: Record<string, unknown>, answer: FakeHandler = { result: { read_back: SENT, read_back_matches: true } }, window: Record<string, unknown> = demo, index = 1) {
  const harness = await serverWith({
    get_window: [{ result: demo }, { result: window }],
    get_ui_tree: [treeResult("t1-aaaaa")],
    activate_window: [{ result: {} }],
    set_value: [answer],
  });
  try {
    await readTree(harness);
    const result = (await setValue(harness, { element_index: index, value: SENT, ...extra })) as ToolResult;
    await harness.finish();
    return { result, text: textContent(result), sent: await logged(harness, "set_value"), order: await methods(harness), calls: await toolCalls(harness) };
  } finally {
    await harness.close();
  }
}

test("ticket 139: delivery_mode is offered as background or foreground and is optional", async () => {
  const harness = await connectServer();
  try {
    const tool = (await harness.client.listTools()).tools.find((entry) => entry.name === "computer_set_value");
    const schema = tool?.inputSchema as { properties?: Record<string, { type?: string; enum?: string[] }>; required?: string[] };
    assert.equal(schema.properties?.delivery_mode?.type, "string");
    assert.deepEqual(schema.properties?.delivery_mode?.enum, ["background", "foreground"]);
    assert.ok(!schema.required?.includes("delivery_mode"));
  } finally {
    await harness.close();
  }
});

test("ticket 139: by default the value is set in the background: the helper is told so and the window is not activated", async () => {
  const { text, sent, order, result } = await setOnce({});
  assert.notEqual(result.isError, true, text);
  assert.equal(text, "Set the value of element [1] of tree t1-aaaaa; it reads back as sent.");
  assert.deepEqual(sent[0]?.params, { window_id: "5", expected_app: demo.app, tree_id: "t1-aaaaa", index: 1, value: SENT, delivery_mode: "background" });
  assert.ok(!order.includes("activate_window"), order.join(", "));
  assert.ok(order.lastIndexOf("overlay_show") < order.lastIndexOf("set_value"), order.join(", "));
});

test("ticket 139: an explicit background is the same as the default", async () => {
  const { sent, order } = await setOnce({ delivery_mode: "background" });
  assert.equal(sent[0]?.params?.delivery_mode, "background");
  assert.ok(!order.includes("activate_window"), order.join(", "));
});

test("ticket 139: an unknown delivery_mode is invalid_params before approval, overlay or input, without quoting it or the value", async () => {
  const harness = await serverWith({ get_window: [{ result: demo }], get_ui_tree: [treeResult("t1-aaaaa")] });
  try {
    await readTree(harness);
    for (const mode of ["auto", "Background", "", 1, null, true]) {
      const result = await setValue(harness, { element_index: 1, value: SENT, delivery_mode: mode });
      assert.equal(code(result), "invalid_params", `${JSON.stringify(mode)}: ${textContent(result)}`);
      assert.match(textContent(result), /delivery_mode/);
      assert.ok(!textContent(result).includes("auto"), textContent(result));
      assert.ok(!textContent(result).includes(SENT), textContent(result));
    }
    assert.equal(harness.elicitations.length, 1, "only the tree read asked");
    const order = await methods(harness);
    for (const method of ["set_value", "overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
  } finally {
    await harness.close();
  }
});

test("ticket 139: a minimized window in background delivery is background_unavailable (minimized): no overlay, activation or input", async () => {
  const { result, text, order, sent } = await setOnce({}, undefined, { ...demo, minimized: true });
  assert.equal(result.isError, true);
  assert.equal(code(result), "background_unavailable", text);
  assert.ok(text.includes(RETRY), text);
  assert.ok(!text.includes(SENT), text);
  assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
  assert.equal(sent.length, 0);
  for (const method of ["overlay_show", "activate_window"]) assert.ok(!order.includes(method), order.join(", "));
});

test("ticket 139: in foreground delivery a minimized window goes ahead as today", async () => {
  const { result, text, order } = await setOnce({ delivery_mode: "foreground" }, undefined, { ...demo, minimized: true });
  assert.notEqual(result.isError, true, text);
  assert.ok(order.includes("activate_window"));
});

test("ticket 139: the helper's background_unavailable reaches the model with its message and its reason", async () => {
  const message = `The target window is minimized, so the action cannot be delivered in the background and nothing was sent. ${RETRY}.`;
  const { result, text } = await setOnce({}, { error: { code: "background_unavailable", message, data: { reason: "minimized", recommended: "foreground", input_sent: false } } });
  assert.equal(text, `background_unavailable\n${message}`);
  assert.deepEqual(result.structuredContent, { reason: "minimized", recommended: "foreground" });
});

test("ticket 139: dialog_open in background delivery reaches the model as today", async () => {
  const { text } = await setOnce({}, { error: { code: "dialog_open", message: "x", data: { window_id: "99", title: "Save As" } } });
  assert.match(text, /^dialog_open\nThe target window has an open dialog \(window_id: 99, title: "Save As"\)/);
  assert.ok(!text.includes(SENT), text);
});

test("ticket 139: a password field in background delivery says it was set, and neither the result nor the log holds the value", async () => {
  const { text, result, calls } = await setOnce({}, { result: { password: true } }, demo, 2);
  assert.equal(text, "Set element [2] of tree t1-aaaaa: the value was set; password fields cannot be read back.");
  assert.ok(!JSON.stringify(result).includes(SENT), JSON.stringify(result));
  assert.ok(!JSON.stringify(calls).includes(SENT), JSON.stringify(calls));
  const refused = await setOnce({}, { error: { code: "password_field", message: "The element is a password field, so no input was sent.", data: { input_sent: false } } }, demo, 2);
  assert.equal(refused.text.split("\n")[0], "password_field");
  assert.ok(!refused.text.includes(SENT), refused.text);
  assert.ok(!JSON.stringify(refused.calls).includes(SENT), JSON.stringify(refused.calls));
});

test("ticket 139: an app taking the foreground after a background value is reported as for other input", async () => {
  const { text } = await setOnce({}, { result: { read_back: SENT, read_back_matches: true, foreground_changed: { foreground: { window_id: "77", title: "Saved", app: demo.app, app_name: "Demo" }, target_closed: false } } });
  assert.match(text, /After the value change, the foreground moved to another window of Demo \(window_id: 77/);
});

test("ticket 139: a foreground the helper gave back is foreground_restored in the result, its structuredContent and the log", async () => {
  const { result, text, calls } = await setOnce({}, { result: { read_back: SENT, read_back_matches: true, foreground_restored: true } });
  assert.notEqual(result.isError, true);
  assert.match(text, /^Set the value of element \[1\] of tree t1-aaaaa; it reads back as sent\./);
  assert.match(text, /took the foreground during the action; it was given back to the window that had it/);
  assert.deepEqual(result.structuredContent, { foreground_restored: true });
  assert.equal(calls[0]?.foreground_restored, true);
  assert.equal("restore_failed" in (calls[0] ?? {}), false);
});

test("ticket 139: a restore that failed tells the model the value was set and the user's window was taken, in the result, structuredContent and log", async () => {
  const answer = { result: { read_back: SENT, read_back_matches: true, foreground_changed: { foreground: { window_id: "42", title: "Demo", app: demo.app, app_name: "Demo" }, target_closed: false }, restore_failed: true } };
  const { result, text, calls } = await setOnce({}, answer);
  assert.notEqual(result.isError, true);
  assert.match(text, /^Set the value of element \[1\] of tree t1-aaaaa; it reads back as sent\./);
  assert.match(text, /The value change completed, but the target app took the foreground from the user's window and it could not be given back \(the foreground is now window_id 42\)\. Do not repeat the action\./);
  assert.doesNotMatch(text, /the foreground moved to/);
  assert.deepEqual(result.structuredContent, { restore_failed: true });
  assert.equal(calls[0]?.restore_failed, true);
  assert.equal("foreground_restored" in (calls[0] ?? {}), false);
});

test("ticket 139: a value whose foreground nobody took has neither field", async () => {
  const { result, calls } = await setOnce({});
  assert.equal(result.structuredContent, undefined);
  assert.equal("foreground_restored" in (calls[0] ?? {}), false);
  assert.equal("restore_failed" in (calls[0] ?? {}), false);
});

test("ticket 139: each call's log event says its delivery; a refused background call logs background_unavailable with its reason", async () => {
  const background = await setOnce({});
  assert.equal(background.calls.length, 1);
  assert.equal(background.calls[0]?.delivery, "background");
  assert.equal(background.calls[0]?.result, "ok");
  const foreground = await setOnce({ delivery_mode: "foreground" });
  assert.equal(foreground.calls[0]?.delivery, "foreground");
  const refused = await setOnce({}, undefined, { ...demo, minimized: true });
  assert.equal(refused.calls[0]?.result, "background_unavailable");
  assert.equal(refused.calls[0]?.reason, "minimized");
  assert.equal(refused.calls[0]?.delivery, "background");
  const fromHelper = await setOnce({}, { error: { code: "background_unavailable", message: "m", data: { reason: "minimized", recommended: "foreground" } } });
  assert.equal(fromHelper.calls[0]?.result, "background_unavailable");
  assert.equal(fromHelper.calls[0]?.reason, "minimized");
  const invalid = await setOnce({ delivery_mode: "auto" });
  assert.equal(invalid.calls[0]?.result, "invalid_params");
  assert.equal("reason" in (invalid.calls[0] ?? {}), false);
});

test("ticket 139: background delivery keeps the elevated and blocked-tier refusals before any overlay or input", async () => {
  const cases: [Record<string, unknown>, string][] = [
    [{ ...demo, elevated: true, helper_elevated: false }, "elevated_target"],
    [{ ...demo, app: "C:\\Windows\\System32\\consent.exe", app_name: "Consent" }, "blocked_app"],
  ];
  for (const [window, expected] of cases) {
    const { result, text, order } = await setOnce({ delivery_mode: "background" }, undefined, window);
    assert.equal(code(result), expected, text);
    for (const method of ["overlay_show", "activate_window", "set_value"]) assert.ok(!order.includes(method), `${expected}: ${order.join(", ")}`);
  }
});

test("ticket 139: background delivery sends the expected_app check to the helper and its app_changed reaches the model", async () => {
  const { text, sent } = await setOnce({}, { error: { code: "app_changed", message: "The window now belongs to C:\\Apps\\other.exe, not the approved app." } });
  assert.equal(sent[0]?.params?.expected_app, demo.app);
  assert.equal(text, "app_changed\nThe window now belongs to C:\\Apps\\other.exe, not the approved app.");
});

test("ticket 149: how long a background value waited for the user is in the result, its structuredContent and the log", async () => {
  const { result, text, calls } = await setOnce({}, { result: { read_back: SENT, read_back_matches: true, waited_for_idle_ms: 800 } });
  assert.match(text, /^Set the value of element \[1\] of tree t1-aaaaa; it reads back as sent\. /);
  assert.match(text, /waited 800 ms \(waited_for_idle_ms\)/);
  assert.deepEqual(result.structuredContent, { waited_for_idle_ms: 800 });
  assert.equal(calls[0]?.waited_for_idle_ms, 800);
});
