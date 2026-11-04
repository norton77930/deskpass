import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { ASK_USER_DIALOG_MS } from "../src/config.ts";
import { approvalEvents, connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// Ticket 97: computer_read_clipboard (seam one: the MCP client, the server, and a scripted helper).

const allow: FakeHandler = { result: { answer: "allow" } };
const deny: FakeHandler = { result: { answer: "deny" } };
const timedOut: FakeHandler = { result: { answer: "timeout" } };
const cannotShow: FakeHandler = { error: { code: "prompt_unavailable", message: "There is no interactive desktop." } };
const clip = (text: string | null): FakeHandler => ({ result: { text } });

/** A string the log must never hold. */
const MARKER = "clip-97-Tr0ub4dor-你好";

const read = (args: Record<string, unknown> = {}) => ({ name: "computer_read_clipboard", arguments: args });

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

async function methods(harness: Harness): Promise<string[]> {
  return (await harness.requests()).map((entry) => String((entry as { method?: string }).method)).filter((method) => method !== "ping" && method !== "process_info");
}

async function asks(harness: Harness): Promise<Record<string, unknown>[]> {
  return (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "ask_user").map((entry) => (entry as { params: Record<string, unknown> }).params);
}

async function logEvents(harness: Harness): Promise<Record<string, unknown>[]> {
  const dir = join(harness.dir, "logs");
  const events: Record<string, unknown>[] = [];
  for (const name of await readdir(dir)) {
    for (const line of (await readFile(join(dir, name), "utf8")).split("\n")) if (line) events.push(JSON.parse(line) as Record<string, unknown>);
  }
  return events;
}

test("computer_read_clipboard is listed with only an optional action_summary, making 19 tools", async () => {
  const harness = await connectServer();
  try {
    const tools = (await harness.client.listTools()).tools;
    assert.equal(tools.length, 19);
    const tool = tools.find((entry) => entry.name === "computer_read_clipboard");
    assert.ok(tool);
    assert.deepEqual(Object.keys(tool.inputSchema.properties ?? {}), ["action_summary"]);
    assert.equal(tool.inputSchema.required?.length ?? 0, 0);
    assert.match(tool.description ?? "", /clipboard/i);
    assert.match(tool.description ?? "", /20,000|20000/);
  } finally {
    await harness.close();
  }
});

test("the first read asks through the helper dialog, never the elicitation, and the allow lasts the session", async () => {
  const harness = await connectServer({ elicitation: "form", script: { handlers: { ask_user: [allow], read_clipboard: [clip("first text"), clip("second text")] } } });
  try {
    const first = await harness.client.callTool(read());
    assert.notEqual(first.isError, true, textContent(first));
    assert.match(textContent(first), /first text/);
    const second = await harness.client.callTool(read());
    assert.notEqual(second.isError, true, textContent(second));
    assert.match(textContent(second), /second text/);
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual(await asks(harness), [{ kind: "read_clipboard", timeout_ms: ASK_USER_DIALOG_MS }]);
    assert.deepEqual(await methods(harness), ["ask_user", "read_clipboard", "read_clipboard"]);
    const approvals = await approvalEvents(harness);
    assert.deepEqual(
      approvals.map(({ app, subject, channel, answer }) => ({ app, subject, channel, answer })),
      [
        { app: null, subject: "clipboard", channel: "dialog", answer: "allow_once" },
        { app: null, subject: "clipboard", channel: "session", answer: "allow_once" },
      ],
    );
  } finally {
    await harness.close();
  }
});

test("a refusal is remembered: the next read is approval_refused_earlier without asking or reading", async () => {
  const harness = await connectServer({ script: { handlers: { ask_user: [deny] } } });
  try {
    const refused = await harness.client.callTool(read());
    assert.equal(refused.isError, true);
    assert.equal(code(refused), "approval_declined");
    assert.match(textContent(refused), /clipboard/i);
    const again = await harness.client.callTool(read());
    assert.equal(code(again), "approval_refused_earlier");
    assert.match(textContent(again), /clipboard/i);
    assert.match(textContent(again), /restart/i);
    assert.deepEqual(await methods(harness), ["ask_user"]);
    assert.deepEqual(
      (await approvalEvents(harness)).map(({ channel, answer }) => ({ channel, answer })),
      [
        { channel: "dialog", answer: "refuse" },
        { channel: "session", answer: "refused_earlier" },
      ],
    );
  } finally {
    await harness.close();
  }
});

test("an unanswered question is not remembered: the next read asks again", async () => {
  const harness = await connectServer({ script: { handlers: { ask_user: [timedOut, allow], read_clipboard: [clip("later")] } } });
  try {
    const unanswered = await harness.client.callTool(read());
    assert.equal(code(unanswered), "approval_declined");
    assert.match(textContent(unanswered), /did not answer/i);
    const allowed = await harness.client.callTool(read());
    assert.notEqual(allowed.isError, true, textContent(allowed));
    assert.deepEqual(await methods(harness), ["ask_user", "ask_user", "read_clipboard"]);
    assert.deepEqual(
      (await approvalEvents(harness)).map(({ channel, answer }) => ({ channel, answer })),
      [
        { channel: "dialog", answer: "timeout" },
        { channel: "dialog", answer: "allow_once" },
      ],
    );
  } finally {
    await harness.close();
  }
});

test("a dialog that cannot be shown is approval_unavailable, with no fallback to the elicitation, and is not remembered", async () => {
  const harness = await connectServer({ elicitation: "form", script: { handlers: { ask_user: [cannotShow, allow], read_clipboard: [clip("x")] } } });
  try {
    const unavailable = await harness.client.callTool(read());
    assert.equal(code(unavailable), "approval_unavailable");
    assert.equal(harness.elicitations.length, 0);
    const allowed = await harness.client.callTool(read());
    assert.notEqual(allowed.isError, true, textContent(allowed));
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual((await approvalEvents(harness)).map(({ answer }) => answer), ["unavailable", "allow_once"]);
  } finally {
    await harness.close();
  }
});

test("text over 20,000 characters is cut at 20,000 code points, never inside a surrogate pair, and says so", async () => {
  const long = "a".repeat(19_999) + "😀😀b";
  const harness = await connectServer({ script: { handlers: { ask_user: [allow], read_clipboard: [clip(long)] } } });
  try {
    const result = await harness.client.callTool(read());
    assert.notEqual(result.isError, true, textContent(result));
    const blocks = (result.content as { type: string; text: string }[]).filter((block) => block.type === "text");
    assert.equal(blocks.length, 2);
    assert.equal(blocks[1].text, "a".repeat(19_999) + "😀");
    assert.match(blocks[0].text, /20,002/);
    assert.match(blocks[0].text, /first 20,000/);
    assert.match(blocks[0].text, /truncated/i);
  } finally {
    await harness.close();
  }
});

test("a clipboard without text is a normal result saying so, not an error", async () => {
  const harness = await connectServer({ script: { handlers: { ask_user: [allow], read_clipboard: [clip(null), clip("")] } } });
  try {
    for (let index = 0; index < 2; index += 1) {
      const result = await harness.client.callTool(read());
      assert.notEqual(result.isError, true, textContent(result));
      assert.match(textContent(result), /has no text/i);
    }
  } finally {
    await harness.close();
  }
});

test("a busy clipboard is clipboard_busy", async () => {
  const busy: FakeHandler = { error: { code: "clipboard_busy", message: "Another program is using the clipboard, so it could not be read." } };
  const harness = await connectServer({ script: { handlers: { ask_user: [allow], read_clipboard: [busy] } } });
  try {
    const result = await harness.client.callTool(read());
    assert.equal(result.isError, true);
    assert.equal(code(result), "clipboard_busy");
    assert.match(textContent(result), /another program/i);
  } finally {
    await harness.close();
  }
});

test("computer_read_clipboard runs as a batch step", async () => {
  const harness = await connectServer({ script: { handlers: { ask_user: [allow], read_clipboard: [clip("from the batch")] } } });
  try {
    const result = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "read_clipboard", action_summary: "Read what the user copied" }, { action: "wait", seconds: 0.1 }] },
    });
    assert.notEqual(result.isError, true, textContent(result));
    const text = textContent(result);
    assert.match(text, /completed 2 of 2 steps/);
    assert.match(text, /\[1\] read_clipboard: ok /);
    assert.match(text, /from the batch/);
  } finally {
    await harness.close();
  }
});

test("the log has clipboard_read with the character count and never the clipboard's text", async () => {
  const harness = await connectServer({ script: { handlers: { ask_user: [allow], read_clipboard: [clip(MARKER), clip(MARKER)] } } });
  try {
    assert.notEqual((await harness.client.callTool(read({ action_summary: "Read the copied text" }))).isError, true);
    await harness.client.callTool({ name: "computer_batch", arguments: { steps: [{ action: "read_clipboard" }] } });
    await harness.finish();
    const dir = join(harness.dir, "logs");
    for (const name of await readdir(dir)) assert.equal((await readFile(join(dir, name), "utf8")).includes(MARKER), false, name);
    const events = await logEvents(harness);
    const reads = events.filter((event) => event.event === "clipboard_read");
    assert.deepEqual(
      reads.map(({ time, session, ...rest }) => rest),
      [
        { event: "clipboard_read", chars: Array.from(MARKER).length, truncated: false },
        { event: "clipboard_read", chars: Array.from(MARKER).length, truncated: false },
      ],
    );
    const calls = events.filter((event) => event.event === "tool_call" && event.tool === "computer_read_clipboard");
    assert.deepEqual(
      calls.map(({ step, app, action_summary, result }) => ({ step, app, action_summary, result })),
      [
        { step: null, app: null, action_summary: "Read the copied text", result: "ok" },
        { step: 1, app: null, action_summary: null, result: "ok" },
      ],
    );
  } finally {
    await harness.close();
  }
});
