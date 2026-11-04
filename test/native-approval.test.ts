import assert from "node:assert/strict";
import { test, type TestContext } from "node:test";
import { ASK_USER_DIALOG_MS, ASK_USER_TIMEOUT_MS } from "../src/config.ts";
import { approvalAnswers, connectServer, textContent, type FakeHandler, type Harness } from "./harness.ts";

// When the client cannot ask (no form elicitation, or elicitInput fails), the server asks through
// the helper's `ask_user` dialog. The fake helper answers it from the script.

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

const other = { ...demo, window_id: "7", app: "C:\\Apps\\other.exe", app_name: "Other", title: "Other", pid: 4343 };

const shot = {
  image_base64: "QQ==",
  original_width: 200,
  original_height: 100,
  window_x: 0,
  window_y: 0,
};

const allow: FakeHandler = { result: { answer: "allow" } };
const deny: FakeHandler = { result: { answer: "deny" } };
const timedOut: FakeHandler = { result: { answer: "timeout" } };
const cannotShow: FakeHandler = { error: { code: "prompt_unavailable", message: "There is no interactive desktop." } };

interface Logged {
  id?: number;
  method?: string;
  params?: Record<string, unknown>;
  at?: number;
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

async function logged(harness: Harness): Promise<Logged[]> {
  return (await harness.requests()) as Logged[];
}

async function asks(harness: Harness): Promise<Logged[]> {
  return (await logged(harness)).filter((entry) => entry.method === "ask_user");
}

function screenshot(harness: Harness, windowId = "5") {
  return harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: windowId } });
}

/** Collects what the server writes with console.error. */
function captureStderr(t: TestContext): () => string {
  const mocked = t.mock.method(console, "error", () => {});
  return () => mocked.mock.calls.map((call) => call.arguments.map(String).join(" ")).join("\n");
}

test("the server waits longer for ask_user than the dialog stays open", () => {
  assert.equal(ASK_USER_DIALOG_MS, 60_000);
  assert.ok(ASK_USER_TIMEOUT_MS > ASK_USER_DIALOG_MS, `${ASK_USER_TIMEOUT_MS} <= ${ASK_USER_DIALOG_MS}`);
});

test("a client without elicitation asks through the helper dialog, and an allow is remembered", async () => {
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: shot }, { result: shot }], ask_user: [allow] } },
  });
  try {
    const first = await screenshot(harness);
    assert.notEqual(first.isError, true, textContent(first));
    const second = await screenshot(harness);
    assert.notEqual(second.isError, true, textContent(second));
    const asked = await asks(harness);
    assert.equal(asked.length, 1, "the allow was not remembered for the session");
    // An unsigned app: no Always allow (ticket 95).
    assert.deepEqual(asked[0]?.params, { kind: "approve_app", app_name: "Demo", app_path: "C:\\Apps\\demo.exe", timeout_ms: ASK_USER_DIALOG_MS, allow_always: false });
    const methods = (await logged(harness)).map((entry) => entry.method);
    assert.ok(methods.indexOf("ask_user") < methods.indexOf("capture_window"), methods.join(", "));
  } finally {
    await harness.close();
  }
});

test("a dialog decline (Refuse, Esc or close) is refused as declined and remembered: the next call is refused without a dialog", async () => {
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: shot }], ask_user: [deny, allow] } },
  });
  try {
    const declined = await screenshot(harness);
    assert.equal(declined.isError, true);
    assert.equal(code(declined), "approval_declined");
    assert.match(textContent(declined), /declined/i);
    assert.match(textContent(declined), /Demo/);
    assert.equal((await logged(harness)).some((entry) => entry.method === "capture_window"), false);
    const again = await screenshot(harness);
    assert.equal(code(again), "approval_refused_earlier");
    assert.equal((await asks(harness)).length, 1);
    assert.equal((await logged(harness)).some((entry) => entry.method === "capture_window"), false);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "dialog", answer: "refuse" },
      { channel: "session", answer: "refused_earlier" },
    ]);
  } finally {
    await harness.close();
  }
});

test("an unanswered dialog is a decline that says the user did not answer, and is not remembered", async () => {
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: shot }], ask_user: [timedOut, allow] } },
  });
  try {
    const result = await screenshot(harness);
    assert.equal(code(result), "approval_declined");
    assert.match(textContent(result), /did not answer/i);
    assert.match(textContent(result), /60 seconds/);
    assert.equal((await logged(harness)).some((entry) => entry.method === "capture_window"), false);
    const allowed = await screenshot(harness);
    assert.notEqual(allowed.isError, true, textContent(allowed));
    assert.equal((await asks(harness)).length, 2);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "dialog", answer: "timeout" },
      { channel: "dialog", answer: "allow_once" },
    ]);
  } finally {
    await harness.close();
  }
});

test("an unavailable dialog is not remembered: the next call asks again", async (t) => {
  captureStderr(t);
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: shot }], ask_user: [cannotShow, allow] } },
  });
  try {
    assert.equal(code(await screenshot(harness)), "approval_unavailable");
    const allowed = await screenshot(harness);
    assert.notEqual(allowed.isError, true, textContent(allowed));
    assert.equal((await asks(harness)).length, 2);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "dialog", answer: "unavailable" },
      { channel: "dialog", answer: "allow_once" },
    ]);
  } finally {
    await harness.close();
  }
});

test("when the dialog cannot be shown the call is refused as unavailable and stderr says why", async (t) => {
  const stderr = captureStderr(t);
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }], ask_user: [cannotShow] } },
  });
  try {
    const result = await screenshot(harness);
    assert.equal(code(result), "approval_unavailable");
    assert.match(textContent(result), /could not be asked/i);
    assert.doesNotMatch(textContent(result), /declined/i);
    assert.equal((await logged(harness)).some((entry) => entry.method === "capture_window"), false);
    assert.match(stderr(), /form elicitation/i);
    assert.match(stderr(), /prompt_unavailable/);
    assert.match(stderr(), /no interactive desktop/);
  } finally {
    await harness.close();
  }
});

test("a dialog answer that is not allow, deny, or timeout refuses the call as unavailable", async () => {
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }], ask_user: [{ result: { answer: "yes" } }] } },
  });
  try {
    assert.equal(code(await screenshot(harness)), "approval_unavailable");
  } finally {
    await harness.close();
  }
});

test("when elicitInput fails the server asks through the dialog and stderr names the failure", async (t) => {
  const stderr = captureStderr(t);
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => {
      throw new Error("elicitation is not available in a background session");
    },
    script: { handlers: { get_window: [{ result: demo }], capture_window: [{ result: shot }], ask_user: [allow] } },
  });
  try {
    const result = await screenshot(harness);
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal(harness.elicitations.length, 1);
    assert.equal((await asks(harness)).length, 1);
    assert.match(stderr(), /elicitation is not available in a background session/);
    assert.deepEqual(await approvalAnswers(harness), [{ channel: "dialog", answer: "allow_once" }]);
  } finally {
    await harness.close();
  }
});

test("a dialog refusal after elicitInput fails is logged as the dialog's and remembered", async (t) => {
  captureStderr(t);
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => {
      throw new Error("elicitation is not available in a background session");
    },
    script: { handlers: { get_window: [{ result: demo }, { result: demo }], capture_window: [{ result: shot }], ask_user: [deny, allow] } },
  });
  try {
    assert.equal(code(await screenshot(harness)), "approval_declined");
    assert.equal(code(await screenshot(harness)), "approval_refused_earlier");
    assert.equal(harness.elicitations.length, 1);
    assert.equal((await asks(harness)).length, 1);
    assert.deepEqual(await approvalAnswers(harness), [
      { channel: "dialog", answer: "refuse" },
      { channel: "session", answer: "refused_earlier" },
    ]);
  } finally {
    await harness.close();
  }
});

test("a client that supports elicitation never gets the dialog, whatever it answers", async () => {
  const harness = await connectServer({
    elicitation: "form",
    onElicit: () => ({ action: "decline" }),
    script: { handlers: { get_window: [{ result: demo }], ask_user: [allow] } },
  });
  try {
    assert.equal(code(await screenshot(harness)), "approval_declined");
    assert.equal(harness.elicitations.length, 1);
    assert.deepEqual(await asks(harness), []);
  } finally {
    await harness.close();
  }
});

test("while the dialog is open, a concurrent call sends nothing to the helper until it is answered", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: other }, { result: demo }],
        ask_user: [allow, { delayMs: 600, result: { answer: "deny" } }],
      },
    },
  });
  try {
    const pressed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "a" } });
    assert.notEqual(pressed.isError, true, textContent(pressed));
    const asking = harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "7", key: "Return" } });
    let dialogOpen: Logged | undefined;
    while (!dialogOpen) {
      await new Promise((resolve) => setTimeout(resolve, 20));
      dialogOpen = (await asks(harness))[1];
    }
    const concurrent = harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "hello" } });
    assert.equal(code(await asking), "approval_declined");
    const typed = await concurrent;
    assert.notEqual(typed.isError, true, textContent(typed));
    const all = await logged(harness);
    const opened = dialogOpen.at ?? 0;
    // Ticket 159: the dialog opens where its ask_user arrived in the log, not at its `at`. The asking call's own
    // app_identity, sent just before its ask_user, can arrive within the same millisecond on a fast machine.
    const dialogId = dialogOpen.id;
    const openedAt = all.findIndex((entry) => entry.method === "ask_user" && entry.id === dialogId);
    assert.ok(openedAt >= 0, "the dialog's ask_user is not in the log");
    const duringDialog = all.slice(openedAt + 1).filter((entry) => (entry.at ?? 0) < opened + 550);
    assert.deepEqual(duringDialog.map((entry) => entry.method), [], "requests reached the helper while the dialog was open");
    assert.ok(all.some((entry) => entry.method === "type_text"), "the concurrent call never ran");
  } finally {
    await harness.close();
  }
});

test("waiting on the dialog past the usual 10-second request timeout does not restart the helper", { timeout: 30_000 }, async () => {
  const harness = await connectServer({
    script: { handlers: { get_window: [{ result: demo }], capture_window: [{ result: shot }], ask_user: [{ delayMs: 11_000, result: { answer: "allow" } }] } },
  });
  try {
    const result = await screenshot(harness);
    assert.notEqual(result.isError, true, textContent(result));
    const pings = (await logged(harness)).filter((entry) => entry.method === "ping");
    assert.equal(pings.length, 1, "the helper was restarted while the user was being asked");
  } finally {
    await harness.close();
  }
});

test("Esc while the dialog is open ends the call as interrupted, and the next call asks to continue", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        ask_user: [{ delayMs: 30, events: [{ event: "stopped", data: { generation: 1 } }], error: { code: "interrupted", message: "stopped" } }, deny],
      },
    },
  });
  try {
    const first = await screenshot(harness);
    assert.equal(code(first), "interrupted");
    const second = await screenshot(harness);
    assert.equal(code(second), "stopped_by_user");
    const asked = await asks(harness);
    assert.equal(asked[1]?.params?.kind, "continue");
  } finally {
    await harness.close();
  }
});

async function stoppedHarness(ask: FakeHandler[], elicitation?: { throws: string }) {
  return connectServer({
    ...(elicitation
      ? {
          elicitation: "form" as const,
          onElicit: () => {
            throw new Error(elicitation.throws);
          },
        }
      : {}),
    script: {
      handlers: {
        list_windows: [{ events: [{ event: "stopped", data: { generation: 3 } }], result: { windows: [] } }],
        get_window: [{ result: demo }],
        capture_window: [{ result: shot }],
        ask_user: ask,
      },
    },
  });
}

test("after Esc, a client without elicitation is asked to continue by the dialog, and an allow resumes", async () => {
  const harness = await stoppedHarness([allow, allow]);
  try {
    assert.equal(code(await harness.client.callTool({ name: "computer_list_windows", arguments: {} })), "interrupted");
    const result = await screenshot(harness);
    assert.notEqual(result.isError, true, textContent(result));
    const asked = await asks(harness);
    assert.deepEqual(asked[0]?.params, { kind: "continue", timeout_ms: ASK_USER_DIALOG_MS });
    assert.equal(asked[1]?.params?.kind, "approve_app");
    const all = await logged(harness);
    const resume = all.find((entry) => entry.method === "resume");
    assert.equal(resume?.params?.generation, 3);
  } finally {
    await harness.close();
  }
});

test("after Esc, declining the continue dialog stays stopped", async () => {
  const harness = await stoppedHarness([deny]);
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const result = await screenshot(harness);
    assert.equal(code(result), "stopped_by_user");
    assert.match(textContent(result), /did not allow it to continue/);
    assert.match(textContent(result), /End this turn and tell the user/);
    assert.equal((await logged(harness)).some((entry) => entry.method === "resume" || entry.method === "get_window"), false);
  } finally {
    await harness.close();
  }
});

test("after Esc, an unanswered continue dialog stays stopped and says the user did not answer", async () => {
  const harness = await stoppedHarness([timedOut]);
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const result = await screenshot(harness);
    assert.equal(code(result), "stopped_by_user");
    assert.match(textContent(result), /did not answer/i);
    assert.match(textContent(result), /End this turn and tell the user/);
  } finally {
    await harness.close();
  }
});

test("after Esc, a continue dialog that cannot be shown stays stopped, says so, and stderr says why", async (t) => {
  const stderr = captureStderr(t);
  const harness = await stoppedHarness([cannotShow]);
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const result = await screenshot(harness);
    assert.equal(code(result), "stopped_by_user");
    assert.match(textContent(result), /could not be asked/i);
    assert.match(textContent(result), /End this turn and tell the user/);
    assert.match(stderr(), /prompt_unavailable/);
  } finally {
    await harness.close();
  }
});

test("after Esc, an elicitInput failure asks to continue through the dialog", async (t) => {
  const stderr = captureStderr(t);
  const harness = await stoppedHarness([allow, allow], { throws: "no one to ask" });
  try {
    await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const result = await screenshot(harness);
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal((await asks(harness))[0]?.params?.kind, "continue");
    assert.match(stderr(), /no one to ask/);
  } finally {
    await harness.close();
  }
});

// Ticket 57: whatever ends the approval question other than the user's allow is a refusal or an
// explicit error. Nothing reaches the app, and no approval is remembered.

function noInputSent(entries: Logged[]): boolean {
  return !entries.some((entry) => ["capture_window", "click", "type_text", "press_key", "activate_window"].includes(entry.method ?? ""));
}

test("the helper crashing while the approval question is open refuses the call and sends nothing to the app", async (t) => {
  const stderr = captureStderr(t);
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        capture_window: [{ result: shot }],
        ask_user: [{ delayMs: 100, exit: true, stderr: "helper died under the dialog" }, deny],
      },
    },
  });
  try {
    const result = await screenshot(harness);
    assert.equal(result.isError, true, textContent(result));
    assert.equal(code(result), "approval_unavailable");
    assert.doesNotMatch(textContent(result), /allowed/i);
    assert.match(stderr(), /helper_crashed/);
    assert.equal(noInputSent(await logged(harness)), true);
    // No approval was remembered: the next call asks again (and the dialog says deny).
    const again = await screenshot(harness);
    assert.equal(code(again), "approval_declined");
    assert.equal((await asks(harness)).length, 2);
    assert.equal(noInputSent(await logged(harness)), true);
  } finally {
    await harness.close();
  }
});

test("the helper not answering the approval question in time ends the call as unavailable, never allowed", { timeout: 110_000 }, async (t) => {
  const stderr = captureStderr(t);
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }],
        capture_window: [{ result: shot }],
        ask_user: [{ delayMs: ASK_USER_TIMEOUT_MS + 5_000, result: { answer: "allow" } }],
      },
    },
  });
  try {
    // The client's own default request timeout (60 s) is shorter than the server's 75 s wait.
    const result = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } }, undefined, { timeout: 100_000 });
    assert.equal(result.isError, true, textContent(result));
    assert.equal(code(result), "approval_unavailable");
    assert.match(stderr(), /helper_timeout/);
    assert.equal(noInputSent(await logged(harness)), true);
  } finally {
    await harness.close();
  }
});

test("disposing the server while the approval question is open ends the call without sending anything to the app", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }],
        capture_window: [{ result: shot }],
        ask_user: [{ delayMs: 10_000, result: { answer: "allow" } }],
      },
    },
  });
  try {
    const asking = harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
    const outcome = asking.then(
      (result) => ({ result }),
      (error: unknown) => ({ error }),
    );
    while ((await asks(harness)).length === 0) await new Promise((resolve) => setTimeout(resolve, 20));
    const started = Date.now();
    await harness.finish();
    assert.ok(Date.now() - started < 5_000, `dispose waited for the open question (${Date.now() - started} ms)`);
    const settled = await outcome;
    if ("result" in settled) {
      assert.equal(settled.result.isError, true, "the call succeeded although the server was disposed");
      assert.notEqual(code(settled.result), "");
    } else {
      assert.ok(settled.error instanceof Error, "the call did not fail with an error");
    }
    // The helper's late "allow" must not have been acted on.
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(noInputSent(await logged(harness)), true);
  } finally {
    await harness.close();
  }
});

test("when session_start fails after an allow, the call errors without sending input, and the allow stands for the retry", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        get_window: [{ result: demo }, { result: demo }],
        ask_user: [allow],
        session_start: [{ error: { code: "internal", message: "the Esc hook could not be installed" } }],
        press_key: [{ result: {} }],
      },
    },
  });
  try {
    const failed = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "a" } });
    assert.equal(failed.isError, true, textContent(failed));
    assert.equal(code(failed), "internal");
    assert.match(textContent(failed), /Esc hook could not be installed/);
    const sent = await logged(harness);
    assert.equal(noInputSent(sent), true, "input was sent although the session did not start");
    assert.equal(sent.filter((entry) => entry.method === "session_start").length, 1);
    // The user's allow is remembered, but the session has not started, so the retry starts it
    // first and only then sends the key; the user is not asked a second time.
    const retried = await harness.client.callTool({ name: "computer_press_key", arguments: { window_id: "5", key: "a" } });
    assert.notEqual(retried.isError, true, textContent(retried));
    const methods = (await logged(harness)).map((entry) => entry.method);
    assert.equal((await asks(harness)).length, 1);
    assert.equal(methods.filter((method) => method === "session_start").length, 2, methods.join(", "));
    assert.ok(methods.lastIndexOf("session_start") < methods.indexOf("press_key"), methods.join(", "));
  } finally {
    await harness.close();
  }
});
