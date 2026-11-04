import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { createServer } from "../src/server.ts";
import type { StepResult } from "../src/steps.ts";
import { repoRoot, retryingSpawn, type FakeHandler } from "./harness.ts";

// Ticket 75: every tool call runs as one step whose structured result says, when it fails,
// whether input was sent (seam A, the server with a scripted helper, below the MCP rendering).

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

const STOPPED_AFTER_ACTION =
  "The user pressed Esc and stopped Deskpass after the action was taken, so its result was not read back. Do not repeat the action; once the user lets you continue, take a screenshot to check its result. Do not call these tools again now: end this turn and tell the user.";

const allow = { result: { answer: "allow" } };

/** The step results of `calls`, in order, against a helper that answers `handlers`; approval goes through its dialog. */
async function stepsWith(handlers: Record<string, FakeHandler[]>, calls: [string, Record<string, unknown>][]): Promise<StepResult[]> {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const dir = await mkdtemp(join(root, "case-"));
  const scriptPath = join(dir, "script.json");
  await writeFile(scriptPath, JSON.stringify({ handlers, logPath: join(dir, "requests.log") }));
  const app = createServer({
    lockDir: join(dir, "lock"),
    logDir: join(dir, "logs"),
    grantsDir: dir,
    spawn: retryingSpawn(),
    helperPath: process.execPath,
    helperArgs: [fileURLToPath(new URL("./fake-helper.mjs", import.meta.url)), "--script", scriptPath],
  });
  try {
    const results: StepResult[] = [];
    for (const [action, params] of calls) results.push(await app.callStep(action, params));
    return results;
  } finally {
    await app.dispose();
    await rm(dir, { recursive: true, force: true });
  }
}

async function lastFailure(handlers: Record<string, FakeHandler[]>, calls: [string, Record<string, unknown>][]) {
  const result = (await stepsWith(handlers, calls)).at(-1);
  assert.ok(result && !result.ok, JSON.stringify(result));
  return result;
}

test("a step that sent no input fails with sent: false", async () => {
  const badParams = await lastFailure({}, [["type_text", { window_id: "5" }]]);
  assert.deepEqual([badParams.code, badParams.sent], ["invalid_params", false]);

  const badAfterApproval = await lastFailure(
    { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: captured }], ask_user: [allow] },
    [["screenshot", { window_id: "5" }], ["scroll", { window_id: "5", x: 5, y: 5, direction: "sideways" }]],
  );
  assert.deepEqual([badAfterApproval.code, badAfterApproval.sent], ["invalid_params", false]);

  const declined = await lastFailure({ get_window: [{ result: demo }], ask_user: [{ result: { answer: "deny" } }] }, [["type_text", { window_id: "5", text: "hi" }]]);
  assert.deepEqual([declined.code, declined.sent], ["approval_declined", false]);

  const stoppedBefore = await lastFailure(
    { get_window: [{ result: demo, eventsAfter: [{ event: "stopped", data: { generation: 1 } }] }], ask_user: [allow] },
    [["type_text", { window_id: "5", text: "hi" }]],
  );
  assert.deepEqual([stoppedBefore.code, stoppedBefore.sent], ["interrupted", false]);

  const obscured = await lastFailure(
    {
      get_window: [{ result: demo }, { result: demo }, { result: demo }],
      capture_window: [{ result: captured }],
      ask_user: [allow],
      activate_window: [{ result: {} }],
      click: [{ error: { code: "target_obscured", message: "The point is covered, so no input was sent.", data: { window_id: "9", title: "Bar", app: "C:\\Apps\\bar.exe", app_name: "Bar" } } }],
    },
    [["screenshot", { window_id: "5" }], ["click", { window_id: "5", x: 5, y: 5 }]],
  );
  assert.deepEqual([obscured.code, obscured.sent], ["target_obscured", false]);
});

test("a step that failed after its input was sent fails with sent: true", async () => {
  const stoppedAfter = await lastFailure(
    {
      get_window: [{ result: demo }],
      ask_user: [allow],
      activate_window: [{ result: {} }],
      type_text: [{ error: { code: "interrupted", message: STOPPED_AFTER_ACTION }, eventsAfter: [{ event: "stopped", data: { generation: 1 } }] }],
    },
    [["type_text", { window_id: "5", text: "hi" }]],
  );
  assert.deepEqual([stoppedAfter.code, stoppedAfter.sent, stoppedAfter.message], ["interrupted", true, STOPPED_AFTER_ACTION]);

  const unread = await lastFailure(
    {
      get_window: [{ result: demo }],
      ask_user: [allow],
      activate_window: [{ result: {} }],
      press_key: [{ error: { code: "input_failed", message: "The target window did not read the key press within 2 seconds; the keys pressed so far were released." } }],
    },
    [["press_key", { window_id: "5", key: "ctrl+s" }]],
  );
  assert.deepEqual([unread.code, unread.sent], ["input_failed", true]);
});
