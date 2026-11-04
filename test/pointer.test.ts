import assert from "node:assert/strict";
import { test } from "node:test";
import { connectServer, textContent, type Harness } from "./harness.ts";

const demo = {
  window_id: "5",
  app: "C:\\Apps\\demo.exe",
  app_name: "Demo",
  title: "Demo",
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 400,
  height: 300,
  pid: 4242,
};

const picture = {
  image_base64: "QQ==",
  original_width: 400,
  original_height: 300,
  window_x: 0,
  window_y: 0,
  window_width: 400,
  window_height: 300,
};

async function prepare(harness: Harness) {
  const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
  assert.notEqual(shot.isError, true);
  return textContent(shot);
}

test("zoom crops in original pixels and does not become the click coordinate base", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 4 }, () => ({ result: demo })),
        capture_window: [{ result: picture }, { result: { ...picture, image_base64: "QQ==" } }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    const first = await prepare(harness);
    const zoom = await harness.client.callTool({
      name: "computer_zoom",
      arguments: { window_id: "5", region: [10, 20, 110, 80] },
    });
    assert.notEqual(zoom.isError, true);
    const text = textContent(zoom);
    assert.match(text, /region:/);
    assert.match(text, /regular screenshot/i);
    const zoomId = /screenshot_id: (\S+)/.exec(text)?.[1];
    const requests = await harness.requests();
    const crops = requests.filter((entry) => (entry as { method?: string }).method === "capture_window") as Array<{ params?: { crop?: { x: number; y: number; width: number; height: number } } }>;
    assert.deepEqual(crops.at(-1)?.params?.crop, { x: 10, y: 20, width: 100, height: 60 });
    const outside = await harness.client.callTool({
      name: "computer_zoom",
      arguments: { window_id: "5", region: [0, 0, 500, 10] },
    });
    assert.equal(textContent(outside).split(/\r?\n/)[0], "out_of_bounds");
    const clicked = await harness.client.callTool({
      name: "computer_click",
      arguments: { window_id: "5", x: 1, y: 1, screenshot_id: zoomId },
    });
    assert.equal(textContent(clicked).split(/\r?\n/)[0], "no_screenshot");
    assert.match(first, /screenshot_id: ss_1/);
  } finally {
    await harness.close();
  }
});

test("zoom past the captured frame returns out_of_bounds instead of the whole window", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 3 }, () => ({ result: demo })),
        capture_window: [
          { result: picture },
          { error: { code: "out_of_bounds", message: "The zoom region is outside the window capture. Take a new screenshot." } },
        ],
      },
    },
  });
  try {
    await prepare(harness);
    const zoom = await harness.client.callTool({
      name: "computer_zoom",
      arguments: { window_id: "5", region: [300, 200, 400, 300] },
    });
    assert.equal(zoom.isError, true);
    assert.equal(textContent(zoom).split(/\r?\n/)[0], "out_of_bounds");
    const content = zoom.content as Array<{ type: string }>;
    assert.equal(content.some((block) => block.type === "image"), false);
    const crops = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "capture_window") as Array<{ params?: { crop?: unknown } }>;
    assert.deepEqual(crops.at(-1)?.params?.crop, { x: 300, y: 200, width: 100, height: 100 });
  } finally {
    await harness.close();
  }
});

test("wait accepts 0.1 and 10 seconds and rejects values outside that range", async () => {
  const harness = await connectServer();
  try {
    const short = await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 0.1 } });
    assert.notEqual(short.isError, true);
    const full = await harness.client.callTool({ name: "computer_wait", arguments: { seconds: 10 } });
    assert.notEqual(full.isError, true);
    for (const seconds of [0, 11]) {
      const result = await harness.client.callTool({ name: "computer_wait", arguments: { seconds } });
      assert.equal(textContent(result).split(/\r?\n/)[0], "invalid_params");
    }
    assert.equal(harness.elicitations.length, 0);
  } finally {
    await harness.close();
  }
});

test("scroll, drag, and move convert coordinates and keep approval", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 5 }, () => ({ result: demo })),
        capture_window: [{ result: picture }],
        activate_window: Array.from({ length: 4 }, () => ({ result: {} })),
        scroll: [{ result: {} }],
        drag: [{ result: {} }],
        move_mouse: [{ result: {} }],
      },
    },
  });
  try {
    await prepare(harness);
    const scrolled = await harness.client.callTool({
      name: "computer_scroll",
      arguments: { window_id: "5", x: 20, y: 30, direction: "down" },
    });
    assert.notEqual(scrolled.isError, true);
    const dragged = await harness.client.callTool({
      name: "computer_drag",
      arguments: { window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50 },
    });
    assert.notEqual(dragged.isError, true);
    const moved = await harness.client.callTool({
      name: "computer_move_mouse",
      arguments: { window_id: "5", x: 15, y: 25 },
    });
    assert.notEqual(moved.isError, true);
    assert.equal(harness.elicitations.length, 1);
    const requests = await harness.requests();
    const scroll = requests.find((entry) => (entry as { method?: string }).method === "scroll") as { params?: { direction?: string; amount?: number; x?: number } };
    assert.equal(scroll.params?.direction, "down");
    assert.equal(scroll.params?.amount, 3);
    assert.equal(scroll.params?.x, 20);
    const drag = requests.find((entry) => (entry as { method?: string }).method === "drag") as { params?: { to_x?: number } };
    assert.equal(drag.params?.to_x, 40);
  } finally {
    await harness.close();
  }
});

test("scroll and drag after which the foreground moved succeed and name the new foreground", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 3 }, () => ({ result: demo })),
        capture_window: [{ result: picture }],
        activate_window: Array.from({ length: 2 }, () => ({ result: {} })),
        scroll: [{ result: { foreground_changed: { foreground: null, target_closed: false } } }],
        drag: [{ result: { foreground_changed: { foreground: { window_id: "9", title: "Copy", app: "C:\\Apps\\demo.exe", app_name: "Demo" }, target_closed: false } } }],
      },
    },
  });
  try {
    await prepare(harness);
    const scrolled = await harness.client.callTool({ name: "computer_scroll", arguments: { window_id: "5", x: 20, y: 30, direction: "down" } });
    assert.notEqual(scrolled.isError, true);
    assert.equal(
      textContent(scrolled),
      "Scrolled down by 3. After the scroll, the target window no longer has the foreground, and no window that computer_list_windows lists has it. Call computer_list_windows to see which windows are open before continuing.",
    );
    const dragged = await harness.client.callTool({ name: "computer_drag", arguments: { window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50 } });
    assert.notEqual(dragged.isError, true);
    assert.match(textContent(dragged), /^Dragged from .*\. After the drag, the foreground moved to another window of Demo \(window_id: 9, title: "Copy"\)\. Take a screenshot of window_id 9 before continuing\.$/);
  } finally {
    await harness.close();
  }
});

test("a pointer move after which the foreground moved succeeds and names the new foreground", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 2 }, () => ({ result: demo })),
        capture_window: [{ result: picture }],
        activate_window: [{ result: {} }],
        move_mouse: [{ result: { foreground_changed: { foreground: { window_id: "9", title: "Menu", app: "C:\\Apps\\demo.exe", app_name: "Demo" }, target_closed: false } } }],
      },
    },
  });
  try {
    await prepare(harness);
    const moved = await harness.client.callTool({ name: "computer_move_mouse", arguments: { window_id: "5", x: 15, y: 25 } });
    assert.notEqual(moved.isError, true);
    assert.equal(
      textContent(moved),
      'Moved the pointer to (15, 25). After the pointer move, the foreground moved to another window of Demo (window_id: 9, title: "Menu"). Take a screenshot of window_id 9 before continuing.',
    );
  } finally {
    await harness.close();
  }
});

test("scroll amount is a whole number of notches from 1 to 50", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 7 }, () => ({ result: demo })),
        capture_window: [{ result: picture }],
        scroll: [{ result: {} }],
      },
    },
  });
  try {
    const listed = await harness.client.listTools();
    const schema = listed.tools.find((tool) => tool.name === "computer_scroll")?.inputSchema as { properties?: Record<string, { type?: string; minimum?: number; maximum?: number }> };
    assert.equal(schema.properties?.amount?.type, "integer");
    assert.equal(schema.properties?.amount?.minimum, 1);
    assert.equal(schema.properties?.amount?.maximum, 50);
    await prepare(harness);
    for (const amount of [2.5, 0, -1, 51, "3"]) {
      const result = await harness.client.callTool({ name: "computer_scroll", arguments: { window_id: "5", x: 20, y: 30, direction: "down", amount } });
      assert.equal(result.isError, true, `amount ${amount} was accepted`);
      assert.equal(textContent(result).split(/\r?\n/)[0], "invalid_params", `amount ${amount}`);
      assert.match(textContent(result), /whole number from 1 to 50/);
    }
    const methods = (await harness.requests()).map((entry) => (entry as { method?: string }).method);
    assert.equal(methods.includes("scroll"), false, "an invalid amount reached the helper");
    const fifty = await harness.client.callTool({ name: "computer_scroll", arguments: { window_id: "5", x: 20, y: 30, direction: "up", amount: 50 } });
    assert.notEqual(fifty.isError, true, textContent(fifty));
    const scroll = (await harness.requests()).find((entry) => (entry as { method?: string }).method === "scroll") as { params?: { amount?: number } };
    assert.equal(scroll.params?.amount, 50);
  } finally {
    await harness.close();
  }
});

// Acceptance attempt 3 (Paint): ss_3 was taken maximized, then the window was restored to a smaller size.
const maximized = { ...demo, x: -8, y: -8, width: 2560, height: 1389 };
const maximizedPicture = {
  image_base64: "QQ==",
  image_width: 1498,
  image_height: 813,
  original_width: 2560,
  original_height: 1389,
  window_x: -8,
  window_y: -8,
  window_width: 2560,
  window_height: 1389,
};
const INPUT_METHODS = new Set(["activate_window", "click", "drag", "scroll", "move_mouse", "overlay_show"]);

test("coordinate tools refuse a screenshot taken before the window was resized and send nothing", async () => {
  const restored = { ...maximized, x: 100, y: 80, width: 1520, height: 856 };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: maximized }, ...Array.from({ length: 5 }, () => ({ result: restored }))],
        capture_window: [{ result: maximizedPicture }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const calls = [
      { name: "computer_click", arguments: { window_id: "5", x: 300, y: 400 } },
      { name: "computer_drag", arguments: { window_id: "5", from_x: 100, from_y: 250, to_x: 400, to_y: 330, screenshot_id: "ss_1" } },
      { name: "computer_scroll", arguments: { window_id: "5", x: 300, y: 400, direction: "down" } },
      { name: "computer_move_mouse", arguments: { window_id: "5", x: 300, y: 400, screenshot_id: "ss_1" } },
      { name: "computer_zoom", arguments: { window_id: "5", region: [100, 100, 300, 300] } },
    ];
    for (const call of calls) {
      const result = await harness.client.callTool(call);
      assert.equal(result.isError, true, `${call.name} was sent: ${textContent(result)}`);
      const text = textContent(result);
      assert.equal(text.split(/\r?\n/)[0], "stale_screenshot", call.name);
      assert.match(text, /2560x1389/);
      assert.match(text, /1520x856/);
      assert.match(text, /computer_screenshot/);
    }
    const methods = (await harness.requests()).map((entry) => (entry as { method?: string }).method ?? "");
    // The screenshot itself shows the overlay (before its capture); nothing after it may.
    const afterScreenshot = methods.slice(methods.indexOf("capture_window") + 1);
    assert.deepEqual(afterScreenshot.filter((method) => INPUT_METHODS.has(method)), []);
    assert.equal(methods.filter((method) => method === "capture_window").length, 1);
  } finally {
    await harness.close();
  }
});

test("a minimized window is restored and clicked as before: its iconic size is not a resize", async () => {
  const minimized = { ...maximized, x: -32000, y: -32000, width: 160, height: 28, minimized: true };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: maximized }, { result: minimized }],
        capture_window: [{ result: maximizedPicture }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 749, y: 406, delivery_mode: "foreground" } });
    assert.notEqual(clicked.isError, true, textContent(clicked));
    const requests = (await harness.requests()) as Array<{ method?: string; params?: Record<string, unknown> }>;
    const methods = requests.map((entry) => entry.method ?? "");
    assert.ok(methods.indexOf("activate_window") >= 0 && methods.indexOf("activate_window") < methods.indexOf("click"), methods.join(","));
    const click = requests.find((entry) => entry.method === "click");
    assert.equal(click?.params?.x, 1280);
    assert.equal(click?.params?.y, 694);
  } finally {
    await harness.close();
  }
});

test("a window that only moved keeps its screenshot: coordinates stay window-relative", async () => {
  const moved = { ...maximized, x: 300, y: 200 };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: maximized }, { result: moved }, { result: moved }],
        capture_window: [{ result: maximizedPicture }],
        click: [{ result: {} }],
        drag: [{ result: {} }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 749, y: 406 } });
    assert.notEqual(clicked.isError, true, textContent(clicked));
    const dragged = await harness.client.callTool({
      name: "computer_drag",
      arguments: { window_id: "5", from_x: 0, from_y: 0, to_x: 749, to_y: 406, screenshot_id: "ss_1" },
    });
    assert.notEqual(dragged.isError, true, textContent(dragged));
    const requests = (await harness.requests()) as Array<{ method?: string; params?: Record<string, unknown> }>;
    const click = requests.find((entry) => entry.method === "click");
    assert.equal(click?.params?.x, 1280);
    assert.equal(click?.params?.y, 694);
    const drag = requests.find((entry) => entry.method === "drag");
    assert.deepEqual([drag?.params?.from_x, drag?.params?.to_x, drag?.params?.to_y], [0, 1280, 694]);
  } finally {
    await harness.close();
  }
});

// Ticket 45 (F2): a coordinate call can still raise the approval question, e.g. when the window now
// reports another app than the screenshot did. The size compared is the one after that question.
const STALE_CALLS = [
  { name: "computer_click", arguments: { window_id: "5", x: 300, y: 400 } },
  { name: "computer_drag", arguments: { window_id: "5", from_x: 100, from_y: 250, to_x: 400, to_y: 330, screenshot_id: "ss_1" } },
  { name: "computer_scroll", arguments: { window_id: "5", x: 300, y: 400, direction: "down" } },
  { name: "computer_move_mouse", arguments: { window_id: "5", x: 300, y: 400, screenshot_id: "ss_1" } },
  { name: "computer_zoom", arguments: { window_id: "5", region: [100, 100, 300, 300] } },
];

test("a window resized while the approval question is open is caught as a stale screenshot", async () => {
  const asked = STALE_CALLS.map((_, index) => ({ ...maximized, app: `C:\\Apps\\other${index}.exe`, app_name: `Other${index}` }));
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        // Per call: the size before the question, then the size once it was answered.
        get_window: [{ result: maximized }, ...asked.flatMap((before) => [{ result: before }, { result: { ...before, x: 100, y: 80, width: 1520, height: 856 } }])],
        capture_window: [{ result: maximizedPicture }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    for (const call of STALE_CALLS) {
      const result = await harness.client.callTool(call);
      const text = textContent(result);
      assert.equal(text.split(/\r?\n/)[0], "stale_screenshot", `${call.name}: ${text}`);
      assert.match(text, /is now 1520x856/);
    }
    assert.equal(harness.elicitations.length, 1 + STALE_CALLS.length);
    const methods = (await harness.requests()).map((entry) => (entry as { method?: string }).method ?? "");
    const afterScreenshot = methods.slice(methods.indexOf("capture_window") + 1);
    assert.deepEqual(afterScreenshot.filter((method) => INPUT_METHODS.has(method)), []);
    assert.equal(methods.filter((method) => method === "capture_window").length, 1);
  } finally {
    await harness.close();
  }
});

test("a declined approval on a coordinate call asks the helper nothing more", async () => {
  const other = { ...maximized, app: "C:\\Apps\\other.exe", app_name: "Other" };
  const harness = await connectServer({
    elicitation: "form",
    onElicit: (message) => (message.includes("Other") ? { action: "decline" } : { action: "accept" }),
    script: {
      handlers: {
        get_window: [{ result: maximized }, ...STALE_CALLS.map(() => ({ result: other }))],
        capture_window: [{ result: maximizedPicture }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const before = (await harness.requests()).length;
    // Ticket 90: the first decline is remembered, so the calls after it are refused without asking.
    for (const [index, call] of STALE_CALLS.entries()) {
      const result = await harness.client.callTool(call);
      assert.equal(textContent(result).split(/\r?\n/)[0], index === 0 ? "approval_declined" : "approval_refused_earlier", call.name);
    }
    // Ticket 93: each approval check reads the app's identity too; nothing else is asked.
    const methods = (await harness.requests()).slice(before).map((entry) => (entry as { method?: string }).method ?? "").filter((method) => method !== "app_identity");
    assert.deepEqual(methods, STALE_CALLS.map(() => "get_window"));
  } finally {
    await harness.close();
  }
});

// Ticket 53: a click or drag waits up to 0.3 s for the target to read its last button release. A
// target that has not read it by then is busy; the call still succeeds (a failure would invite a
// retry that clicks twice) and the helper marks the result read_confirmed: false. The message says
// the input may still take effect and asks for a screenshot before repeating it, in pointer words.
test("a click or drag the target had not read in time succeeds and warns it may act on it later", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 5 }, () => ({ result: demo })),
        capture_window: [{ result: picture }],
        activate_window: Array.from({ length: 4 }, () => ({ result: {} })),
        click: [
          { result: { read_confirmed: false } },
          { result: { read_confirmed: false, foreground_changed: { foreground: null, target_closed: false } } },
        ],
        drag: [{ result: { read_confirmed: false } }],
        type_text: [{ result: { read_confirmed: false } }],
      },
    },
  });
  const slow = (input: string) =>
    `The ${input} was sent to Demo, but it had not read it after 0.3 seconds (it may be busy), so it may act on it later; take a screenshot to check the result before repeating it.`;
  try {
    await prepare(harness);
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 20, y: 30, delivery_mode: "foreground" } });
    assert.notEqual(clicked.isError, true, textContent(clicked));
    assert.equal(textContent(clicked), `Clicked left 1 time(s) at window pixel (20, 30). ${slow("click")}`);
    const doubled = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 20, y: 30, click_count: 2, delivery_mode: "foreground" } });
    assert.notEqual(doubled.isError, true, textContent(doubled));
    assert.ok(
      textContent(doubled).startsWith(`Clicked left 2 time(s) at window pixel (20, 30). ${slow("click")} After the click, `),
      `the warning comes before the foreground note: ${textContent(doubled)}`,
    );
    const dragged = await harness.client.callTool({ name: "computer_drag", arguments: { window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50 } });
    assert.notEqual(dragged.isError, true, textContent(dragged));
    assert.equal(textContent(dragged), `Dragged from (10, 10) to (40, 50). ${slow("drag")}`);
    // The keyboard wording is unchanged for an input the helper marks the same way.
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "7*8" } });
    assert.notEqual(typed.isError, true, textContent(typed));
    assert.equal(
      textContent(typed),
      "Typed text into the target window. The keys were sent to Demo, but this app does not let Deskpass confirm it read each key; take a screenshot to check the result before continuing.",
    );
  } finally {
    await harness.close();
  }
});

test("a click or drag the helper confirmed the target read carries no warning", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 3 }, () => ({ result: demo })),
        capture_window: [{ result: picture }],
        activate_window: Array.from({ length: 2 }, () => ({ result: {} })),
        click: [{ result: {} }],
        drag: [{ result: {} }],
      },
    },
  });
  try {
    await prepare(harness);
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 20, y: 30, delivery_mode: "foreground" } });
    assert.equal(textContent(clicked), "Clicked left 1 time(s) at window pixel (20, 30).");
    const dragged = await harness.client.callTool({ name: "computer_drag", arguments: { window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50 } });
    assert.equal(textContent(dragged), "Dragged from (10, 10) to (40, 50).");
  } finally {
    await harness.close();
  }
});
