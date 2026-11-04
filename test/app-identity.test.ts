import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { connectServer, repoRoot, textContent } from "./harness.ts";

// Ticket 55 (F8): the helper and the server refuse an unidentifiable frame with one text. The
// server's copy only covers a window description that did not come from `get_window`.
test("the app_unresolved refusal reads the same from the helper and the server", () => {
  const helper = readFileSync(path.join(repoRoot(), "helper", "src", "windows_list.rs"), "utf8");
  const server = readFileSync(path.join(repoRoot(), "src", "server.ts"), "utf8");
  const fromHelper = /"app_unresolved"\.to_string\(\),\s*"([^"]+)"\.to_string\(\)/.exec(helper)?.[1];
  const fromServer = /const APP_UNRESOLVED_MESSAGE =\s*"([^"]+)";/.exec(server)?.[1];
  assert.ok(fromHelper && fromServer, "both texts are found");
  assert.equal(fromServer, fromHelper);
});

// Ticket 55 (F1): the app a call was approved for travels with every request that acts on the
// window, and a window that turned into another app while the question was open is asked about
// (or refused) again rather than operated as the approved one.

const demo = {
  window_id: "5",
  app: "C:\\Apps\\demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 200,
  height: 100,
  pid: 4242,
};

const other = { ...demo, app: "C:\\Apps\\other.exe", app_name: "Other", pid: 4343 };
const third = { ...demo, app: "C:\\Apps\\third.exe", app_name: "Third", pid: 4444 };

const shot = {
  image_base64: "QQ==",
  original_width: 200,
  original_height: 100,
  window_x: 0,
  window_y: 0,
  window_width: 200,
  window_height: 100,
};

type Logged = { method?: string; params?: Record<string, unknown> };

test("every request that acts on the window names the app it was approved for", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }, { result: demo }, { result: demo }, { result: demo }],
        capture_window: [{ result: shot }],
      },
    },
  });
  try {
    const calls = [
      { name: "computer_screenshot", arguments: { window_id: "5" } },
      { name: "computer_click", arguments: { window_id: "5", x: 10, y: 10, delivery_mode: "foreground" } },
      { name: "computer_type_text", arguments: { window_id: "5", text: "a" } },
      { name: "computer_press_key", arguments: { window_id: "5", key: "enter" } },
      { name: "computer_activate_window", arguments: { window_id: "5" } },
    ];
    for (const call of calls) {
      const result = await harness.client.callTool(call);
      assert.notEqual(result.isError, true, `${call.name}: ${textContent(result)}`);
    }
    const acting = ((await harness.requests()) as Logged[]).filter((entry) =>
      ["capture_window", "activate_window", "click", "type_text", "press_key"].includes(entry.method ?? ""),
    );
    assert.equal(acting.length, 8);
    for (const entry of acting) assert.equal(entry.params?.expected_app, demo.app, `${entry.method} names the approved app`);
  } finally {
    await harness.close();
  }
});

test("a window that turned into another app while the question was open is asked about again, and a decline sends nothing", async () => {
  let asked = 0;
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => {
      asked += 1;
      return asked < 3 ? { action: "accept", content: { decision: "allow" } } : { action: "decline" };
    },
    script: {
      handlers: {
        // The screenshot approves Demo; the click finds Other (asked), and after that question
        // the window is Third, which is asked about too and declined.
        get_window: [{ result: demo }, { result: other }, { result: third }],
        capture_window: [{ result: shot }],
      },
    },
  });
  try {
    assert.notEqual((await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } })).isError, true);
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 10, y: 10 } });
    assert.equal(clicked.isError, true);
    assert.equal(textContent(clicked).split(/\r?\n/)[0], "approval_declined");
    assert.equal(harness.elicitations.length, 3);
    assert.match(harness.elicitations[2] ?? "", /third\.exe/i);
    const methods = ((await harness.requests()) as Logged[]).map((entry) => entry.method);
    assert.equal(methods.includes("click"), false);
  } finally {
    await harness.close();
  }
});

test("the helper's app_changed reaches the model as it is", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }],
        type_text: [{ error: { code: "app_changed", message: "The window now belongs to Other (C:\\Apps\\other.exe), not to Demo (C:\\Apps\\demo.exe), which this call was approved for, so nothing was sent." } }],
      },
    },
  });
  try {
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "a" } });
    assert.equal(typed.isError, true);
    const text = textContent(typed);
    assert.equal(text.split(/\r?\n/)[0], "app_changed");
    assert.match(text, /nothing was sent/);
  } finally {
    await harness.close();
  }
});
