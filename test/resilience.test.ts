import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { test } from "node:test";
import { connectServer, launchScriptedHelper, retryingSpawn, textContent } from "./harness.ts";
import { ToolFailure } from "../src/errors.ts";

const windowResult = {
  windows: [
    {
      window_id: "7",
      app: "C:\\Apps\\recovered.exe",
      app_name: "Recovered",
      title: "Recovered",
      minimized: false,
      elevated: false,
    },
  ],
};

test("a request that runs past 10 seconds returns helper_timeout and the next call recovers", { timeout: 25_000 }, async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [{ delayMs: 11_000, result: { windows: [] } }, { result: windowResult }],
      },
    },
  });
  try {
    const timedOut = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(textContent(timedOut).split(/\r?\n/)[0], "helper_timeout");
    assert.equal(timedOut.isError, true);
    const recovered = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(recovered.isError, true);
    assert.match(textContent(recovered), /Recovered/);
  } finally {
    await harness.close();
  }
});

test("a crashed helper fails the pending request with its stderr and the next call recovers", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [{ exit: true, stderr: "boom line from helper" }, { result: windowResult }],
      },
    },
  });
  try {
    const crashed = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(crashed.isError, true);
    const text = textContent(crashed);
    assert.equal(text.split(/\r?\n/)[0], "helper_crashed");
    assert.match(text, /boom line from helper/);
    const recovered = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(recovered.isError, true);
    assert.match(textContent(recovered), /Recovered/);
  } finally {
    await harness.close();
  }
});

test("a non-json helper line fails that request and the server still answers the next one", { timeout: 25_000 }, async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [{ raw: "this is not json" }, { result: windowResult }],
      },
    },
  });
  try {
    const broken = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(broken.isError, true);
    assert.equal(textContent(broken).split(/\r?\n/)[0], "helper_protocol");
    const recovered = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(recovered.isError, true);
    assert.match(textContent(recovered), /Recovered/);
  } finally {
    await harness.close();
  }
});

test("an incompatible helper version is reported for every tool call", async () => {
  const harness = await connectServer({ script: { version: "11" } });
  try {
    const first = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    const second = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    for (const result of [first, second]) {
      assert.equal(result.isError, true);
      const text = textContent(result);
      assert.equal(text.split(/\r?\n/)[0], "helper_version_mismatch");
      assert.match(text, /"11"/);
      assert.match(text, /"10"/);
    }
  } finally {
    await harness.close();
  }
});

test("type_text waits 30 seconds, past the helper's 20-second typing limit, while other requests time out at 10", { timeout: 45_000 }, async () => {
  const withinBudget = await launchScriptedHelper({
    handlers: { type_text: [{ delayMs: 27_000, result: {} }] },
  });
  const overBudget = await launchScriptedHelper({
    handlers: { type_text: [{ delayMs: 31_000, result: {} }] },
  });
  try {
    const [succeeded, timedOut] = await Promise.allSettled([
      withinBudget.helper.request("type_text", { window_id: "5", text: "long" }),
      overBudget.helper.request("type_text", { window_id: "5", text: "long" }),
    ]);
    assert.equal(succeeded.status, "fulfilled", String(succeeded.status === "rejected" ? succeeded.reason : ""));
    assert.equal(timedOut.status, "rejected");
    assert.ok(timedOut.reason instanceof ToolFailure);
    assert.equal(timedOut.reason.code, "helper_timeout");
  } finally {
    await withinBudget.close();
    await overBudget.close();
  }
});

test("launch_app waits 15 seconds while other requests time out at 10", { timeout: 40_000 }, async () => {
  const withinBudget = await launchScriptedHelper({
    handlers: { launch_app: [{ delayMs: 14_000, result: { started: true } }] },
  });
  const overBudget = await launchScriptedHelper({
    handlers: { launch_app: [{ delayMs: 16_000, result: { started: true } }] },
  });
  try {
    const succeeded = await withinBudget.helper.request("launch_app", {});
    assert.deepEqual(succeeded, { started: true });
    await assert.rejects(overBudget.helper.request("launch_app", {}), (error: unknown) => {
      assert.ok(error instanceof ToolFailure);
      assert.equal(error.code, "helper_timeout");
      return true;
    });
  } finally {
    await withinBudget.close();
    await overBudget.close();
  }
});

// Ticket 45 (F30): with many test files in parallel, Windows occasionally refuses to start the
// fake helper's node process; spawn throws EPERM before any request. The harness retries that.
test("the harness spawn retries a refused start and gives up after a few", () => {
  const perm = () => Object.assign(new Error("spawn EPERM"), { code: "EPERM" });
  let calls = 0;
  const started = retryingSpawn(((..._args: unknown[]) => {
    calls += 1;
    if (calls < 3) throw perm();
    return "child";
  }) as unknown as typeof spawn, () => {});
  assert.equal(started("node", [], {}) as unknown, "child");
  assert.equal(calls, 3);

  calls = 0;
  const other = retryingSpawn((() => {
    calls += 1;
    throw Object.assign(new Error("spawn EINVAL"), { code: "EINVAL" });
  }) as unknown as typeof spawn, () => {});
  assert.throws(() => other("node", [], {}), /EINVAL/);
  assert.equal(calls, 1);

  calls = 0;
  const refused = retryingSpawn((() => {
    calls += 1;
    throw perm();
  }) as unknown as typeof spawn, () => {});
  assert.throws(() => refused("node", [], {}), /EPERM/);
  assert.equal(calls, 5);
});

test("the helper client starts its process through the spawn it is given", async () => {
  let calls = 0;
  const { helper, close } = await launchScriptedHelper({ handlers: { list_windows: [{ result: windowResult }] } }, {
    spawn: ((...args: Parameters<typeof spawn>) => {
      calls += 1;
      return spawn(...args);
    }) as typeof spawn,
  });
  try {
    assert.deepEqual(await helper.request("list_windows", {}), windowResult);
    assert.equal(calls, 1);
  } finally {
    await close();
  }
});
