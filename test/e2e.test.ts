import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ElicitRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { test } from "node:test";
import { defaultHelperPath } from "../src/config.ts";
import { createServer } from "../src/server.ts";
import { repoRoot, textContent } from "./harness.ts";

// CI (.github/workflows/ci.yml) has no interactive desktop for the fixture windows these tests open.
const NO_DESKTOP = process.env.DESKPASS_NO_DESKTOP ? "DESKPASS_NO_DESKTOP: no interactive desktop" : false;

const TOOLS = [
  "computer_list_windows",
  "computer_launch_app",
  "computer_screenshot",
  "computer_get_ui_tree",
  "computer_activate_window",
  "computer_click",
  "computer_set_value",
  "computer_element_action",
  "computer_type_text",
  "computer_paste_text",
  "computer_press_key",
  "computer_scroll",
  "computer_drag",
  "computer_move_mouse",
  "computer_zoom",
  "computer_wait",
  "computer_batch",
  "computer_list_apps",
  "computer_read_clipboard",
];

test("the stdio server entry lists every computer use tool", async () => {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: ["--experimental-strip-types", join(repoRoot(), "src", "index.ts")],
    cwd: repoRoot(),
    stderr: "pipe",
  });
  const client = new Client({ name: "deskpass-e2e", version: "0.0.0" }, { capabilities: {} });
  await client.connect(transport);
  try {
    const listed = await client.listTools();
    assert.equal(listed.tools.length, TOOLS.length, listed.tools.map((tool) => tool.name).join(", "));
    for (const name of TOOLS) {
      assert.equal(listed.tools.some((tool) => tool.name === name), true, name);
    }
  } finally {
    await client.close();
  }
});

test("end to end: list, screenshot, click, and Chinese text on the fixture", { skip: NO_DESKTOP }, async () => {
  const fixtureExe = [
    join(repoRoot(), "helper", "target", "debug", "fixture-window.exe"),
    join(repoRoot(), "helper", "target", "release", "fixture-window.exe"),
  ].find((candidate) => existsSync(candidate));
  assert.ok(fixtureExe, "fixture-window.exe is missing; build the helper tests first");
  const helper = [
    join(repoRoot(), "helper", "target", "debug", "deskpass-helper.exe"),
    join(repoRoot(), "helper", "target", "release", "deskpass-helper.exe"),
    defaultHelperPath(),
  ].find((candidate) => existsSync(candidate));
  assert.ok(helper, "deskpass-helper.exe is missing");
  const fixture = spawn(fixtureExe, [], { stdio: "ignore" });
  const dir = await mkdtemp(join(repoRoot(), "test-tmp", "e2e-"));
  const app = createServer({ lockDir: join(dir, "lock"), logDir: join(dir, "logs"), grantsDir: dir, helperPath: helper });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "deskpass-e2e", version: "0.0.0" },
    { capabilities: { elicitation: { form: {} } } },
  );
  client.setRequestHandler(ElicitRequestSchema, async () => ({ action: "accept" as const, content: { decision: "allow" } }));
  await Promise.all([app.server.connect(serverTransport), client.connect(clientTransport)]);
  try {
    const windowId = await waitForFixture(client);
    const shot = await client.callTool({ name: "computer_screenshot", arguments: { window_id: windowId } });
    assert.notEqual(shot.isError, true, textContent(shot));
    const image = (shot.content as Array<{ type: string; mimeType?: string; data?: string }>).find((block) => block.type === "image");
    assert.equal(image?.mimeType, "image/jpeg");
    assert.ok(image?.data && image.data.length > 0);
    const shotText = textContent(shot);
    assert.match(shotText, /screenshot_id: ss_/);
    // The plain fixture draws its button without a UI element, so the point clicks need the real mouse (ticket 141).
    const click = await client.callTool({
      name: "computer_click",
      arguments: { window_id: windowId, x: imageX(shotText, 70, 480), y: imageY(shotText, 62, 360), button: "left", click_count: 1 , delivery_mode: "foreground" },
    });
    const clickText = textContent(click);
    assert.notEqual(click.isError, true, clickText);
    const focusEdit = await client.callTool({
      name: "computer_click",
      arguments: { window_id: windowId, x: imageX(shotText, 240, 480), y: imageY(shotText, 24, 360), button: "left", click_count: 1 , delivery_mode: "foreground" },
    });
    assert.notEqual(focusEdit.isError, true, textContent(focusEdit));
    const typed = await client.callTool({ name: "computer_type_text", arguments: { window_id: windowId, text: "你好" } });
    assert.notEqual(typed.isError, true, textContent(typed));
    const again = await client.callTool({ name: "computer_screenshot", arguments: { window_id: windowId } });
    assert.notEqual(again.isError, true, textContent(again));
    const listed = await client.callTool({ name: "computer_list_windows", arguments: {} });
    const block = textContent(listed).split(/\n\n/).find((entry) => entry.includes(`window_id: ${windowId}`));
    assert.ok(block?.includes("你好"), block);
  } finally {
    await closeFixture(fixture);
    await client.close();
    await app.dispose();
    await rm(dir, { recursive: true, force: true });
  }
});

test("end to end: a screenshot with include_ui_tree puts the tree's button inside the button on the image", { skip: NO_DESKTOP }, async () => {
  const fixtureExe = builtExe("fixture-window.exe");
  assert.ok(fixtureExe, "fixture-window.exe is missing; build the helper tests first");
  const helper = builtExe("deskpass-helper.exe");
  assert.ok(helper, "deskpass-helper.exe is missing");
  const fixture = spawn(fixtureExe, ["--ui-controls"], { stdio: "ignore" });
  const dir = await mkdtemp(join(repoRoot(), "test-tmp", "e2e-"));
  const app = createServer({ lockDir: join(dir, "lock"), logDir: join(dir, "logs"), grantsDir: dir, helperPath: helper });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "deskpass-e2e", version: "0.0.0" }, { capabilities: { elicitation: { form: {} } } });
  client.setRequestHandler(ElicitRequestSchema, async () => ({ action: "accept" as const, content: { decision: "allow" } }));
  await Promise.all([app.server.connect(serverTransport), client.connect(clientTransport)]);
  try {
    const windowId = await waitForFixture(client);
    const shot = await client.callTool({ name: "computer_screenshot", arguments: { window_id: windowId, include_ui_tree: true } });
    assert.notEqual(shot.isError, true, textContent(shot));
    const texts = (shot.content as Array<{ type: string; text?: string }>).filter((block) => block.type === "text").map((block) => block.text ?? "");
    assert.equal(texts.length, 2, texts.join("\n---\n"));
    const [meta = "", treeText = ""] = texts;
    const screenshotId = meta.match(/screenshot_id: (ss_\d+)/)?.[1];
    assert.ok(screenshotId, meta);
    assert.match(treeText, new RegExp(`tree_id t\\d+[^ ]* \\(screenshot ${screenshotId}\\)`));
    // The fixture's "Click me" button is at 10,48 and 120x28 in its designed 480x360 window.
    const line = treeText.split("\n").find((entry) => entry.includes('button "Click me"'));
    assert.ok(line, treeText);
    const range = line.match(/\(x (-?\d+)-(-?\d+), y (-?\d+)-(-?\d+)\)/);
    assert.ok(range, line);
    const [left, right, top, bottom] = range.slice(1).map(Number) as [number, number, number, number];
    const expected = {
      left: imageX(meta, 10, 480),
      right: imageX(meta, 130, 480) - 1,
      top: imageY(meta, 48, 360),
      bottom: imageY(meta, 76, 360) - 1,
    };
    for (const [name, got] of Object.entries({ left, right, top, bottom })) {
      assert.ok(Math.abs(got - expected[name as keyof typeof expected]) <= 2, `${name} ${got} is not near ${expected[name as keyof typeof expected]}: ${line}`);
    }
    // The centre the model would click, found on the image, is inside the tree's range.
    const centre = { x: imageX(meta, 70, 480), y: imageY(meta, 62, 360) };
    assert.ok(left <= centre.x && centre.x <= right && top <= centre.y && centre.y <= bottom, `${JSON.stringify(centre)} is outside ${line}`);
  } finally {
    await closeFixture(fixture);
    await client.close();
    await app.dispose();
    await rm(dir, { recursive: true, force: true });
  }
});

test("end to end without elicitation: the helper dialog asks, and the user's allow lets the calls run", { skip: NO_DESKTOP }, async () => {
  const fixtureExe = builtExe("fixture-window.exe");
  assert.ok(fixtureExe, "fixture-window.exe is missing; build the helper tests first");
  const answerer = builtExe("prompt-answerer.exe");
  assert.ok(answerer, "prompt-answerer.exe is missing; build the helper tests first");
  const helper = builtExe("deskpass-helper.exe");
  assert.ok(helper, "deskpass-helper.exe is missing");
  const fixture = spawn(fixtureExe, [], { stdio: "ignore" });
  const dir = await mkdtemp(join(repoRoot(), "test-tmp", "e2e-"));
  const app = createServer({ lockDir: join(dir, "lock"), logDir: join(dir, "logs"), grantsDir: dir, helperPath: helper });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  // Declares no elicitation, like a client that cannot ask the user itself.
  const client = new Client({ name: "deskpass-e2e", version: "0.0.0" }, { capabilities: {} });
  await Promise.all([app.server.connect(serverTransport), client.connect(clientTransport)]);
  try {
    const windowId = await waitForFixture(client);
    // The helper is this process's child: the answerer clicks allow in its dialog like a person.
    const answered = runAnswerer(answerer, ["--parent-pid", String(process.pid), "--press", "allow", "--wait-ms", "15000"]);
    const shot = await client.callTool({ name: "computer_screenshot", arguments: { window_id: windowId } });
    const answer = await answered;
    assert.equal(answer.code, 0, answer.stderr);
    const shown = JSON.parse(answer.stdout) as { text?: string };
    assert.match(shown.text ?? "", /fixture-window\.exe/i, "the dialog does not show the fixture's path");
    assert.notEqual(shot.isError, true, textContent(shot));
    assert.match(textContent(shot), /screenshot_id: ss_/);
    const again = await client.callTool({ name: "computer_screenshot", arguments: { window_id: windowId } });
    assert.notEqual(again.isError, true, textContent(again));
  } finally {
    await closeFixture(fixture);
    await client.close();
    await app.dispose();
    await rm(dir, { recursive: true, force: true });
  }
});

test("end to end: read the tree, click a button and fill the edit by element number, read the new value", { skip: NO_DESKTOP }, async () => {
  const fixtureExe = builtExe("fixture-window.exe");
  assert.ok(fixtureExe, "fixture-window.exe is missing; build the helper tests first");
  const helper = builtExe("deskpass-helper.exe");
  assert.ok(helper, "deskpass-helper.exe is missing");
  const fixture = spawn(fixtureExe, [], { stdio: "ignore" });
  const dir = await mkdtemp(join(repoRoot(), "test-tmp", "e2e-"));
  const app = createServer({ lockDir: join(dir, "lock"), logDir: join(dir, "logs"), grantsDir: dir, helperPath: helper });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "deskpass-e2e", version: "0.0.0" }, { capabilities: { elicitation: { form: {} } } });
  client.setRequestHandler(ElicitRequestSchema, async () => ({ action: "accept" as const, content: { decision: "allow" } }));
  await Promise.all([app.server.connect(serverTransport), client.connect(clientTransport)]);
  try {
    const windowId = await waitForFixture(client);
    const tree = await client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: windowId } });
    const treeText = textContent(tree);
    assert.notEqual(tree.isError, true, treeText);
    assert.match(treeText, /tree_id t\d+/, treeText);
    const buttonIndex = treeText.match(/\[(\d+)\] button "Click me"/)?.[1];
    assert.ok(buttonIndex, `no numbered "Click me" button in the tree\n${treeText}`);
    const editLine = treeText.split("\n").find((line) => /\[\d+\] edit /.test(line) && line.includes("set_value"));
    const editIndex = editLine?.match(/\[(\d+)\] edit /)?.[1];
    assert.ok(editIndex, `no numbered edit that takes a value in the tree\n${treeText}`);

    // Ticket 140: by default the click is delivered in the background (the button's Invoke pattern); the fixture's title records `click:left`.
    const click = await client.callTool({ name: "computer_click", arguments: { window_id: windowId, element_index: Number(buttonIndex) } });
    assert.notEqual(click.isError, true, textContent(click));
    await waitForTitle(client, windowId, (title) => title.includes("click:left"), "the fixture's title never showed click:left after the element click");

    const value = "你好,世界 測試 12345";
    const set = await client.callTool({ name: "computer_set_value", arguments: { window_id: windowId, element_index: Number(editIndex), value } });
    const setText = textContent(set);
    assert.notEqual(set.isError, true, setText);
    assert.match(setText, /reads back as sent/, setText);

    const again = await client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: windowId } });
    const againText = textContent(again);
    assert.notEqual(again.isError, true, againText);
    const editNow = againText.split("\n").find((line) => /\[\d+\] edit /.test(line));
    assert.ok(editNow?.includes(`value="${value}"`), `the edit does not show the new value\n${againText}`);
    // The same value reached the app itself, not only the tree: the fixture's title carries the edit's text.
    await waitForTitle(client, windowId, (title) => title.includes(`edit=${value}`), "the fixture's title does not show the set value");
  } finally {
    await closeFixture(fixture);
    await client.close();
    await app.dispose();
    await rm(dir, { recursive: true, force: true });
  }
});

/** Polls `computer_list_windows` until the fixture's listed title satisfies `ok`, up to 5 seconds. */
async function waitForTitle(client: Client, windowId: string, ok: (title: string) => boolean, message: string): Promise<void> {
  const deadline = Date.now() + 5_000;
  let block: string | undefined;
  while (Date.now() < deadline) {
    const listed = await client.callTool({ name: "computer_list_windows", arguments: {} });
    block = textContent(listed).split(/\n\n/).find((entry) => entry.includes(`window_id: ${windowId}`));
    if (block && ok(block)) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.fail(`${message}\n${block ?? "(fixture not listed)"}`);
}

function builtExe(name: string): string | undefined {
  return [join(repoRoot(), "helper", "target", "debug", name), join(repoRoot(), "helper", "target", "release", name)].find((candidate) => existsSync(candidate));
}

function runAnswerer(exe: string, args: string[]): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(exe, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout?.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    child.stderr?.setEncoding("utf8").on("data", (chunk: string) => (stderr += chunk));
    child.on("exit", (code) => resolve({ code, stdout, stderr }));
  });
}

function imageX(text: string, windowPixel: number, designed: number): number {
  return scale(text, "width", windowPixel, designed);
}

function imageY(text: string, windowPixel: number, designed: number): number {
  return scale(text, "height", windowPixel, designed);
}

function scale(text: string, axis: "width" | "height", windowPixel: number, designed: number): number {
  const image = text.match(/image: (\d+)x(\d+)/);
  const window = text.match(/window: (\d+)x(\d+)/);
  const imageSize = Number(axis === "width" ? image?.[1] : image?.[2]);
  const windowSize = Number(axis === "width" ? window?.[1] : window?.[2]);
  if (!imageSize || !windowSize) return Math.round(windowPixel * (imageSize || designed) / designed);
  return Math.round((windowPixel * windowSize / designed) * imageSize / windowSize);
}

async function waitForFixture(client: Client): Promise<string> {
  const deadline = Date.now() + 5_000;
  let last = "";
  while (Date.now() < deadline) {
    const listed = await client.callTool({ name: "computer_list_windows", arguments: {} });
    last = textContent(listed);
    const block = last.split(/\n\n/).find((entry) => entry.includes("CUMCP_FIXTURE"));
    const id = block?.match(/window_id: (\d+)/)?.[1];
    if (id) return id;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`fixture was not listed\n${last}`);
}

async function closeFixture(fixture: ChildProcess): Promise<void> {
  if (fixture.exitCode === null && fixture.signalCode === null && fixture.pid) {
    spawn("taskkill", ["/PID", String(fixture.pid), "/T", "/F"], { stdio: "ignore" });
    fixture.kill();
  }
  await new Promise((resolve) => {
    if (fixture.exitCode !== null || fixture.signalCode !== null) resolve(undefined);
    else fixture.once("exit", () => resolve(undefined));
  });
}
