import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { connectServer } from "./harness.ts";

// Ticket 147: the descriptions of the tools with a delivery_mode say how to use background delivery
// (seam A: tools/list). Every tool, reason and result field they mention exists.

interface Listed {
  name: string;
  description?: string;
  inputSchema: { properties?: Record<string, { description?: string; enum?: string[] }> };
}

const BACKGROUND_TOOLS = ["computer_click", "computer_element_action", "computer_set_value"];
const FOREGROUND_ONLY_TOOLS = ["computer_type_text", "computer_press_key", "computer_scroll", "computer_drag", "computer_paste_text"];

/** The reasons a background_unavailable can carry, and the ones each background-capable tool can give. */
const REASONS_OF: Record<string, string[]> = {
  computer_click: ["needs_pointer", "no_click_pattern", "no_element_at_point", "minimized"],
  computer_element_action: ["focus_needs_foreground", "minimized"],
  computer_set_value: ["minimized"],
};

/** The fields a background action's result carries, which the descriptions explain. */
const RESULT_FIELDS = ["foreground_restored", "restore_failed", "waited_for_idle_ms"];

async function listed(): Promise<Listed[]> {
  const harness = await connectServer();
  try {
    return (await harness.client.listTools()).tools as Listed[];
  } finally {
    await harness.close();
  }
}

function describe(tool: Listed | undefined): string {
  assert.ok(tool, "tool is listed");
  return tool.description ?? "";
}

test("ticket 147: a background-capable tool says its default, background_unavailable and its reasons, and the foreground retry", async () => {
  const tools = await listed();
  for (const name of BACKGROUND_TOOLS) {
    const text = describe(tools.find((tool) => tool.name === name));
    assert.match(text, /delivery_mode[^.]*defaults? to "?background/i, `${name}: default`);
    assert.match(text, /background_unavailable/, name);
    for (const reason of REASONS_OF[name] ?? []) assert.match(text, new RegExp(`\\b${reason}\\b`), `${name}: ${reason}`);
    assert.match(text, /nothing was sent/i, `${name}: what a refusal means`);
    assert.match(text, /use background first/i, name);
    assert.match(text, /only the refused action[^.]*delivery_mode: "foreground"/i, `${name}: redo only the refused action`);
    assert.match(text, /then (keep using|use) background/i, `${name}: back to background after the retry`);
  }
});

test("ticket 147: click and element_action say Invoke is not a real click and to confirm with a screenshot; set_value does not", async () => {
  const tools = await listed();
  for (const name of ["computer_click", "computer_element_action"]) {
    const text = describe(tools.find((tool) => tool.name === name));
    assert.match(text, /Invoke is not a real click/, name);
    assert.match(text, /screenshot/i, name);
    assert.match(text, /nothing changed[^.]*foreground/i, `${name}: redo in foreground`);
  }
  assert.doesNotMatch(describe(tools.find((tool) => tool.name === "computer_set_value")), /Invoke is not a real click|nothing changed/);
});

test("ticket 147: a background-capable tool explains foreground_restored, restore_failed and waited_for_idle_ms", async () => {
  const tools = await listed();
  for (const name of BACKGROUND_TOOLS) {
    const text = describe(tools.find((tool) => tool.name === name));
    assert.match(text, /foreground_restored[^.]*needs no handling/i, `${name}: foreground_restored`);
    assert.match(text, /restore_failed[^.]*action was done[^.]*user's window[^.]*do not redo/i, `${name}: restore_failed`);
    assert.match(text, /waited_for_idle_ms[^.]*waiting for the user to stop/i, `${name}: waited_for_idle_ms`);
  }
});

test("ticket 147: a background-capable tool says the user's switch turns background actions into foreground ones", async () => {
  const tools = await listed();
  for (const name of BACKGROUND_TOOLS) {
    assert.match(describe(tools.find((tool) => tool.name === name)), /turned (background operation|it) off[^.]*foreground/i, name);
  }
});

test("ticket 147: a foreground-only tool says its default and that background is refused with tool_needs_foreground", async () => {
  const tools = await listed();
  for (const name of FOREGROUND_ONLY_TOOLS) {
    const tool = tools.find((candidate) => candidate.name === name);
    const text = describe(tool);
    assert.match(text, /delivery_mode[^.]*defaults? to "?foreground/i, `${name}: default`);
    assert.match(text, /"background" is refused with background_unavailable/, name);
    assert.match(text, /\btool_needs_foreground\b/, name);
    assert.match(text, /nothing is sent/i, name);
    assert.match(String(tool?.inputSchema.properties?.delivery_mode?.description), /Defaults to "foreground"/, name);
  }
});

test("ticket 147: computer_batch says each step takes its tool's delivery_mode and a refused step stops the batch", async () => {
  const tools = await listed();
  const text = describe(tools.find((tool) => tool.name === "computer_batch"));
  assert.match(text, /delivery_mode/);
  assert.match(text, /background_unavailable/);
  assert.match(text, /stops the batch/i);
});

test("ticket 147: the descriptions mention only tools, parameters, reasons and result fields that exist", async () => {
  const tools = await listed();
  const delivery = await readFile(new URL("../src/delivery.ts", import.meta.url), "utf8");
  const server = await readFile(new URL("../src/server.ts", import.meta.url), "utf8");

  const union = /export type BackgroundUnavailableReason =([^;]+);/.exec(delivery)?.[1] ?? "";
  const reasons = [...union.matchAll(/"([a-z_]+)"/g)].map((match) => match[1] as string);
  assert.ok(reasons.length >= 6, `reasons read from src/delivery.ts: ${reasons.join(", ")}`);
  for (const name of Object.keys(REASONS_OF)) for (const reason of REASONS_OF[name] ?? []) assert.ok(reasons.includes(reason), `${name}: ${reason} is a reason`);

  for (const field of RESULT_FIELDS) assert.match(server, new RegExp(`structuredContent[^\\n]*\\b${field}\\b`), `${field} is a field of a result`);

  const toolNames = new Set(tools.map((tool) => tool.name));
  const parameters = new Set<string>();
  const values = new Set<string>();
  for (const tool of tools) {
    for (const [name, schema] of Object.entries(tool.inputSchema.properties ?? {})) {
      parameters.add(name);
      for (const value of schema.enum ?? []) values.add(value);
    }
  }
  const known = new Set<string>([...parameters, ...values, ...reasons, ...RESULT_FIELDS, "background_unavailable", ...[...toolNames].map((name) => name.slice("computer_".length))]);

  const texts: [string, string][] = [];
  for (const name of [...BACKGROUND_TOOLS, ...FOREGROUND_ONLY_TOOLS, "computer_batch"]) {
    const tool = tools.find((candidate) => candidate.name === name);
    texts.push([name, describe(tool)]);
    texts.push([`${name} delivery_mode`, tool?.inputSchema.properties?.delivery_mode?.description ?? ""]);
  }
  for (const [where, whole] of texts) {
    // Only the sentences about delivery: the rest of a description is older text other tests pin.
    const text = whole.split(/(?<=\.)\s+/).filter((sentence) => /delivery_mode|background|foreground/i.test(sentence)).join(" ");
    for (const token of text.match(/\b[a-z]+(?:_[a-z]+)+\b/g) ?? []) {
      if (token.startsWith("computer_")) assert.ok(toolNames.has(token), `${where}: tool ${token} exists`);
      else assert.ok(known.has(token), `${where}: unknown identifier ${token}`);
    }
    for (const match of text.matchAll(/delivery_mode: "([^"]*)"/g)) assert.ok(["background", "foreground"].includes(match[1] as string), `${where}: delivery_mode value ${match[1]}`);
    for (const reason of text.match(/\b(?:needs_pointer|no_click_pattern|no_element_at_point|focus_needs_foreground|minimized|tool_needs_foreground)\b/g) ?? []) {
      assert.ok(reasons.includes(reason), `${where}: reason ${reason} exists`);
    }
  }
  assert.deepEqual([...values].filter((value) => value === "background" || value === "foreground").sort(), ["background", "foreground"]);
});
