import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { PASTE_TEXT_TIMEOUT_MS } from "../src/config.ts";
import { createServer } from "../src/server.ts";
import type { StepResult } from "../src/steps.ts";
import { connectServer, repoRoot, retryingSpawn, textContent, type FakeHandler } from "./harness.ts";

// Ticket 77: computer_paste_text (seam A: the MCP client, the server, and a scripted helper).

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

/** Text that must never be quoted back: it may be a password. */
const SECRET = "hunter2-Tr0ub4dor";

const INTERRUPTED = "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";
const STOPPED_AFTER_ACTION =
  "The user pressed Esc and stopped Deskpass after the action was taken, so its result was not read back. Do not repeat the action; once the user lets you continue, take a screenshot to check its result. Do not call these tools again now: end this turn and tell the user.";
const stopped = [{ event: "stopped", data: { generation: 1 } }];

const methodsOf = (requests: unknown[]) => requests.map((entry) => String((entry as { method?: string }).method)).filter((method) => method !== "ping");
const paste = (text: string, window_id = "5") => ({ name: "computer_paste_text", arguments: { window_id, text } });

/** One computer_paste_text call against a helper whose paste_text answers `answer`. */
async function pasteAnswered(answer: FakeHandler) {
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: demo }], paste_text: [answer] } } });
  try {
    const result = await harness.client.callTool(paste(SECRET));
    return { result, text: textContent(result), requests: await harness.requests() };
  } finally {
    await harness.close();
  }
}

test("computer_paste_text takes window_id and text, and says it pastes through the clipboard, puts it back and keeps the text out of history and the cloud", async () => {
  const harness = await connectServer();
  try {
    const tool = (await harness.client.listTools()).tools.find((entry) => entry.name === "computer_paste_text");
    assert.ok(tool);
    assert.deepEqual(Object.keys(tool.inputSchema.properties ?? {}), ["window_id", "text", "delivery_mode", "action_summary"]);
    assert.deepEqual(tool.inputSchema.required, ["window_id", "text"]);
    const description = tool.description ?? "";
    assert.match(description, /clipboard/);
    assert.match(description, /long text/);
    assert.match(description, /20000 characters/);
    assert.match(description, /put back/);
    assert.match(description, /clipboard history/);
    assert.match(description, /cloud clipboard/);
  } finally {
    await harness.close();
  }
});

test("text that is empty, not a string, longer than 20000 characters, or holds a NUL is invalid_params before any helper request, and is not quoted", async () => {
  const tooLong = SECRET.repeat(Math.ceil(20_001 / SECRET.length)).slice(0, 20_001);
  // Characters, not UTF-16 units: 20000 emoji are 40000 units and still within the cap.
  const harness = await connectServer({ elicitation: "form" });
  try {
    for (const text of ["", 42, tooLong, `${SECRET}\u0000tail`]) {
      const result = await harness.client.callTool({ name: "computer_paste_text", arguments: { window_id: "5", text } });
      assert.equal(result.isError, true, String(text).slice(0, 20));
      const shown = textContent(result);
      assert.equal(shown.split("\n")[0], "invalid_params");
      assert.ok(!shown.includes(SECRET.slice(0, 6)), shown);
    }
    assert.match(textContent(await harness.client.callTool(paste(tooLong))), /20000 characters/);
    assert.match(textContent(await harness.client.callTool(paste("a\u0000b"))), /NUL/);
    assert.deepEqual(methodsOf(await harness.requests()), []);
  } finally {
    await harness.close();
  }
});

test("text of exactly 20000 characters, counted as characters, is sent to the helper as it is", async () => {
  const emoji = "\u{1F600}".repeat(20_000);
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: demo }], paste_text: [{ result: { read_confirmed: true, clipboard_restored: "full" } }] } } });
  try {
    const result = await harness.client.callTool(paste(emoji));
    assert.notEqual(result.isError, true, textContent(result));
    const sent = (await harness.requests()).find((entry) => (entry as { method?: string }).method === "paste_text") as { params?: Record<string, unknown> };
    assert.deepEqual(sent.params, { window_id: "5", expected_app: demo.app, text: emoji });
  } finally {
    await harness.close();
  }
});

test("a paste is approved once per app, activates the target, then asks the helper to paste with the expected app", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], paste_text: [{ result: { read_confirmed: true, clipboard_restored: "full" } }, { result: { read_confirmed: true, clipboard_restored: "full" } }] } },
  });
  try {
    for (let call = 0; call < 2; call += 1) {
      const result = await harness.client.callTool(paste(SECRET));
      assert.notEqual(result.isError, true, textContent(result));
    }
    assert.equal(harness.elicitations.length, 1);
    assert.ok(!harness.elicitations[0].includes(SECRET), harness.elicitations[0]);
    const methods = methodsOf(await harness.requests()).filter((method) => ["get_window", "activate_window", "paste_text"].includes(method));
    assert.deepEqual(methods, ["get_window", "activate_window", "paste_text", "get_window", "activate_window", "paste_text"]);
  } finally {
    await harness.close();
  }
});

test("a declined approval pastes nothing", async () => {
  const harness = await connectServer({ elicitation: "form", onElicit: () => ({ action: "decline" }), script: { handlers: { get_window: [{ result: demo }] } } });
  try {
    const result = await harness.client.callTool(paste(SECRET));
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split("\n")[0], "approval_declined");
    assert.ok(!methodsOf(await harness.requests()).includes("paste_text"));
  } finally {
    await harness.close();
  }
});

test("while another session holds the operator lock, computer_paste_text is in_use and nothing reaches the target", async () => {
  const lockDir = join(repoRoot(), "test-tmp", `lock-${Date.now()}-paste-text`);
  await mkdir(lockDir, { recursive: true });
  const holder = { pid: 70001, startedAt: "2026-09-24T01:00:00.000Z" };
  await writeFile(join(lockDir, "session.lock"), JSON.stringify(holder));
  const harness = await connectServer({
    elicitation: "form",
    lockDir,
    script: { handlers: { process_info: [{ result: { alive: true, started_ms: Date.parse(holder.startedAt) } }] } },
  });
  try {
    const result = await harness.client.callTool(paste(SECRET));
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split("\n")[0], "in_use");
    assert.deepEqual(new Set(methodsOf(await harness.requests())), new Set(["process_info"]));
  } finally {
    await harness.close();
    await rm(lockDir, { recursive: true, force: true });
  }
});

test("a confirmed paste with the clipboard put back in full says both", async () => {
  const { result, text } = await pasteAnswered({ result: { read_confirmed: true, clipboard_restored: "full" } });
  assert.notEqual(result.isError, true, text);
  assert.equal(
    text,
    "Pasted the text into the target window; the focused element's value now contains it (read_confirmed: true). The user's clipboard was put back as it was (clipboard_restored: full).",
  );
});

test("an unconfirmed paste says it was pasted but not confirmed, and to check a screenshot", async () => {
  const { result, text } = await pasteAnswered({ result: { read_confirmed: false, clipboard_restored: "full" } });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /^Pasted the text into the target window: it read Ctrl\+V, but its content cannot be read back or did not show the text within 300 ms, so the paste is not confirmed \(read_confirmed: false\);/);
  assert.match(text, /take a screenshot to check the text arrived before continuing or pasting again/);
  assert.match(text, /\(clipboard_restored: full\)\.$/);
});

test("a partial restore says some of what the user's clipboard held was not put back", async () => {
  const { result, text } = await pasteAnswered({ result: { read_confirmed: true, clipboard_restored: "partial" } });
  assert.notEqual(result.isError, true, text);
  assert.match(
    text,
    /The user's clipboard was put back, but some of what it held could not be saved or written back \(for example an image kept as a bitmap or metafile\), so it may not be exactly as the user left it \(clipboard_restored: partial\)\./,
  );
  assert.doesNotMatch(text, /still holds the pasted text/);
});

test("a restore that could not reopen the clipboard says the pasted text is still on it and the user may want to clear it, without quoting it", async () => {
  const { result, text } = await pasteAnswered({ result: { read_confirmed: true, clipboard_restored: "not_restored" } });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /\(clipboard_restored: not_restored\)/);
  assert.match(text, /the pasted text is still on the user's clipboard/);
  assert.match(text, /Tell the user, who may want to clear it/);
  assert.ok(!text.includes(SECRET), text);
});

test("a result without a clipboard_restored the server knows says the clipboard's state is not known", async () => {
  const { result, text } = await pasteAnswered({ result: { read_confirmed: true } });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /The helper did not say whether the user's clipboard was put back; it may still hold the pasted text\.$/);
});

test("a restore skipped because another program changed the clipboard says it was left as that program made it", async () => {
  const { result, text } = await pasteAnswered({ result: { read_confirmed: true, clipboard_restored: "skipped_changed" } });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /Another program changed the clipboard during the paste, so it was left as that program made it and the user's earlier clipboard was not put back \(clipboard_restored: skipped_changed\)\./);
});

test("a paste after which the foreground moved succeeds and names the new foreground", async () => {
  const guest = { window_id: "33883792", title: "Guest", app: "C:\\Apps\\demo.exe", app_name: "Demo" };
  const { result, text } = await pasteAnswered({ result: { read_confirmed: false, clipboard_restored: "full", foreground_changed: { foreground: guest, target_closed: false } } });
  assert.notEqual(result.isError, true, text);
  assert.match(text, /After the paste, the foreground moved to another window of Demo \(window_id: 33883792, title: "Guest"\)\. Take a screenshot of window_id 33883792 before continuing\.$/);
});

test("the helper's failures come back as they are, and no result or error quotes the text", async () => {
  const unread =
    "Ctrl+V was sent, but the target window did not read it within 2 seconds. It may still act on it later, and would then paste whatever the clipboard holds by then rather than this text (the user's clipboard was put back). Take a screenshot before retrying.";
  const { result, text, requests } = await pasteAnswered({ error: { code: "input_failed", message: unread } });
  assert.equal(result.isError, true);
  assert.equal(text, `input_failed\n${unread}`);
  const sent = requests.find((entry) => (entry as { method?: string }).method === "paste_text") as { params?: { text?: string } };
  assert.equal(sent.params?.text, SECRET);
});

test("PASTE_TEXT_TIMEOUT_MS leaves the helper at least 7 seconds for one paste", () => {
  assert.ok(PASTE_TEXT_TIMEOUT_MS >= 7_000, String(PASTE_TEXT_TIMEOUT_MS));
});

/** The step result of one paste_text call, below the MCP rendering, against a helper whose paste_text answers `answer`. */
async function pasteStep(answer: FakeHandler): Promise<StepResult> {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const dir = await mkdtemp(join(root, "case-"));
  const scriptPath = join(dir, "script.json");
  const handlers = { get_window: [{ result: demo }], ask_user: [{ result: { answer: "allow" } }], paste_text: [answer] };
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
    return await app.callStep("paste_text", { window_id: "5", text: SECRET });
  } finally {
    await app.dispose();
    await rm(dir, { recursive: true, force: true });
  }
}

test("sent: false for a stop before Ctrl+V and for the helper's refusals, true once Ctrl+V may have been sent", async () => {
  const cases: [FakeHandler, string, boolean][] = [
    [{ error: { code: "interrupted", message: INTERRUPTED }, eventsAfter: stopped }, "interrupted", false],
    [{ error: { code: "interrupted", message: "Input stopped before it finished." }, eventsAfter: stopped }, "interrupted", false],
    [{ error: { code: "interrupted", message: STOPPED_AFTER_ACTION }, eventsAfter: stopped }, "interrupted", true],
    [{ error: { code: "invalid_params", message: "text is longer than 20000 characters." } }, "invalid_params", false],
    [{ error: { code: "dialog_open", message: "A dialog of the target is open, so no input was sent." } }, "dialog_open", false],
    [{ error: { code: "input_failed", message: "Another program is using the clipboard, so nothing was pasted and no input was sent." } }, "input_failed", true],
    [{ error: { code: "input_failed", message: "Ctrl+V was sent, but the target window did not read it within 2 seconds." } }, "input_failed", true],
  ];
  for (const [answer, code, sent] of cases) {
    const result = await pasteStep(answer);
    assert.ok(!result.ok, JSON.stringify(result));
    assert.deepEqual([result.code, result.sent], [code, sent], JSON.stringify(answer));
    assert.ok(!result.message.includes(SECRET), result.message);
  }
});

test("a helper that dies during the paste says the user's clipboard may still hold the pasted text", async () => {
  const result = await pasteStep({ exit: 1 });
  assert.ok(!result.ok, JSON.stringify(result));
  assert.deepEqual([result.code, result.sent], ["helper_crashed", true]);
  assert.match(result.message, /The user's clipboard may still hold the pasted text; tell the user, who may want to clear it\./);
  assert.ok(!result.message.includes(SECRET), result.message);
});
