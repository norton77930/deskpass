import assert from "node:assert/strict";
import { test } from "node:test";
import { connectServer, textContent, type Harness } from "./harness.ts";

// Ticket 34: a UWP app's window belongs to ApplicationFrameHost.exe; the helper describes it by the
// app inside it (the process of its Windows.UI.Core.CoreWindow child), or marks it app_unresolved.

const FRAME_HOST = "C:\\Windows\\System32\\ApplicationFrameHost.exe";
const CALCULATOR = "C:\\Program Files\\WindowsApps\\Microsoft.WindowsCalculator_11.2405.2.0_x64__8wekyb3d8bbwe\\CalculatorApp.exe";
const SETTINGS = "C:\\Windows\\ImmersiveControlPanel\\SystemSettings.exe";

function hosted(windowId: string, app: string, appName: string, title: string) {
  return { window_id: windowId, app, app_name: appName, title, minimized: false, elevated: false, x: 0, y: 0, width: 200, height: 100, pid: 4242 };
}

const calculator = hosted("5", CALCULATOR, "CalculatorApp", "小算盤");
const settings = hosted("6", SETTINGS, "Settings", "設定");
const unresolved = { ...hosted("7", FRAME_HOST, "Application Frame Host", "小算盤"), app_unresolved: true };

const capture = {
  image_base64: "QQ==",
  original_width: 200,
  original_height: 100,
  window_x: 0,
  window_y: 0,
  window_width: 200,
  window_height: 100,
};

async function methods(harness: Harness): Promise<string[]> {
  return (await harness.requests()).map((entry) => String((entry as { method?: unknown }).method));
}

test("an app behind the frame host is asked for, and approved, as the app inside it", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { get_window: [{ result: calculator }], capture_window: [{ result: capture }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal(harness.elicitations.length, 1);
    assert.match(harness.elicitations[0] ?? "", /CalculatorApp/);
    assert.ok((harness.elicitations[0] ?? "").includes(CALCULATOR), harness.elicitations[0]);
    assert.doesNotMatch(harness.elicitations[0] ?? "", /ApplicationFrameHost|Application Frame Host/i);
  } finally {
    await harness.close();
  }
});

test("approving one app behind the frame host does not approve another one behind it", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: calculator }, { result: settings }],
        capture_window: [{ result: capture }, { result: capture }],
      },
    },
  });
  try {
    const first = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(first.isError, true, textContent(first));
    const second = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "6" } });
    assert.notEqual(second.isError, true, textContent(second));
    assert.equal(harness.elicitations.length, 2);
    assert.match(harness.elicitations[1] ?? "", /Settings/);
    assert.ok((harness.elicitations[1] ?? "").includes(SETTINGS), harness.elicitations[1]);
  } finally {
    await harness.close();
  }
});

test("the high-risk list applies to the app inside the frame host", async () => {
  const terminal = hosted("8", "C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.21.0.0_x64__8wekyb3d8bbwe\\WindowsTerminal.exe", "Terminal", "Terminal");
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: terminal }] } } });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "8" } });
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split(/\r?\n/)[0], "blocked_app");
    assert.equal(harness.elicitations.length, 0);
    assert.equal((await methods(harness)).includes("capture_window"), false);
  } finally {
    await harness.close();
  }
});

test("a window whose app the helper could not resolve is refused with app_unresolved before any prompt", async () => {
  const calls: [string, Record<string, unknown>][] = [
    ["computer_screenshot", { window_id: "7" }],
    ["computer_activate_window", { window_id: "7" }],
    ["computer_type_text", { window_id: "7", text: "7*8" }],
    ["computer_press_key", { window_id: "7", key: "Return" }],
  ];
  for (const [name, args] of calls) {
    const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: unresolved }] } } });
    try {
      const result = await harness.client.callTool({ name, arguments: args });
      assert.equal(result.isError, true, name);
      const text = textContent(result);
      assert.equal(text.split(/\r?\n/)[0], "app_unresolved", `${name}: ${text}`);
      assert.match(text, /computer_list_windows/);
      assert.equal(harness.elicitations.length, 0, name);
      const sent = await methods(harness);
      for (const method of ["ask_user", "session_start", "activate_window", "capture_window", "type_text", "press_key"]) {
        assert.equal(sent.includes(method), false, `${name} sent ${method}`);
      }
    } finally {
      await harness.close();
    }
  }
});

test("the helper's app_unresolved refusal of get_window reaches the model without a prompt", async () => {
  const message =
    "The app inside this window could not be identified (it may still be starting or be suspended), so the window cannot be operated and no input was sent. Wait a moment, call computer_list_windows again, then retry.";
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { get_window: [{ error: { code: "app_unresolved", message } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "7", text: "7*8" } });
    assert.equal(result.isError, true);
    assert.equal(textContent(result), `app_unresolved\n${message}`);
    assert.equal(harness.elicitations.length, 0);
    assert.equal((await methods(harness)).includes("type_text"), false);
  } finally {
    await harness.close();
  }
});

test("computer_list_windows marks a window whose app could not be resolved", async () => {
  const listed = (window: Record<string, unknown>) => {
    const { x: _x, y: _y, width: _w, height: _h, pid: _pid, ...rest } = window;
    return rest;
  };
  const harness = await connectServer({
    script: { handlers: { list_windows: [{ result: { windows: [listed(calculator), listed(unresolved)] } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(result.isError, true);
    const [first, second] = textContent(result).split(/\n\n/);
    assert.ok(first?.includes(`app: ${CALCULATOR}`), first);
    assert.doesNotMatch(first ?? "", /app_unresolved/);
    assert.ok(second?.includes(`app: ${FRAME_HOST}`), second);
    assert.match(second ?? "", /^app_unresolved: true$/m);
  } finally {
    await harness.close();
  }
});

// Ticket 44: Windows does not let the helper follow a UWP app's input queue, so its keys are sent
// without confirming the app read each one, and the helper says so with read_confirmed: false.
test("keys the app could not be confirmed to read succeed and ask for a screenshot to check", async () => {
  const unconfirmed =
    "The keys were sent to CalculatorApp, but this app does not let Deskpass confirm it read each key; take a screenshot to check the result before continuing.";
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: calculator }, { result: calculator }],
        activate_window: [{ result: {} }, { result: {} }],
        type_text: [{ result: { read_confirmed: false } }],
        press_key: [{ result: { read_confirmed: false } }],
      },
    },
  });
  try {
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "7*8" } });
    assert.notEqual(typed.isError, true, textContent(typed));
    assert.equal(textContent(typed), `Typed text into the target window. ${unconfirmed}`);
    const pressed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(pressed.isError, true, textContent(pressed));
    assert.equal(textContent(pressed), `Pressed return. ${unconfirmed}`);
  } finally {
    await harness.close();
  }
});

// Ticket 44: when the app did not take the English keyboard layout, the helper typed on the app's
// own layout and says so with layout_switched: false; the message warns about the input method.
test("text typed without the English layout warns that an input method may have held it", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: calculator }],
        activate_window: [{ result: {} }],
        type_text: [{ result: { read_confirmed: false, layout_switched: false } }],
      },
    },
  });
  try {
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "7*8" } });
    assert.notEqual(typed.isError, true, textContent(typed));
    assert.equal(
      textContent(typed),
      "Typed text into the target window. The keys were sent to CalculatorApp, but this app does not let Deskpass confirm it read each key; take a screenshot to check the result before continuing. " +
        "The app did not switch to the English keyboard layout, so an input method may have composed or held the typed characters; check the screenshot for missing or converted text.",
    );
  } finally {
    await harness.close();
  }
});
