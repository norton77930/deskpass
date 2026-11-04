import assert from "node:assert/strict";
import { test } from "node:test";
import { approvalAnswers, approvalEvents, connectServer, textContent, type Harness } from "./harness.ts";

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

function shot(label: string) {
  return {
    image_base64: "QQ==",
    original_width: 200,
    original_height: 100,
    window_x: 0,
    window_y: 0,
    window_width: 200,
    window_height: 100,
    label,
  };
}

async function screenshot(harness: Harness) {
  return harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
}

test("the first action asks, and an allow is remembered for the session", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        capture_window: [{ result: shot("one") }, { result: shot("two") }],
      },
    },
  });
  try {
    const first = await screenshot(harness);
    assert.notEqual(first.isError, true);
    assert.equal(harness.elicitations.length, 1);
    assert.match(harness.elicitations[0] ?? "", /Demo/);
    assert.match(harness.elicitations[0] ?? "", /demo\.exe/i);
    const second = await screenshot(harness);
    assert.notEqual(second.isError, true);
    assert.equal(harness.elicitations.length, 1);
  } finally {
    await harness.close();
  }
});

test("a decline is remembered for the session: the next action is refused at once, without asking", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => ({ action: "decline" }),
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        capture_window: [{ result: shot("after") }],
      },
    },
  });
  try {
    const declined = await screenshot(harness);
    assert.equal(declined.isError, true);
    const declinedText = textContent(declined);
    assert.equal(declinedText.split(/\r?\n/)[0], "approval_declined");
    assert.match(declinedText, /report/i);
    const again = await screenshot(harness);
    assert.equal(again.isError, true);
    const againText = textContent(again);
    assert.equal(againText.split(/\r?\n/)[0], "approval_refused_earlier");
    assert.match(againText, /Demo/);
    assert.match(againText, /refused/i);
    assert.match(againText, /will not ask again/i);
    assert.match(againText, /restart/i);
    assert.equal(harness.elicitations.length, 1);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => ["capture_window", "ask_user"].includes((entry as { method?: string }).method ?? "")), false);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "elicitation", answer: "refuse" },
      { channel: "session", answer: "refused_earlier" },
    ]);
  } finally {
    await harness.close();
  }
});

test("a remembered refusal is kept per app: another app is still asked", async () => {
  const other = { ...demo, window_id: "7", app: "C:\\Apps\\other.exe", app_name: "Other", pid: 4343 };
  const harness = await connectServer({
    elicitation: "form",
    onElicit: (message) => (/Other/.test(message) ? { action: "accept" } : { action: "decline" }),
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: other }, { result: { ...demo, app: "C:\\APPS\\Demo.EXE" } }],
        capture_window: [{ result: shot("other") }],
      },
    },
  });
  try {
    assert.equal(textContent(await screenshot(harness)).split(/\r?\n/)[0], "approval_declined");
    const allowed = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "7" } });
    assert.notEqual(allowed.isError, true, textContent(allowed));
    // The same app named with other letter case has the same approval key.
    assert.equal(textContent(await screenshot(harness)).split(/\r?\n/)[0], "approval_refused_earlier");
    assert.equal(harness.elicitations.length, 2);
  } finally {
    await harness.close();
  }
});

test("a remembered refusal also refuses computer_launch_app and a batch step, without asking or sending input", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => ({ action: "decline" }),
    resolveApp: async () => "C:\\Apps\\demo.exe",
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], launch_app: [{ result: { pid: 50, window: null } }] } },
  });
  try {
    assert.equal(textContent(await screenshot(harness)).split(/\r?\n/)[0], "approval_declined");
    const launched = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } });
    assert.equal(textContent(launched).split(/\r?\n/)[0], "approval_refused_earlier");
    const batch = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "press_key", window_id: "5", key: "a" }, { action: "press_key", window_id: "5", key: "b" }] },
    });
    assert.equal(batch.isError, true);
    const text = textContent(batch);
    assert.match(text, /stopped at step 1 of 2: approval_refused_earlier/);
    assert.match(text, /\[1\] press_key: approval_refused_earlier \(sent: false\) /);
    assert.match(text, /\[2\] press_key: not run/);
    assert.equal(harness.elicitations.length, 1);
    const methods = (await harness.requests()).map((entry) => (entry as { method?: string }).method);
    assert.equal(methods.some((method) => ["launch_app", "press_key", "ask_user"].includes(method ?? "")), false, methods.join(", "));
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "elicitation", answer: "refuse" },
      { channel: "session", answer: "refused_earlier" },
      { channel: "session", answer: "refused_earlier" },
    ]);
  } finally {
    await harness.close();
  }
});

// The frame host is refused by its own rule, before the tiers, so its tier is the one its name has.
for (const [label, args, path, tier] of [
  ["a blocked-tier name", { app: "powershell" }, "powershell", "blocked"],
  ["a blocked-tier app it resolves to", { app: "shell" }, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", "blocked"],
  ["the UWP frame host", { app: "ApplicationFrameHost.exe" }, "ApplicationFrameHost.exe", "normal"],
] as const) {
  test(`computer_launch_app refusing ${label} logs a blocked approval with the app`, async () => {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async () => "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: args });
      assert.equal(textContent(result).split(/\r?\n/)[0], "blocked_app");
      const events = await approvalEvents(harness);
      assert.deepEqual(events.map(({ channel, answer }) => ({ channel, answer })), [{ channel: null, answer: "blocked" }]);
      assert.deepEqual(events[0]?.app, { path, key: path.toLowerCase(), tier });
    } finally {
      await harness.close();
    }
  });
}

test("every approval decision is logged with the app, the channel and the answer", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        capture_window: [{ result: shot("one") }, { result: shot("two") }],
      },
    },
  });
  try {
    assert.notEqual((await screenshot(harness)).isError, true);
    assert.notEqual((await screenshot(harness)).isError, true);
    const events = await approvalEvents(harness);
    assert.equal(events.length, 2);
    for (const event of events) {
      assert.deepEqual(Object.keys(event).sort(), ["answer", "app", "channel", "event", "session", "time"]);
      assert.deepEqual(event.app, { path: "C:\\Apps\\demo.exe", key: "c:\\apps\\demo.exe", tier: "normal" });
      assert.equal(typeof event.session, "string");
      assert.ok(!Number.isNaN(Date.parse(String(event.time))));
    }
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "elicitation", answer: "allow_once" },
      { channel: "session", answer: "allow_once" },
    ]);
  } finally {
    await harness.close();
  }
});

test("a new session asks again", async () => {
  const script = {
    handlers: {
      get_window: [{ result: demo }],
      capture_window: [{ result: shot("once") }],
    },
  };
  const first = await connectServer({ elicitation: "form", script });
  const second = await connectServer({ elicitation: "form", script });
  try {
    assert.notEqual((await screenshot(first)).isError, true);
    assert.equal(first.elicitations.length, 1);
    assert.notEqual((await screenshot(second)).isError, true);
    assert.equal(second.elicitations.length, 1);
  } finally {
    await first.close();
    await second.close();
  }
});

test("a new session does not carry a refusal from the last one", async () => {
  const script = {
    handlers: {
      get_window: [{ result: demo }],
      capture_window: [{ result: shot("once") }],
    },
  };
  const first = await connectServer({ elicitation: "form", onElicit: () => ({ action: "decline" }), script });
  const second = await connectServer({ elicitation: "form", script });
  try {
    assert.equal(textContent(await screenshot(first)).split(/\r?\n/)[0], "approval_declined");
    assert.notEqual((await screenshot(second)).isError, true);
    assert.equal(second.elicitations.length, 1);
  } finally {
    await first.close();
    await second.close();
  }
});

test("a client without elicitation is refused when the helper dialog cannot ask either", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }],
        capture_window: [{ result: shot("nope") }],
        ask_user: [{ error: { code: "prompt_unavailable", message: "There is no interactive desktop." } }],
      },
    },
  });
  try {
    const result = await screenshot(harness);
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "approval_unavailable");
    assert.match(text, /report/i);
    assert.equal(harness.elicitations.length, 0);
  } finally {
    await harness.close();
  }
});

test("a high-risk app is blocked without a prompt", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{
          result: {
            ...demo,
            app: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            app_name: "Windows PowerShell",
          },
        }],
        capture_window: [{ result: shot("blocked") }],
      },
    },
  });
  try {
    const result = await screenshot(harness);
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "blocked_app");
    assert.match(text, /report/i);
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual(await approvalAnswers(harness), [{ channel: null, answer: "blocked" }]);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "capture_window"), false);
  } finally {
    await harness.close();
  }
});

test("an elevated window is refused when the helper is not elevated", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: [{ result: { ...demo, elevated: true, helper_elevated: false } }],
        capture_window: [{ result: shot("admin") }],
      },
    },
  });
  try {
    const result = await screenshot(harness);
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.equal(text.split(/\r?\n/)[0], "elevated_target");
    assert.match(text, /report/i);
    assert.equal(harness.elicitations.length, 0);
  } finally {
    await harness.close();
  }
});

test("the approval form has no field to fill: Accept alone allows", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => ({ action: "accept" }),
    script: {
      handlers: {
        get_window: [{ result: demo }],
        capture_window: [{ result: shot("accepted") }],
      },
    },
  });
  try {
    const result = await screenshot(harness);
    assert.notEqual(result.isError, true, textContent(result));
    const schema = harness.elicitationSchemas[0] as { properties?: object; required?: unknown[] };
    assert.deepEqual(Object.keys(schema.properties ?? {}), []);
    assert.deepEqual(schema.required ?? [], []);
  } finally {
    await harness.close();
  }
});

for (const [label, answer] of [
  ["an accept that still says deny", { action: "accept" as const, content: { decision: "deny" } }],
  ["a cancel", { action: "cancel" as const }],
] as const) {
  test(`${label} is a decline`, async () => {
    const harness = await connectServer({
      elicitation: "form",
      onElicit: () => answer,
      script: { handlers: { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: shot("no") }] } },
    });
    try {
      const result = await screenshot(harness);
      assert.equal(textContent(result).split(/\r?\n/)[0], "approval_declined");
      const requests = await harness.requests();
      assert.equal(requests.some((entry) => (entry as { method?: string }).method === "capture_window"), false);
      // Ticket 90: remembered like a Decline: the next call is refused without a second elicitation.
      assert.equal(textContent(await screenshot(harness)).split(/\r?\n/)[0], "approval_refused_earlier");
      assert.equal(harness.elicitations.length, 1);
      assert.deepEqual(await approvalAnswers(harness), [
        { channel: "elicitation", answer: "refuse" },
        { channel: "session", answer: "refused_earlier" },
      ]);
    } finally {
      await harness.close();
    }
  });
}

test("listing windows does not ask for approval", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: { handlers: { list_windows: [{ result: { windows: [demo] } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(result.isError, true);
    assert.equal(harness.elicitations.length, 0);
  } finally {
    await harness.close();
  }
});
