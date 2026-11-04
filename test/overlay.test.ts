import assert from "node:assert/strict";
import { test } from "node:test";
import { connectServer, textContent } from "./harness.ts";

const openWindow = {
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

test("an approved action shows the overlay on that window", async () => {
  const harness = await connectServer({
    elicitation: "form",
    overlayIdleMs: 60_000,
    script: {
      handlers: {
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
      },
    },
  });
  try {
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(shot.isError, true);
    const requests = await harness.requests();
    const shown = requests.find((entry) => (entry as { method?: string }).method === "overlay_show") as { params?: { window_id?: string } } | undefined;
    assert.equal(shown?.params?.window_id, "5");
    assert.match(textContent(shot), /screenshot_id: ss_1/);
  } finally {
    await harness.close();
  }
});

test("switching windows updates the overlay target", async () => {
  const other = { ...openWindow, window_id: "7", app: "C:\\Apps\\other.exe", app_name: "Other" };
  const harness = await connectServer({
    elicitation: "form",
    overlayIdleMs: 60_000,
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: other }],
        capture_window: [{ result: captured }, { result: captured }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "7" } });
    const shows = (await harness.requests())
      .filter((entry) => (entry as { method?: string }).method === "overlay_show")
      .map((entry) => (entry as { params?: { window_id?: string } }).params?.window_id);
    assert.deepEqual(shows, ["5", "7"]);
  } finally {
    await harness.close();
  }
});

test("the overlay hides after the idle interval", async () => {
  const harness = await connectServer({
    elicitation: "form",
    overlayIdleMs: 40,
    script: {
      handlers: {
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const before = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "overlay_hide");
    assert.equal(before.length, 0);
    await new Promise((resolve) => setTimeout(resolve, 150));
    const after = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "overlay_hide");
    assert.equal(after.length, 1);
  } finally {
    await harness.close();
  }
});

test("closing the session hides the overlay", async () => {
  const harness = await connectServer({
    elicitation: "form",
    overlayIdleMs: 60_000,
    script: {
      handlers: {
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    await harness.finish();
    const methods = (await harness.requests()).map((entry) => (entry as { method?: string }).method);
    assert.equal(methods.filter((method) => method === "overlay_hide").length, 1);
    assert.equal(methods.includes("session_end"), true);
  } finally {
    await harness.close();
  }
});

test("Esc hides the overlay", async () => {
  const harness = await connectServer({
    elicitation: "form",
    overlayIdleMs: 60_000,
    script: {
      handlers: {
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
        list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const stopped = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(textContent(stopped).split(/\r?\n/)[0], "interrupted");
    let hides = 0;
    for (let attempt = 0; attempt < 20 && hides === 0; attempt += 1) {
      hides = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "overlay_hide").length;
      if (hides === 0) await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert.equal(hides, 1);
  } finally {
    await harness.close();
  }
});

type Shown = { method?: string; params?: { window_id?: string; app_name?: string; delivery_mode?: string } };

const treeElements = [
  { depth: 0, index: 1, role: "button", name: "OK", bounds: { x: 10, y: 10, width: 60, height: 20 }, states: [], actions: ["click", "invoke"] },
  { depth: 0, index: 2, role: "edit", name: "Search", value: "", bounds: { x: 10, y: 40, width: 100, height: 20 }, states: [], actions: ["click", "set_value"] },
];

test("ticket 144: the overlay request names the approved app and the delivery mode, for the corner notice's text", async () => {
  const harness = await connectServer({
    elicitation: "form",
    overlayIdleMs: 60_000,
    script: {
      handlers: {
        get_window: Array.from({ length: 20 }, () => ({ result: openWindow })),
        capture_window: [{ result: captured }],
        get_ui_tree: [{ result: { tree_id: "t1-aaaaa", window_title: "Demo", elements: treeElements } }],
        click: [{ result: { action: "invoke" } }, { result: {} }, { result: { action: "invoke" } }, { result: {} }, { result: { action: "invoke" } }, { result: {} }],
        element_action: [{ result: {} }, { result: {} }],
        set_value: [{ result: { value: "hi" } }, { result: { value: "hi" } }],
      },
    },
  });
  try {
    const calls: [string, Record<string, unknown>][] = [
      ["computer_screenshot", { window_id: "5" }],
      ["computer_click", { window_id: "5", x: 5, y: 5 }],
      ["computer_click", { window_id: "5", x: 5, y: 5, delivery_mode: "foreground" }],
      ["computer_get_ui_tree", { window_id: "5" }],
      ["computer_click", { window_id: "5", element_index: 1 }],
      ["computer_click", { window_id: "5", element_index: 1, delivery_mode: "foreground" }],
      ["computer_element_action", { window_id: "5", element_index: 1, action: "invoke" }],
      ["computer_element_action", { window_id: "5", element_index: 1, action: "invoke", delivery_mode: "foreground" }],
      ["computer_set_value", { window_id: "5", element_index: 2, value: "hi" }],
      ["computer_set_value", { window_id: "5", element_index: 2, value: "hi", delivery_mode: "foreground" }],
      ["computer_type_text", { window_id: "5", text: "a" }],
    ];
    for (const [name, args] of calls) {
      const result = await harness.client.callTool({ name, arguments: args });
      assert.notEqual(result.isError, true, `${name}: ${textContent(result)}`);
    }
    const batch = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "click", window_id: "5", x: 5, y: 5 }, { action: "click", window_id: "5", x: 5, y: 5, delivery_mode: "foreground" }] },
    });
    assert.notEqual(batch.isError, true, textContent(batch));
    const shows = ((await harness.requests()) as Shown[]).filter((entry) => entry.method === "overlay_show").map((entry) => entry.params);
    assert.deepEqual(
      shows.map((params) => params?.delivery_mode),
      [
        // A read with no overlay showing: only Ctrl+Alt+Esc stops at any time.
        "background",
        "background",
        "foreground",
        // A read keeps the stop key of the action whose border is still showing.
        "foreground",
        "background",
        "foreground",
        "background",
        "foreground",
        "background",
        "foreground",
        "foreground",
        "background",
        "foreground",
      ],
    );
    for (const params of shows) {
      assert.equal(params?.window_id, "5");
      assert.equal(params?.app_name, "Demo");
    }
  } finally {
    await harness.close();
  }
});
