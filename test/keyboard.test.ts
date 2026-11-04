import assert from "node:assert/strict";
import { test } from "node:test";
import { connectServer, textContent } from "./harness.ts";

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

test("an unknown key is invalid_params and lists usable names", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { get_window: [{ result: demo }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "ctrl+nope" } });
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "invalid_params");
    assert.match(text, /return/);
    assert.match(text, /f1/);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "press_key"), false);
  } finally {
    await harness.close();
  }
});

test("type_text that stops at the time limit or where the text closed the window says what was typed", async () => {
  const limit = "Typed 772 of 3000 characters before the 20-second limit for one call; the remaining 2228 were not sent. Send them with another type_text call.";
  const closed = "The target window closed after character 3 of 5; the remaining 2 were not sent.";
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        activate_window: [{ result: {} }, { result: {} }],
        type_text: [{ error: { code: "input_failed", message: limit } }, { error: { code: "input_failed", message: closed } }],
      },
    },
  });
  try {
    const tools = await harness.client.listTools();
    const description = tools.tools.find((tool) => tool.name === "computer_type_text")?.description ?? "";
    assert.match(description, /20 seconds/);
    assert.match(description, /another call/);
    const long = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "long" } });
    assert.equal(long.isError, true);
    assert.equal(textContent(long), `input_failed\n${limit}`);
    const short = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "ab\ncd" } });
    assert.equal(short.isError, true);
    assert.equal(textContent(short), `input_failed\n${closed}`);
  } finally {
    await harness.close();
  }
});

test("a key press or text after which the foreground moved succeeds and names the new foreground", async () => {
  const guest = { window_id: "33883792", title: "Guest", app: "C:\\Apps\\demo.exe", app_name: "Demo" };
  const meet = { window_id: "77", title: "Call with Ada", app: "C:\\Apps\\meet.exe", app_name: "Meet" };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        activate_window: [{ result: {} }, { result: {} }],
        press_key: [{ result: { foreground_changed: { foreground: guest, target_closed: false } } }],
        type_text: [{ result: { foreground_changed: { foreground: meet, target_closed: true } } }],
      },
    },
  });
  try {
    const pressed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "ctrl+n" } });
    assert.notEqual(pressed.isError, true);
    assert.equal(
      textContent(pressed),
      'Pressed ctrl+n. After the key press, the foreground moved to another window of Demo (window_id: 33883792, title: "Guest"). Take a screenshot of window_id 33883792 before continuing.',
    );
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "ok\n" } });
    assert.notEqual(typed.isError, true);
    const text = textContent(typed);
    assert.match(text, /^Typed text into the target window\. The target window closed after the text input; the foreground is now a window of Meet \(C:\\Apps\\meet\.exe\) \(window_id: 77, title: "Call with Ada"\), a different app\./);
    assert.match(text, /The text input went to the target window of Demo, not to Meet\./);
  } finally {
    await harness.close();
  }
});

test("the screenshot and keyboard tools say to wait and look again before repeating input", async () => {
  const harness = await connectServer({ elicitation: "form", script: { handlers: {} } });
  try {
    const tools = (await harness.client.listTools()).tools;
    for (const name of ["computer_screenshot", "computer_type_text", "computer_press_key"]) {
      const description = tools.find((tool) => tool.name === name)?.description ?? "";
      assert.match(description, /a moment later/, name);
      assert.match(description, /wait about 1 second/, name);
      assert.match(description, /before repeating/, name);
    }
  } finally {
    await harness.close();
  }
});

test("key chords are normalized and both keyboard tools require approval", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }, { result: demo }],
        activate_window: [{ result: {} }, { result: {} }, { result: {} }],
        press_key: [{ result: {} }, { result: {} }],
        type_text: [{ result: {} }],
      },
    },
  });
  try {
    const chord = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "shift+Tab" } });
    assert.notEqual(chord.isError, true);
    assert.equal(harness.elicitations.length, 1);
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "你好\n" } });
    assert.notEqual(typed.isError, true);
    assert.equal(harness.elicitations.length, 1);
    const requests = await harness.requests();
    const press = requests.find((entry) => (entry as { method?: string }).method === "press_key") as { params?: { keys?: string[] } };
    assert.deepEqual(press.params?.keys, ["shift", "tab"]);
    const text = requests.find((entry) => (entry as { method?: string }).method === "type_text") as { params?: { text?: string } };
    assert.equal(text.params?.text, "你好\n");
  } finally {
    await harness.close();
  }
});
