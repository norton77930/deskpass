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

function capture(image: string) {
  return {
    image_base64: image,
    original_width: 400,
    original_height: 300,
    window_x: 10,
    window_y: 20,
    window_width: 400,
    window_height: 300,
  };
}

test("computer_screenshot returns the image and scale metadata", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }],
        capture_window: [{ result: capture("RlJBTUUx") }, { result: capture("RlJBTUUy") }],
      },
    },
  });
  try {
    const first = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(first.isError, true);
    const image = (first.content as Array<{ type: string; data?: string; mimeType?: string }>).find((block) => block.type === "image");
    assert.equal(image?.data, "RlJBTUUx");
    assert.equal(image?.mimeType, "image/jpeg");
    const text = textContent(first);
    assert.match(text, /screenshot_id: ss_1/);
    assert.match(text, /image: 400x300/);
    assert.match(text, /window: 400x300/);
    assert.match(text, /scale: 1\.0000/);
    const second = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.match(textContent(second), /screenshot_id: ss_2/);
    const requests = await harness.requests();
    const captureCall = requests.find((entry) => (entry as { method?: string }).method === "capture_window") as { params?: { target_width?: number; target_height?: number } };
    assert.equal(captureCall.params?.target_width, 400);
    assert.equal(captureCall.params?.target_height, 300);
  } finally {
    await harness.close();
  }
});

test("a window larger than the image budget is captured at the scaled size", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: { ...openWindow, width: 4000, height: 3000 } }],
        capture_window: [{ result: { ...capture("SCALED"), original_width: 4000, original_height: 3000, window_width: 4000, window_height: 3000 } }],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(result.isError, true);
    const requests = await harness.requests();
    const captureCall = requests.find((entry) => (entry as { method?: string }).method === "capture_window") as {
      params?: { target_width?: number; target_height?: number };
    };
    const targetWidth = captureCall.params?.target_width ?? 0;
    const targetHeight = captureCall.params?.target_height ?? 0;
    assert.ok(targetWidth < 4000 && targetHeight < 3000);
    assert.ok(targetWidth <= 1568 && targetHeight <= 1568);
    assert.match(textContent(result), new RegExp(`image: ${targetWidth}x${targetHeight}`));
    assert.match(textContent(result), /window: 4000x3000/);
  } finally {
    await harness.close();
  }
});

test("a captured frame a pixel off the window size is used as is for the image and for clicks", async () => {
  const frame = { ...capture("RlJBTUUx"), original_width: 399, original_height: 300, image_width: 399, image_height: 300 };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }, { result: openWindow }],
        capture_window: [{ result: frame }],
        activate_window: [{ result: {} }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(shot.isError, true);
    const text = textContent(shot);
    assert.match(text, /image: 399x300/);
    assert.match(text, /window: 399x300/);
    assert.match(text, /scale: 1\.0000/);
    const edge = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 398, y: 150 } });
    assert.notEqual(edge.isError, true, textContent(edge));
    const beyond = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 399, y: 150 } });
    assert.equal(textContent(beyond).split(/\r?\n/)[0], "out_of_bounds");
    const clicks = (await harness.requests()).filter((entry) => (entry as { method?: string }).method === "click") as Array<{ params?: { x?: number; y?: number } }>;
    assert.equal(clicks.length, 1);
    assert.deepEqual([clicks[0].params?.x, clicks[0].params?.y], [398, 150]);
  } finally {
    await harness.close();
  }
});

test("a minimized window returns window_minimized and does not capture", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: { ...openWindow, minimized: true } }],
        capture_window: [{ result: capture("SHOULD_NOT_CAPTURE") }],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "window_minimized");
    assert.match(text, /computer_activate_window/);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "capture_window"), false);
  } finally {
    await harness.close();
  }
});

test("a closed window returns window_not_found", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ error: { code: "window_not_found", message: "The window is gone." } }],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "missing" } });
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split(/\r?\n/)[0], "window_not_found");
  } finally {
    await harness.close();
  }
});

test("a window partly outside every screen names its on-screen parts in image pixels, and input off screen is offscreen", async () => {
  const wide = { ...openWindow, width: 2000, height: 1000 };
  const frame = {
    ...capture("RlJBTUUx"),
    original_width: 2000,
    original_height: 1000,
    image_width: 1568,
    image_height: 784,
    window_width: 2000,
    window_height: 1000,
  };
  const offscreen = {
    error: {
      code: "offscreen",
      message: "Screen point (-459, 19) is outside every screen: that part of the window is off screen, so no input was sent.",
    },
  };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: Array.from({ length: 4 }, () => ({ result: wide })),
        capture_window: [
          // The top 100 rows of the left half are off screen; the right half is on another monitor.
          { result: { ...frame, visible_parts: [{ x: 0, y: 100, width: 1000, height: 900 }, { x: 1000, y: 0, width: 1000, height: 1000 }] } },
          { result: { ...frame, visible_parts: [] } },
          { result: frame },
        ],
        activate_window: [{ result: {} }],
        click: [offscreen],
      },
    },
  });
  try {
    const partly = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(partly.isError, true);
    const lines = textContent(partly).split(/\r?\n/);
    assert.deepEqual(lines.slice(0, 4), ["screenshot_id: ss_1", "image: 1568x784", "window: 2000x1000", "scale: 0.7840"]);
    assert.equal(lines.length, 5);
    assert.match(lines[4], /^on screen: image x 0-783, y 79-783 and x 784-1567, y 0-783; /);
    assert.match(lines[4], /outside every screen/);

    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 700, y: 10 } });
    assert.equal(clicked.isError, true);
    const [code, ...rest] = textContent(clicked).split(/\r?\n/);
    assert.equal(code, "offscreen");
    assert.match(rest.join("\n"), /outside every screen.*no input was sent/);

    const gone = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const goneLines = textContent(gone).split(/\r?\n/);
    assert.equal(goneLines.length, 5);
    assert.match(goneLines[4], /^on screen: none; the whole window is outside every screen/);

    const whole = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(textContent(whole).split(/\r?\n/).length, 4, "a window wholly on screen keeps the four metadata lines");
  } finally {
    await harness.close();
  }
});

test("the target's own popups beside the window are in the image, named in one line, and clickable at negative window pixels", async () => {
  // The helper's canvas: a 400x300 window and its menu, 60 pixels left of it and down to 420,
  // fitted inside the 400x300 target size.
  const composite = {
    ...capture("Q09NUE9TSVRF"),
    original_width: 460,
    original_height: 420,
    image_width: 329,
    image_height: 300,
    origin_x: -60,
    origin_y: 0,
    popups: [{ kind: "menu", x: -60, y: 260, width: 203, height: 160 }],
    // The canvas's bottom 20 rows are below every screen.
    visible_parts: [{ x: -60, y: 0, width: 460, height: 400 }],
  };
  const resized = { ...openWindow, width: 401 };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }, { result: openWindow }, { result: resized }, { result: openWindow }, { result: openWindow }],
        capture_window: [
          { result: composite },
          { result: { ...capture("Wk9PTQ=="), original_width: 201, original_height: 159, image_width: 201, image_height: 159 } },
          { result: { ...capture("VEFSR0VU"), popups_not_captured: { kinds: ["menu"], message: "the menu could not be captured (PrintWindow failed)" } } },
          { result: capture("UExBSU4=") },
        ],
        activate_window: [{ result: {} }],
        click: [{ result: {} }],
      },
    },
  });
  try {
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(shot.isError, true, textContent(shot));
    const lines = textContent(shot).split(/\r?\n/);
    assert.deepEqual(lines.slice(0, 4), ["screenshot_id: ss_1", "image: 329x300", "window: 460x420", "scale: 0.7152"]);
    assert.equal(lines.length, 6, lines.join("\n"));
    // Canvas coordinates: the window's left edge is image x 43, not 0.
    assert.match(lines[4], /^on screen: image x 0-328, y 0-285; /);
    // The window itself is wholly on screen; only the menu reaches below it.
    assert.match(lines[4], /parts of its popups are outside every screen/);
    assert.doesNotMatch(lines[4], /rest of the window/);
    assert.match(lines[5], /^popups: menu at image x 0-144, y 186-299 /);
    assert.match(lines[5], /window itself is at image x 43-328, y 0-213/);

    // A point on the menu left of and below the window: window pixel (-60 + 28, 392).
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 20, y: 280 } });
    assert.notEqual(clicked.isError, true, textContent(clicked));
    assert.match(textContent(clicked), /window pixel \(-32, 392\)/);

    // A zoom of the menu crops it in window pixels.
    const zoomed = await harness.client.callTool({ name: "computer_zoom", arguments: { window_id: "5", region: [0, 186, 144, 299] } });
    assert.notEqual(zoomed.isError, true, textContent(zoomed));

    // The window's own size, not the canvas, decides whether the screenshot is stale.
    const stale = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 20, y: 280 } });
    assert.equal(textContent(stale).split(/\r?\n/)[0], "stale_screenshot");
    assert.match(textContent(stale), /was 400x300 when ss_1 was taken and is now 401x300/);

    const fallback = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const fallbackLines = textContent(fallback).split(/\r?\n/);
    assert.equal(fallbackLines.length, 5, fallbackLines.join("\n"));
    assert.match(fallbackLines[4], /^popups: the menu could not be captured \(PrintWindow failed\); this image shows the window only/);

    const plain = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(textContent(plain).split(/\r?\n/).length, 4, "without popups the metadata keeps its four lines");

    const requests = (await harness.requests()) as Array<{ method?: string; params?: Record<string, unknown> }>;
    const clicks = requests.filter((entry) => entry.method === "click");
    assert.equal(clicks.length, 1);
    assert.deepEqual([clicks[0].params?.x, clicks[0].params?.y], [-32, 392]);
    const crops = requests.filter((entry) => entry.method === "capture_window" && entry.params?.crop !== undefined);
    assert.deepEqual(crops[0]?.params?.crop, { x: -60, y: 260, width: 201, height: 159 });
  } finally {
    await harness.close();
  }
});

test("a malformed popups list is reported as popups, not as visible_parts", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }],
        capture_window: [{ result: { ...capture("QkFE"), popups: [{ kind: "menu", x: "left", y: 0, width: 10, height: 10 }] } }],
      },
    },
  });
  try {
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(shot.isError, true);
    assert.match(textContent(shot), /popups x/);
    assert.doesNotMatch(textContent(shot), /visible_parts/);
  } finally {
    await harness.close();
  }
});
