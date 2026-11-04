import assert from "node:assert/strict";
import { test } from "node:test";
import { INTERRUPTED_MESSAGE } from "../src/helper-client.ts";
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

test("a stopped event interrupts the in-flight request", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        list_windows: [{ delayMs: 30, events: [{ event: "stopped" }], result: { windows: [] } }],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(textContent(result).split(/\r?\n/)[0], "interrupted");
    assert.match(textContent(result), /Do not call these tools again/);
    assert.match(textContent(result), /End this turn and tell the user/);
  } finally {
    await harness.close();
  }
});

test("after Esc, the next action asks to continue and an allow runs it", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
      },
    },
  });
  try {
    const interrupted = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(textContent(interrupted).split(/\r?\n/)[0], "interrupted");
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(harness.elicitations[0], "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?");
    assert.match(harness.elicitations[1] ?? "", /demo\.exe/i);
    assert.notEqual(shot.isError, true);
    assert.match(textContent(shot), /screenshot_id: ss_1/);
  } finally {
    await harness.close();
  }
});

test("after Esc, Accept alone on the continue question runs the action", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => ({ action: "accept" }),
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(shot.isError, true, textContent(shot));
    assert.equal(harness.elicitations[0], "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?");
    assert.deepEqual(Object.keys((harness.elicitationSchemas[0] as { properties?: object }).properties ?? {}), []);
  } finally {
    await harness.close();
  }
});

test("declining the continue question stays stopped", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => ({ action: "decline" }),
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
        get_window: [{ result: openWindow }],
        capture_window: [{ result: captured }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const first = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const second = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(textContent(first).split(/\r?\n/)[0], "stopped_by_user");
    assert.match(textContent(first), /Do not call these tools again/);
    assert.match(textContent(first), /End this turn and tell the user/);
    assert.equal(textContent(second).split(/\r?\n/)[0], "stopped_by_user");
    assert.equal(harness.elicitations.length, 2);
    assert.equal(harness.elicitations.every((message) => message.includes("要讓 AI 繼續嗎?")), true);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "capture_window"), false);
  } finally {
    await harness.close();
  }
});

// Ticket 45 (F7): the continue question shares the approval's answer rules (test/approval.test.ts).
for (const [label, answer] of [
  ["an accept that still says deny", { action: "accept" as const, content: { decision: "deny" } }],
  ["a cancel", { action: "cancel" as const }],
] as const) {
  test(`${label} on the continue question stays stopped`, async () => {
    const harness = await connectServer({
      elicitation: "form",
      onElicit: () => answer,
      script: {
        handlers: {
          list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
          get_window: [{ result: openWindow }],
          capture_window: [{ result: captured }],
        },
      },
    });
    try {
      await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
      const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
      assert.equal(textContent(shot).split(/\r?\n/)[0], "stopped_by_user");
      assert.deepEqual(harness.elicitations, ["你先前按了 Esc 停止操作,要讓 AI 繼續嗎?"]);
      const methods = (await harness.requests()).map((entry) => (entry as { method?: string }).method);
      assert.equal(methods.some((method) => method === "resume" || method === "capture_window"), false, methods.join(","));
    } finally {
      await harness.close();
    }
  });
}

test("a client that cannot ask stays stopped when the helper dialog cannot ask either", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
        get_window: [{ result: openWindow }],
        ask_user: [{ error: { code: "prompt_unavailable", message: "There is no interactive desktop." } }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(textContent(shot).split(/\r?\n/)[0], "stopped_by_user");
    assert.match(textContent(shot), /End this turn and tell the user/);
    assert.equal(harness.elicitations.length, 0);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "get_window"), false);
  } finally {
    await harness.close();
  }
});

const INPUT_METHODS = new Set(["overlay_show", "activate_window", "click", "type_text", "press_key", "scroll", "drag", "move_mouse", "launch_app"]);

function methodOf(entry: unknown): string {
  return String((entry as { method?: unknown }).method ?? "");
}

test("a tool call queued behind a prompt when Esc is pressed sends no input afterwards", async () => {
  const otherWindow = { ...openWindow, window_id: "7", app: "C:\\Apps\\other.exe", app_name: "Other", title: "Other" };
  let releaseOther: () => void = () => {};
  const otherMayContinue = new Promise<void>((resolve) => {
    releaseOther = resolve;
  });
  let otherAsked: () => void = () => {};
  const otherWaiting = new Promise<void>((resolve) => {
    otherAsked = resolve;
  });
  const harness = await connectServer({
    elicitation: "form",
    onElicit: async (message) => {
      if (message.includes("other.exe")) {
        otherAsked();
        await otherMayContinue;
      }
      return { action: "accept", content: { decision: "allow" } };
    },
    script: {
      handlers: {
        get_window: [{ result: otherWindow }, { result: openWindow }],
        type_text: [{ events: [{ event: "stopped", data: { generation: 1 } }], result: {} }],
      },
    },
  });
  try {
    // The press_key call waits at its approval prompt while the type_text call is stopped by Esc.
    const pressing = harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "7", key: "Return" } });
    await otherWaiting;
    const typed = await harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "hello" } });
    assert.equal(textContent(typed).split(/\r?\n/)[0], "interrupted");
    releaseOther();
    const pressed = await pressing;
    assert.match(textContent(pressed).split(/\r?\n/)[0] ?? "", /^(interrupted|stopped_by_user)$/);
    assert.match(textContent(pressed), /End this turn and tell the user/);
    const requests = (await harness.requests()).map(methodOf);
    const after = requests.slice(requests.indexOf("type_text") + 1);
    assert.deepEqual(after.filter((method) => INPUT_METHODS.has(method)), [], `input reached the helper after Esc: ${requests.join(", ")}`);
  } finally {
    await harness.close();
  }
});

test("allowing to continue resumes the helper at the stop it saw before any input", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped", data: { generation: 4 } }], result: { windows: [] } }],
        get_window: [{ result: openWindow }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const pressed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(pressed.isError, true, textContent(pressed));
    const requests = await harness.requests();
    const methods = requests.map(methodOf);
    const resume = requests.find((entry) => methodOf(entry) === "resume") as { params?: { generation?: unknown } } | undefined;
    assert.equal(resume?.params?.generation, 4);
    assert.ok(methods.indexOf("resume") < methods.indexOf("press_key"), methods.join(", "));
  } finally {
    await harness.close();
  }
});

test("the first approval starts the helper session and closing ends it", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }],
        capture_window: [{ result: captured }, { result: captured }],
      },
    },
  });
  try {
    const first = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const second = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.notEqual(first.isError, true);
    assert.notEqual(second.isError, true);
    const running = await harness.requests();
    assert.equal(running.filter((entry) => (entry as { method?: string }).method === "session_start").length, 1);
    await harness.finish();
    const finished = await harness.requests();
    assert.equal(finished.filter((entry) => (entry as { method?: string }).method === "session_end").length, 1);
  } finally {
    await harness.close();
  }
});

test("after the helper crashes, the next input call starts a session on the new helper before any input", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }],
        list_windows: [{ exit: true, stderr: "helper went away" }],
      },
    },
  });
  try {
    const before = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(before.isError, true, textContent(before));
    const crashed = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(textContent(crashed).split(/\r?\n/)[0], "helper_crashed");
    const after = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(after.isError, true, textContent(after));
    const methods = (await harness.requests()).map(methodOf);
    const restarted = methods.slice(methods.indexOf("list_windows") + 1);
    assert.ok(restarted.includes("session_start"), `no session_start on the new helper: ${methods.join(", ")}`);
    assert.ok(restarted.indexOf("session_start") < restarted.indexOf("overlay_show"), methods.join(", "));
    assert.ok(restarted.indexOf("session_start") < restarted.indexOf("press_key"), methods.join(", "));
    // The restart is not a stop: only the approval was asked.
    assert.equal(harness.elicitations.length, 1);
  } finally {
    await harness.close();
  }
});

// Ticket 52 (F32): the same after a timeout, where the server itself kills the helper that held the session.
test("after a helper timeout, the next input call starts a session on the new helper before any input", { timeout: 25_000 }, async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }],
        list_windows: [{ delayMs: 11_000, result: { windows: [] } }],
      },
    },
  });
  try {
    const before = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(before.isError, true, textContent(before));
    const timedOut = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(textContent(timedOut).split(/\r?\n/)[0], "helper_timeout");
    const after = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(after.isError, true, textContent(after));
    const methods = (await harness.requests()).map(methodOf);
    assert.equal(methods.filter((method) => method === "ping").length, 2, `the helper was not restarted after the timeout: ${methods.join(", ")}`);
    const restarted = methods.slice(methods.indexOf("list_windows") + 1);
    assert.ok(restarted.includes("session_start"), `no session_start on the new helper: ${methods.join(", ")}`);
    assert.ok(restarted.indexOf("session_start") < restarted.indexOf("overlay_show"), methods.join(", "));
    assert.ok(restarted.indexOf("session_start") < restarted.indexOf("press_key"), methods.join(", "));
    // The timeout is not a stop: only the approval was asked.
    assert.equal(harness.elicitations.length, 1);
  } finally {
    await harness.close();
  }
});

test("a stop survives a helper restart, and a stop from the new helper is resumed at its own generation", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }, { result: openWindow }],
        list_windows: [
          { events: [{ event: "stopped", data: { generation: 4 } }], result: { windows: [] } },
          { exit: true },
          { events: [{ event: "stopped", data: { generation: 1 } }], result: { windows: [] } },
        ],
      },
    },
  });
  const call = async (name: string, args: Record<string, unknown>) => harness.client.callTool({ name, arguments: args });
  try {
    assert.equal(textContent(await call("computer_list_windows", {})).split(/\r?\n/)[0], "interrupted");
    assert.equal(textContent(await call("computer_list_windows", {})).split(/\r?\n/)[0], "helper_crashed");
    const first = await call("computer_press_key", { window_id: "5", key: "Return" });
    assert.notEqual(first.isError, true, textContent(first));
    assert.equal(harness.elicitations[0], "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?");
    assert.equal(textContent(await call("computer_list_windows", {})).split(/\r?\n/)[0], "interrupted");
    const second = await call("computer_press_key", { window_id: "5", key: "Return" });
    assert.notEqual(second.isError, true, textContent(second));
    assert.equal(harness.elicitations.filter((message) => message.includes("要讓 AI 繼續嗎?")).length, 2);
    const requests = await harness.requests();
    const resumes = requests.filter((entry) => methodOf(entry) === "resume") as Array<{ params?: { generation?: unknown } }>;
    // The helper that saw stop 4 is gone: the new one has no stop the server saw, and later reports its own stop 1.
    assert.deepEqual(resumes.map((entry) => entry.params?.generation), [0, 1]);
    const methods = requests.map(methodOf);
    assert.ok(methods.indexOf("session_start") < methods.indexOf("press_key"), methods.join(", "));
  } finally {
    await harness.close();
  }
});

test("an overlay and session on a helper that crashed are not hidden or ended on a new helper at close", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: openWindow }],
        list_windows: [{ exit: true }],
      },
    },
  });
  try {
    const pressed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
    assert.notEqual(pressed.isError, true, textContent(pressed));
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    await harness.finish();
    const methods = (await harness.requests()).map(methodOf);
    assert.deepEqual(methods.slice(methods.indexOf("list_windows") + 1), [], `a new helper was started to clean up: ${methods.join(", ")}`);
  } finally {
    await harness.close();
  }
});

// Ticket 56 (F3): a stop that arrives after the user allowed continuing but before the call's
// approval question (no request in flight, so nothing fails on its own) is not followed by the
// question; the call ends `interrupted`, and input stays stopped for the next call.
test("a stop between the continue answer and the approval question ends the call without asking", async () => {
  let asked = 0;
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => {
      asked += 1;
      return asked === 1 ? { action: "accept", content: {} } : { action: "decline" };
    },
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped", data: { generation: 1 } }], result: { windows: [] } }],
        resume: [{ result: {}, eventsAfter: [{ event: "stopped", data: { generation: 2 } }] }],
        get_window: [{ result: openWindow }],
      },
    },
  });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(result.isError, true);
    assert.equal(textContent(result).split(/\r?\n/)[0], "interrupted");
    assert.equal(harness.elicitations.length, 1, `only the continue question is asked: ${JSON.stringify(harness.elicitations)}`);
    // The next call asks whether to continue again (declined here), rather than going on stopped-unaware.
    const again = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    assert.equal(harness.elicitations.length, 2, "the next call asks whether to continue again");
    assert.match(harness.elicitations[1] ?? "", /Esc/);
    assert.equal(textContent(again).split(/\r?\n/)[0], "stopped_by_user");
  } finally {
    await harness.close();
  }
});

// Ticket 70: the helper's own `interrupted` text reaches the model only when it says to end the
// turn; its shorter ones (a stop during input, input refused while stopped) and a missing text
// become the server's.
test("an interrupted answer without the end-this-turn instruction reaches the model as the server's message", async () => {
  for (const message of ["Input stopped before it finished.", "Input is stopped until the server resumes it.", undefined]) {
    const error = (message === undefined ? { code: "interrupted" } : { code: "interrupted", message }) as { code: string; message: string };
    const harness = await connectServer({
      elicitation: "form",
      script: { handlers: { get_window: [{ result: openWindow }], press_key: [{ error }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "Return" } });
      assert.equal(textContent(result), `interrupted\n${INTERRUPTED_MESSAGE}`, String(message));
    } finally {
      await harness.close();
    }
  }
});
