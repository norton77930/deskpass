import assert from "node:assert/strict";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, repoRoot, type FakeHandler, type HarnessOptions } from "./harness.ts";

// Ticket 76: computer_batch runs steps in order through the same step runner as the single tools
// (seam A: the MCP client, the server, and a scripted helper).

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

const captured = {
  image_base64: "RlJBTUUx",
  original_width: 400,
  original_height: 300,
  window_x: 10,
  window_y: 20,
  window_width: 400,
  window_height: 300,
};

const windows = (count: number): FakeHandler[] => Array.from({ length: count }, () => ({ result: demo }));

type Tool = { name: string; inputSchema: { properties?: Record<string, unknown>; required?: string[] } };
type StepSchema = { type: string; properties: Record<string, unknown>; required: string[] };

test("computer_batch lists one step schema per single tool but launch_app, made from that tool's own schema", async () => {
  const harness = await connectServer();
  try {
    const { tools } = (await harness.client.listTools()) as { tools: Tool[] };
    const batch = tools.find((tool) => tool.name === "computer_batch");
    assert.ok(batch, "computer_batch is listed");
    const steps = batch.inputSchema.properties?.steps as { type: string; minItems: number; maxItems: number; items: { anyOf: StepSchema[] } };
    assert.deepEqual([steps.type, steps.minItems, steps.maxItems], ["array", 1, 20]);
    assert.deepEqual(batch.inputSchema.required, ["steps"]);
    const singles = tools.filter((tool) => tool.name !== "computer_batch" && tool.name !== "computer_launch_app");
    assert.deepEqual(
      steps.items.anyOf.map((step) => (step.properties.action as { const: string }).const),
      singles.map((tool) => tool.name.slice("computer_".length)),
    );
    for (const tool of singles) {
      const action = tool.name.slice("computer_".length);
      const step = steps.items.anyOf.find((entry) => (entry.properties.action as { const: string }).const === action);
      assert.ok(step);
      // element_action has its own `action` parameter: in a step it is named after the step's action.
      const own = (name: string) => (name === "action" ? action : name);
      const { action: _stepAction, ...properties } = step.properties;
      assert.deepEqual(
        properties,
        Object.fromEntries(Object.entries(tool.inputSchema.properties ?? {}).map(([name, schema]) => [own(name), schema])),
        action,
      );
      assert.deepEqual(step.required, ["action", ...(tool.inputSchema.required ?? []).map(own)], action);
    }
  } finally {
    await harness.close();
  }
});

type CallResult = { isError?: boolean; content?: Array<{ type: string; text?: string; data?: string }> };

function methodsOf(requests: unknown[]): string[] {
  return requests.map((entry) => String((entry as { method?: string }).method)).filter((method) => method !== "ping");
}

async function runBatch(options: HarnessOptions, steps: unknown[], before: [string, Record<string, unknown>][] = []) {
  const harness = await connectServer(options);
  for (const [name, args] of before) await harness.client.callTool({ name, arguments: args });
  const result = (await harness.client.callTool({ name: "computer_batch", arguments: { steps } })) as CallResult;
  const text = (result.content ?? []).filter((block) => block.type === "text").map((block) => block.text ?? "");
  return { harness, result, text, lines: (text[0] ?? "").split("\n") };
}

test("an invalid step rejects the whole batch with invalid_params naming the step, before any helper request", async () => {
  const cases: [unknown[], RegExp][] = [
    [[], /1 to 20 steps/],
    [Array.from({ length: 21 }, () => ({ action: "wait", seconds: 0.1 })), /1 to 20 steps/],
    [[{ action: "wait", seconds: 0.1 }, { action: "launch_app", app: "notepad" }], /step 2\b.*launch_app/],
    [[{ action: "wait", seconds: 0.1 }, { action: "batch", steps: [] }], /step 2\b.*batch/],
    [[{ action: "fly" }], /step 1\b.*action must be one of list_windows, screenshot/],
    [["click"], /step 1\b/],
    [[{ action: "wait", seconds: 0.1 }, { action: "type_text", window_id: "5" }], /step 2\b.*text/],
    [[{ action: "wait", seconds: 0.1 }, { action: "wait", seconds: 0.1 }, { action: "scroll", window_id: "5", x: 1, y: 1, direction: "sideways" }], /step 3\b.*direction/],
    [[{ action: "list_windows" }, { action: "wait", seconds: 20 }], /step 2\b.*seconds/],
    [[{ action: "list_windows" }, { action: "click", window_id: "5" }], /step 2\b.*element_index/],
    [[{ action: "list_windows" }, { action: "click", window_id: "5", x: 1, y: 1, modifiers: ["hyper"] }], /step 2\b.*modifiers/],
    [[{ action: "list_windows" }, { action: "element_action", window_id: "5", element_index: 1, action_name: "invoke" }], /step 2\b.*element_action/],
  ];
  for (const [steps, message] of cases) {
    const { harness, result, lines } = await runBatch({ elicitation: "form" }, steps);
    try {
      assert.equal(result.isError, true, JSON.stringify(steps));
      assert.equal(lines[0], "invalid_params", JSON.stringify(steps));
      assert.match(lines.slice(1).join("\n"), message, JSON.stringify(steps));
      assert.deepEqual(methodsOf(await harness.requests()), [], JSON.stringify(steps));
    } finally {
      await harness.close();
    }
  }
});

test("a batch runs its steps in order and lists each one's result, then the images of its screenshot steps by step", async () => {
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: windows(4), capture_window: [{ result: captured }, { result: { ...captured, image_base64: "U0hPVDI=" } }] } } },
    [
      { action: "click", window_id: "5", x: 5, y: 5, delivery_mode: "foreground" },
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "screenshot", window_id: "5" },
    ],
    [["computer_screenshot", { window_id: "5" }]],
  );
  try {
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 3 of 3 steps");
    assert.equal(lines[1], "[1] click: ok Clicked left 1 time(s) at window pixel (5, 5).");
    assert.equal(lines[2], "[2] type_text: ok Typed text into the target window.");
    assert.equal(lines[3], "[3] screenshot: ok screenshot_id: ss_2");
    assert.deepEqual(result.content?.slice(1), [
      { type: "text", text: "image of step 3 (screenshot)" },
      { type: "image", data: "U0hPVDI=", mimeType: "image/jpeg" },
    ]);
    assert.deepEqual(methodsOf(await harness.requests()).filter((method) => ["click", "type_text"].includes(method)), ["click", "type_text"]);
  } finally {
    await harness.close();
  }
});

test("paste_text is a batch step: its result follows the step label, its text goes to the helper and is never quoted", async () => {
  const secret = "hunter2-Tr0ub4dor";
  const { harness, result, lines, text } = await runBatch(
    {
      elicitation: "form",
      script: {
        handlers: {
          get_window: windows(5),
          capture_window: [{ result: captured }, { result: { ...captured, image_base64: "U0hPVDI=" } }],
          paste_text: [{ result: { read_confirmed: false, clipboard_restored: "full" } }],
        },
      },
    },
    [
      { action: "click", window_id: "5", x: 5, y: 5 },
      { action: "paste_text", window_id: "5", text: secret },
      { action: "press_key", window_id: "5", key: "Return" },
      { action: "screenshot", window_id: "5" },
    ],
    [["computer_screenshot", { window_id: "5" }]],
  );
  try {
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 4 of 4 steps");
    assert.match(lines[2], /^\[2\] paste_text: ok Pasted the text into the target window: it read Ctrl\+V, .*\(read_confirmed: false\).*\(clipboard_restored: full\)\.$/);
    assert.equal(lines[3], "[3] press_key: ok Pressed return.");
    assert.ok(!text.join("\n").includes(secret));
    const requests = await harness.requests();
    assert.deepEqual(methodsOf(requests).filter((method) => ["click", "paste_text", "press_key"].includes(method)), ["click", "paste_text", "press_key"]);
    const pasted = requests.find((entry) => (entry as { method?: string }).method === "paste_text") as { params?: Record<string, unknown> };
    assert.deepEqual(pasted.params, { window_id: "5", expected_app: demo.app, text: secret });
  } finally {
    await harness.close();
  }
});

test("a paste_text step stopped before its Ctrl+V ends the batch with sent: false", async () => {
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      script: { handlers: { get_window: windows(2), paste_text: [{ error: { code: "interrupted", message: "Input stopped before it finished." }, eventsAfter: [{ event: "stopped", data: { generation: 1 } }] }] } },
    },
    [
      { action: "paste_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1), ["stopped at step 1 of 2: interrupted", `[1] paste_text: interrupted (sent: false) ${INTERRUPTED}`, "[2] press_key: not run"]);
  } finally {
    await harness.close();
  }
});

/** A batch of a paste_text step and a press_key step whose paste_text fails with `error`. */
async function failedPaste(error: { code: string; message: string; data?: Record<string, unknown> }) {
  const { harness, result, lines } = await runBatch({ elicitation: "form", script: { handlers: { get_window: windows(2), paste_text: [{ error }] } } }, [
    { action: "paste_text", window_id: "5", text: "hi" },
    { action: "press_key", window_id: "5", key: "Return" },
  ]);
  await harness.close();
  assert.equal(result.isError, true);
  return lines.slice(1);
}

test("ticket 87: a step's sent follows the helper's input_sent: false for each paste_text failure before Ctrl+V, true after it", async () => {
  const cases: [string, boolean][] = [
    ["The target window did not read the input within the 5-second limit for one call, so Ctrl+V was not sent and nothing was pasted.", false],
    ["Another program is using the clipboard, so nothing was pasted and no input was sent.", false],
    ["The target window is not visible, so no input was sent.", false],
    ["Ctrl+V was sent, but the target window did not read it within 5 seconds. Take a screenshot before retrying.", true],
  ];
  for (const [message, sent] of cases) {
    assert.deepEqual(
      await failedPaste({ code: "input_failed", message, data: { input_sent: sent } }),
      ["stopped at step 1 of 2: input_failed", `[1] paste_text: input_failed (sent: ${sent}) ${message}`, "[2] press_key: not run"],
      message,
    );
  }
});

test("ticket 92: a type_text step refused in a password field ends the batch there with sent: false and asks for the user", async () => {
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      script: {
        handlers: {
          get_window: windows(2),
          type_text: [{ error: { code: "password_field", message: "The keyboard focus is a password field, so no input was sent.", data: { input_sent: false } } }],
        },
      },
    },
    [
      { action: "type_text", window_id: "5", text: "hunter2" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1), ["stopped at step 1 of 2: password_field", `[1] type_text: password_field (sent: false) ${PASSWORD_FIELD}`, "[2] press_key: not run"]);
    assert.ok(!lines.join("\n").includes("hunter2"));
    assert.equal(methodsOf(await harness.requests()).includes("press_key"), false);
  } finally {
    await harness.close();
  }
});

test("ticket 92 review: a type_text step stopped where its Tab moved the focus into a password field is sent: true and says how much was typed", async () => {
  const partial =
    "The keyboard focus moved into a password field after character 4 of 10, so typing stopped: 4 of 10 characters were typed and the remaining 6 were not sent. Deskpass never enters passwords. Ask the user to type it themselves.";
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: windows(2), type_text: [{ error: { code: "password_field", message: partial, data: { input_sent: true } } }] } } },
    [
      { action: "type_text", window_id: "5", text: "abc\tsecret" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1), ["stopped at step 1 of 2: password_field", `[1] type_text: password_field (sent: true) ${partial}`, "[2] press_key: not run"]);
  } finally {
    await harness.close();
  }
});

const PASSWORD_FIELD =
  "The keyboard focus or the element is a password field, so no input was sent: Deskpass never types, pastes or sets passwords. " +
  "Ask the user to type it themselves and continue once they say it is done. Navigation keys such as Tab still work, to move the focus out of the field.";

test("ticket 87: without input_sent (an older helper) a paste_text input_failed is sent: true, whatever its message says", async () => {
  const message = "The target window did not read the input within the 5-second limit for one call, so Ctrl+V was not sent and nothing was pasted.";
  assert.deepEqual(await failedPaste({ code: "input_failed", message }), [
    "stopped at step 1 of 2: input_failed",
    `[1] paste_text: input_failed (sent: true) ${message}`,
    "[2] press_key: not run",
  ]);
});

const INTERRUPTED = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";
const STOPPED_AFTER_ACTION =
  "The user pressed Esc and stopped Deskpass after the action was taken, so its result was not read back. Do not repeat the action; once the user lets you continue, take a screenshot to check its result. Do not call these tools again now: end this turn and tell the user.";
const stopped = [{ event: "stopped", data: { generation: 1 } }];

/** The requests but the overlay_hide a stop itself sends. */
const afterStop = (requests: unknown[]) => methodsOf(requests).filter((method) => method !== "overlay_hide");

test("a stop between two steps ends the batch: the next step sends nothing at all", async () => {
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: windows(3), type_text: [{ result: {}, eventsAfter: stopped }] } } },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
      { action: "type_text", window_id: "5", text: "more" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1), [
      "stopped at step 2 of 3: interrupted",
      "[1] type_text: ok Typed text into the target window.",
      `[2] press_key: interrupted (sent: false) ${INTERRUPTED}`,
      "[3] type_text: not run",
    ]);
    assert.equal(afterStop(await harness.requests()).at(-1), "type_text");
  } finally {
    await harness.close();
  }
});

test("a stop after a step's input was sent ends the batch with that step's action-taken message", async () => {
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      script: { handlers: { get_window: windows(3), type_text: [{ error: { code: "interrupted", message: STOPPED_AFTER_ACTION }, eventsAfter: stopped }] } },
    },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1), ["stopped at step 1 of 2: interrupted", `[1] type_text: interrupted (sent: true) ${STOPPED_AFTER_ACTION}`, "[2] press_key: not run"]);
    assert.equal(afterStop(await harness.requests()).at(-1), "type_text");
  } finally {
    await harness.close();
  }
});

test("a batch started after Esc asks once whether to continue, then runs every step", async () => {
  const answers: string[] = [];
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      onElicit: (message) => {
        answers.push(message);
        return { action: "accept" };
      },
      script: { handlers: { list_windows: [{ events: stopped, result: { windows: [] } }], get_window: windows(3) } },
    },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
      { action: "type_text", window_id: "5", text: "more" },
    ],
    [["computer_list_windows", {}]],
  );
  try {
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 3 of 3 steps");
    assert.deepEqual(answers.filter((message) => message === "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?").length, 1);
    assert.equal(answers.length, 2, "the continue question and one approval");
  } finally {
    await harness.close();
  }
});

test("a batch started after Esc whose continue question is declined runs no step", async () => {
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      onElicit: () => ({ action: "decline" }),
      script: { handlers: { list_windows: [{ events: stopped, result: { windows: [] } }], get_window: windows(2) } },
    },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
    [["computer_list_windows", {}]],
  );
  try {
    assert.equal(result.isError, true);
    assert.equal(lines[0], "stopped_by_user");
    assert.equal(harness.elicitations.length, 1);
    assert.deepEqual(afterStop(await harness.requests()).filter((method) => !READS.has(method)), ["list_windows"]);
  } finally {
    await harness.close();
  }
});

test("a batch takes the operator lock once: held by another session, the batch is in_use and runs no step", async () => {
  const lockDir = join(repoRoot(), "test-tmp", `lock-${Date.now()}-batch`);
  await mkdir(lockDir, { recursive: true });
  await writeFile(join(lockDir, "session.lock"), JSON.stringify({ pid: 70001, startedAt: "2026-09-24T01:00:00.000Z" }));
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      lockDir,
      owner: { pid: 70002, startedAt: "2026-09-24T02:00:00.000Z" },
      script: { handlers: { get_window: windows(3), process_info: [{ result: { alive: true, started_ms: Date.parse("2026-09-24T01:00:00.000Z") } }] } },
    },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
      { action: "type_text", window_id: "5", text: "more" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.equal(lines[0], "in_use");
    assert.ok(!lines.some((line) => line.startsWith("stopped at") || line.startsWith("[")), "refused before any step, not by a step");
    assert.deepEqual(methodsOf(await harness.requests()), ["process_info"]);
  } finally {
    await harness.close();
    await rm(lockDir, { recursive: true, force: true });
  }
});

test("the 60 second limit is checked between steps: the step in progress finishes, the next one is timeout", async () => {
  let clock = 1_000_000;
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      now: () => clock,
      // The approval question of step 1 takes 61 seconds of the batch's time.
      onElicit: () => {
        clock += 61_000;
        return { action: "accept" };
      },
      script: { handlers: { get_window: windows(3) } },
    },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
      { action: "type_text", window_id: "5", text: "more" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(0, 3), ["timeout", "stopped at step 2 of 3: timeout", "[1] type_text: ok Typed text into the target window."]);
    assert.match(lines[3] ?? "", /^\[2\] press_key: timeout \(sent: false\) .*60-second limit.*after 1 of 3 steps/);
    assert.equal(lines[4], "[3] type_text: not run");
    assert.deepEqual(methodsOf(await harness.requests()).filter((method) => ["type_text", "press_key"].includes(method)), ["type_text"]);
  } finally {
    await harness.close();
  }
});

const treeOf = (treeId: string) => ({
  result: { tree_id: treeId, window_title: "Demo", elements: [{ depth: 0, index: 1, role: "button", name: "OK", states: [], actions: ["click"] }] },
});

test("steps without screenshot_id or tree_id use the window's latest screenshot and tree from before the batch, not ones it takes", async () => {
  const { harness, lines } = await runBatch(
    {
      elicitation: "form",
      script: {
        handlers: {
          get_window: windows(6),
          // The batch's own screenshot is at half scale: a point of it is twice as far in window pixels.
          capture_window: [{ result: captured }, { result: { ...captured, original_width: 800, original_height: 600 } }],
          get_ui_tree: [treeOf("t1"), treeOf("t2")],
        },
      },
    },
    [
      { action: "screenshot", window_id: "5" },
      { action: "get_ui_tree", window_id: "5" },
      { action: "click", window_id: "5", x: 5, y: 5 },
      { action: "click", window_id: "5", element_index: 1 },
    ],
    [
      ["computer_screenshot", { window_id: "5" }],
      ["computer_get_ui_tree", { window_id: "5" }],
    ],
  );
  try {
    assert.equal(lines[0], "completed 4 of 4 steps", lines.join("\n"));
    const clicks = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "click") as Array<{ params: Record<string, unknown> }>;
    assert.deepEqual([clicks[0]?.params.x, clicks[0]?.params.y], [5, 5]);
    assert.deepEqual(clicks[1]?.params.element, { tree_id: "t1", index: 1 });
  } finally {
    await harness.close();
  }
});

test("a step refused for what the batch started with (no screenshot) fails when it is reached, after the steps before it ran", async () => {
  const { harness, result, lines } = await runBatch({ elicitation: "form", script: { handlers: { get_window: windows(2) } } }, [
    { action: "type_text", window_id: "5", text: "hi" },
    { action: "click", window_id: "5", x: 5, y: 5 },
    { action: "press_key", window_id: "5", key: "Return" },
  ]);
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1, 3), ["stopped at step 2 of 3: no_screenshot", "[1] type_text: ok Typed text into the target window."]);
    assert.match(lines[3] ?? "", /^\[2\] click: no_screenshot \(sent: false\) /);
    assert.deepEqual(methodsOf(await harness.requests()).filter((method) => ["type_text", "click", "press_key"].includes(method)), ["type_text"]);
  } finally {
    await harness.close();
  }
});

test("the same parameters alone and as batch steps send the helper the same requests", async () => {
  const handlers = () => ({
    get_window: windows(20),
    capture_window: [{ result: captured }, { result: captured }, { result: captured }],
    get_ui_tree: [treeOf("t1"), treeOf("t2")],
    set_value: [{ result: { read_back: "hello", read_back_matches: true } }],
    element_action: [{ result: { state: "on", previous_state: "off" } }],
    list_windows: [{ result: { windows: [] } }],
  });
  const before: [string, Record<string, unknown>][] = [
    ["computer_screenshot", { window_id: "5" }],
    ["computer_get_ui_tree", { window_id: "5" }],
  ];
  // Screenshot and tree steps come last: alone, each would become the latest for the calls after it.
  const calls: [string, Record<string, unknown>][] = [
    ["activate_window", { window_id: "5" }],
    ["click", { window_id: "5", x: 5, y: 6, button: "right", click_count: 2, modifiers: ["Control"], delivery_mode: "foreground" }],
    ["click", { window_id: "5", element_index: 1 }],
    ["type_text", { window_id: "5", text: "hi" }],
    ["press_key", { window_id: "5", key: "ctrl+s" }],
    ["scroll", { window_id: "5", x: 5, y: 6, direction: "down", amount: 2 }],
    ["drag", { window_id: "5", from_x: 1, from_y: 2, to_x: 30, to_y: 40, modifiers: ["shift"] }],
    ["move_mouse", { window_id: "5", x: 7, y: 8 }],
    ["set_value", { window_id: "5", element_index: 1, value: "hello" }],
    ["element_action", { window_id: "5", element_index: 1, action: "toggle" }],
    ["zoom", { window_id: "5", region: [0, 0, 100, 50] }],
    ["wait", { seconds: 0.1 }],
    ["list_windows", {}],
    ["screenshot", { window_id: "5", include_ui_tree: true }],
  ];
  const strip = (requests: unknown[]) => requests.map((entry) => ({ method: (entry as { method: string }).method, params: (entry as { params?: unknown }).params }));

  const alone = await connectServer({ elicitation: "form", script: { handlers: handlers() } });
  let aloneRequests: unknown[];
  try {
    for (const [name, args] of before) await alone.client.callTool({ name, arguments: args });
    for (const [action, args] of calls) {
      const result = (await alone.client.callTool({ name: `computer_${action}`, arguments: args })) as CallResult;
      assert.notEqual(result.isError, true, `${action}: ${JSON.stringify(result.content)}`);
    }
    aloneRequests = strip(await alone.requests());
  } finally {
    await alone.close();
  }

  const steps = calls.map(([action, args]) => {
    const { action: own, ...rest } = args;
    return { action, ...rest, ...(own === undefined ? {} : { [action]: own }) };
  });
  const { harness, result, lines } = await runBatch({ elicitation: "form", script: { handlers: handlers() } }, steps, before);
  try {
    assert.equal(lines[0], `completed ${calls.length} of ${calls.length} steps`, lines.join("\n"));
    // Only the zoom and screenshot steps bring images, in step order, each after its step's label.
    assert.deepEqual(
      result.content?.slice(1).map((block) => (block.type === "text" ? block.text : block.type)),
      ["image of step 11 (zoom)", "image", "image of step 14 (screenshot)", "image"],
    );
    assert.deepEqual(strip(await harness.requests()), aloneRequests);
  } finally {
    await harness.close();
  }
});
/** Requests that send no input: a window read, the lock's question about the session's own process, and (ticket 93) the app's identity. */
const READS = new Set(["get_window", "process_info", "app_identity"]);

const UNREAD ="The target window did not read the key press within 2 seconds; the keys pressed so far were released.";

test("a batch stops at the first step that fails: its code, message and sent are shown and the later steps are not run", async () => {
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: windows(3), press_key: [{ error: { code: "input_failed", message: UNREAD } }] } } },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "ctrl+s" },
      { action: "type_text", window_id: "5", text: "more" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines, [
      "input_failed",
      "stopped at step 2 of 3: input_failed",
      "[1] type_text: ok Typed text into the target window.",
      `[2] press_key: input_failed (sent: true) ${UNREAD}`,
      "[3] type_text: not run",
    ]);
    assert.equal(result.content?.length, 1, "no images");
    assert.deepEqual(methodsOf(await harness.requests()).filter((method) => ["type_text", "press_key"].includes(method)), ["type_text", "press_key"]);
  } finally {
    await harness.close();
  }
});

test("a step whose app the user does not approve stops the batch before it sends anything", async () => {
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", onElicit: () => ({ action: "decline" }), script: { handlers: { get_window: windows(2) } } },
    [
      { action: "wait", seconds: 0.1 },
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.equal(lines[1], "stopped at step 2 of 3: approval_declined");
    assert.match(lines[3] ?? "", /^\[2\] type_text: approval_declined \(sent: false\) /);
    assert.equal(lines[4], "[3] press_key: not run");
    assert.equal(harness.elicitations.length, 1);
    assert.deepEqual(methodsOf(await harness.requests()).filter((method) => !READS.has(method)), []);
  } finally {
    await harness.close();
  }
});

// Code review of ticket 76.

test("a step without screenshot_id keeps the batch's starting screenshot after the batch's own screenshots push it out of the store", async () => {
  // Eight screenshots of the batch, at half scale, would leave the store without the one from before it.
  const half = { result: { ...captured, original_width: 800, original_height: 600 } };
  const { harness, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: windows(20), capture_window: [{ result: captured }, ...Array.from({ length: 8 }, () => half)] } } },
    [...Array.from({ length: 8 }, () => ({ action: "screenshot", window_id: "5" })), { action: "click", window_id: "5", x: 5, y: 5 }],
    [["computer_screenshot", { window_id: "5" }]],
  );
  try {
    assert.equal(lines[0], "completed 9 of 9 steps", lines.join("\n"));
    const clicks = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "click") as Array<{ params: Record<string, unknown> }>;
    assert.deepEqual([clicks[0]?.params.x, clicks[0]?.params.y], [5, 5]);
  } finally {
    await harness.close();
  }
});

test("a step's screenshot checks are made again when it is reached: a screenshot_id an earlier step takes is found", async () => {
  const { harness, lines } = await runBatch({ elicitation: "form", script: { handlers: { get_window: windows(4), capture_window: [{ result: captured }] } } }, [
    { action: "screenshot", window_id: "5" },
    { action: "click", window_id: "5", x: 5, y: 5, screenshot_id: "ss_1" },
  ]);
  try {
    assert.equal(lines[0], "completed 2 of 2 steps", lines.join("\n"));
    assert.ok(methodsOf(await harness.requests()).includes("click"));
  } finally {
    await harness.close();
  }
});

test("a stopped session is not asked to continue for a batch whose locked steps all come after a step that already fails its checks", async () => {
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", onElicit: () => ({ action: "decline" }), script: { handlers: { list_windows: [{ events: stopped, result: { windows: [] } }] } } },
    [
      { action: "wait", seconds: 0.1 },
      { action: "click", window_id: "5", x: 5, y: 5 },
      { action: "type_text", window_id: "5", text: "hi" },
    ],
    [["computer_list_windows", {}]],
  );
  try {
    assert.equal(result.isError, true);
    assert.equal(harness.elicitations.length, 0, JSON.stringify(harness.elicitations));
    assert.deepEqual(lines.slice(1, 3), ["stopped at step 2 of 3: no_screenshot", "[1] wait: ok Waited 0.1 seconds."]);
    assert.equal(lines.at(-1), "[3] type_text: not run");
  } finally {
    await harness.close();
  }
});

test("a stopped session runs a batch of only wait and list_windows steps without asking to continue", async () => {
  const { harness, result, lines } = await runBatch(
    {
      elicitation: "form",
      onElicit: () => ({ action: "decline" }),
      script: { handlers: { list_windows: [{ events: stopped, result: { windows: [] } }, { result: { windows: [] } }] } },
    },
    [{ action: "wait", seconds: 0.1 }, { action: "list_windows" }],
    [["computer_list_windows", {}]],
  );
  try {
    assert.notEqual(result.isError, true, lines.join("\n"));
    assert.equal(lines[0], "completed 2 of 2 steps");
    assert.equal(harness.elicitations.length, 0);
  } finally {
    await harness.close();
  }
});

test("off Windows a batch is refused as a whole with windows_only, like a single tool", async () => {
  const { harness, result, text } = await runBatch({ elicitation: "form", platform: "linux" }, [{ action: "wait", seconds: 0.1 }]);
  try {
    assert.equal(result.isError, true);
    assert.equal(text.join("\n"), "windows_only\nDeskpass supports Windows only.");
    assert.deepEqual(methodsOf(await harness.requests()), []);
  } finally {
    await harness.close();
  }
});

test("a helper crash in a step's input ends the batch there with sent: true and nothing more is asked of the helper", async () => {
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: windows(3), type_text: [{ result: {} }, { exit: true, stderr: "helper went away" }] } } },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "type_text", window_id: "5", text: "more" },
      { action: "press_key", window_id: "5", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.equal(lines[1], "stopped at step 2 of 3: helper_crashed");
    assert.match(lines[3] ?? "", /^\[2\] type_text: helper_crashed \(sent: true\) /);
    assert.equal(lines.at(-1), "[3] press_key: not run");
    assert.equal(methodsOf(await harness.requests()).at(-1), "type_text");
  } finally {
    await harness.close();
  }
});

test("a step that fails with an unexpected error ends the batch with internal", async () => {
  const { harness, result, lines } = await runBatch(
    // A window the helper reports 0 pixels wide: the screenshot's size is computed from it and throws.
    { elicitation: "form", script: { handlers: { get_window: [{ result: { ...demo, width: 0 } }, ...windows(2)] } } },
    [
      { action: "screenshot", window_id: "5" },
      { action: "type_text", window_id: "5", text: "hi" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.equal(lines[1], "stopped at step 1 of 2: internal", lines.join("\n"));
    assert.match(lines[2] ?? "", /^\[1\] screenshot: internal \(sent: false\) /);
    assert.equal(lines[3], "[2] type_text: not run");
    assert.ok(!methodsOf(await harness.requests()).includes("type_text"));
  } finally {
    await harness.close();
  }
});

test("Esc while a later step's app is being approved ends the batch at that step with interrupted, nothing of it sent", async () => {
  const other = { ...demo, window_id: "6", app: "C:\\Apps\\other.exe", app_name: "Other", title: "Other", pid: 4343 };
  const { harness, result, lines } = await runBatch(
    { elicitation: "form", script: { handlers: { get_window: [{ result: demo }, { result: other, eventsAfter: stopped }] } } },
    [
      { action: "type_text", window_id: "5", text: "hi" },
      { action: "type_text", window_id: "6", text: "more" },
      { action: "press_key", window_id: "6", key: "Return" },
    ],
  );
  try {
    assert.equal(result.isError, true);
    assert.deepEqual(lines.slice(1), [
      "stopped at step 2 of 3: interrupted",
      "[1] type_text: ok Typed text into the target window.",
      `[2] type_text: interrupted (sent: false) ${INTERRUPTED}`,
      "[3] press_key: not run",
    ]);
    assert.equal(harness.elicitations.length, 1, "only step 1's app was asked about");
    assert.deepEqual(afterStop(await harness.requests()).filter((method) => ["type_text", "press_key"].includes(method)), ["type_text"]);
  } finally {
    await harness.close();
  }
});

// Ticket 153 item 1: a step refused with background_unavailable gives the batch result its structured
// reason, as the single tool's result has it, not only the text.
test("ticket 153: a batch step refused with background_unavailable carries that step's reason and recommended in structuredContent", async () => {
  const { harness, result, lines } = await runBatch({ elicitation: "form", script: { handlers: { get_window: windows(2) } } }, [
    { action: "type_text", window_id: "5", text: "hi" },
    { action: "press_key", window_id: "5", key: "Return", delivery_mode: "background" },
    { action: "type_text", window_id: "5", text: "more" },
  ]);
  try {
    assert.equal(result.isError, true);
    assert.equal(lines[0], "background_unavailable");
    assert.match(lines[2] ?? "", /^\[1\] type_text: ok/);
    assert.match(lines[3] ?? "", /^\[2\] press_key: background_unavailable \(sent: false\)/);
    assert.deepEqual((result as { structuredContent?: unknown }).structuredContent, {
      steps: [{ step: 2, action: "press_key", code: "background_unavailable", reason: "tool_needs_foreground", recommended: "foreground" }],
    });
  } finally {
    await harness.close();
  }
});
