import assert from "node:assert/strict";
import { test } from "node:test";
import { connectServer, textContent } from "./harness.ts";

const owner = {
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

const dialog = { ...owner, window_id: "9", title: "Save As", owner_window_id: "5", x: 40, y: 20, width: 120, height: 60 };

function captured() {
  return {
    image_base64: "QQ==",
    original_width: 200,
    original_height: 100,
    window_x: 0,
    window_y: 0,
    window_width: 200,
    window_height: 100,
  };
}

function methods(requests: unknown[]): string[] {
  return requests.map((entry) => String((entry as { method?: string }).method));
}

test("computer_list_windows shows owner_window_id for an owned dialog only", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [
          {
            result: {
              windows: [
                { window_id: "5", app: owner.app, app_name: "Demo", title: "Demo", minimized: false, elevated: false },
                { window_id: "9", app: owner.app, app_name: "Demo", title: "Save As", minimized: false, elevated: false, owner_window_id: "5" },
              ],
            },
          },
        ],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(result.isError, true);
    const [ownerBlock, dialogBlock] = textContent(result).split(/\r?\n\r?\n/);
    assert.match(dialogBlock ?? "", /window_id: 9/);
    assert.match(dialogBlock ?? "", /title: Save As/);
    assert.match(dialogBlock ?? "", /owner_window_id: 5/);
    assert.match(ownerBlock ?? "", /window_id: 5/);
    assert.doesNotMatch(ownerBlock ?? "", /owner_window_id/);
  } finally {
    await harness.close();
  }
});

test("input to a window whose dialog holds the foreground is dialog_open naming the dialog", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: owner }],
        activate_window: [{ result: {} }],
        press_key: [
          {
            error: {
              code: "dialog_open",
              message: "The target window has an open dialog, so no input was sent.",
              data: { window_id: "9", title: "Save As" },
            },
          },
        ],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "ctrl+s" } });
    assert.equal(result.isError, true);
    const [code, ...rest] = textContent(result).split(/\r?\n/);
    const message = rest.join("\n");
    assert.equal(code, "dialog_open");
    assert.match(message, /window_id:? 9\b/);
    assert.match(message, /Save As/);
    assert.match(message, /no input was sent/i);
    assert.match(message, /window_id 9 as the target/i);
  } finally {
    await harness.close();
  }
});

test("a dialog inherits its app's approval, so operating it does not ask again", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: owner }, { result: dialog }, { result: dialog }],
        capture_window: [{ result: captured() }, { result: { ...captured(), original_width: 120, original_height: 60, window_width: 120, window_height: 60 } }],
        activate_window: [{ result: {} }],
        press_key: [{ result: {} }],
      },
    },
  });
  try {
    const first = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(first.isError, true);
    assert.equal(harness.elicitations.length, 1);
    const pictured = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "9" } });
    assert.notEqual(pictured.isError, true, textContent(pictured));
    const pressed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "9", key: "Return" } });
    assert.notEqual(pressed.isError, true, textContent(pressed));
    assert.equal(harness.elicitations.length, 1);
    const requests = await harness.requests();
    const press = requests.find((entry) => (entry as { method?: string }).method === "press_key") as { params?: { window_id?: string } };
    assert.equal(press.params?.window_id, "9");
    assert.deepEqual(methods(requests).filter((method) => method === "activate_window"), ["activate_window"]);
  } finally {
    await harness.close();
  }
});
