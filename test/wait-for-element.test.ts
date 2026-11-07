import assert from "node:assert/strict";
import { mkdir, readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { INTERRUPTED_MESSAGE } from "../src/helper-client.ts";
import { connectServer, repoRoot, textContent, type FakeHandler, type Harness, type HarnessOptions } from "./harness.ts";

// Reference-gaps ticket 04 (G2, element conditions): computer_wait_for waits for an element whose name or
// value holds a text to appear or to go. Through the MCP tool call with the scripted helper: a fake is not
// UI Automation, so these prove the server's logic (what it asks the helper, when it is satisfied, what it
// returns); that a poll keeps no element references and that a password box is not searched is the
// helper's own test (helper/tests/ui_tree.rs).

const notes = {
  window_id: "7",
  app: "C:\\Apps\\Notes.exe",
  app_name: "Notes",
  title: "Notes",
  minimized: false,
  elevated: false,
  x: 0,
  y: 0,
  width: 400,
  height: 300,
  pid: 4242,
};

/** What the helper's `find_in_ui_tree` answers: whether a match was found, and why the read was cut short, if it was. */
const found = (truncated?: string[]): FakeHandler => ({ result: { found: true, ...(truncated ? { truncated } : {}) } });
const absent = (truncated?: string[]): FakeHandler => ({ result: { found: false, ...(truncated ? { truncated } : {}) } });
const failing = (code: string, message = "The read failed."): FakeHandler => ({ error: { code, message } });

const element = (index: number, name: string) => ({ depth: 0, index, role: "button", name, bounds: { x: 10, y: 10, width: 100, height: 20 }, states: [], actions: ["click"] });
/** What a normal `get_ui_tree` answers, for the read an `element_appears` that is met makes. */
const treeOf = (treeId: string, ...elements: Record<string, unknown>[]): FakeHandler => ({ result: { tree_id: treeId, window_title: "Notes", elements } });

interface Logged {
  method?: string;
  params?: Record<string, unknown>;
}

const waitFor = (harness: Harness, args: Record<string, unknown>) => harness.client.callTool({ name: "computer_wait_for", arguments: { window_id: "7", ...args } });
const code = (result: { content?: unknown }) => textContent(result).split(/\r?\n/)[0] ?? "";
const requests = async (harness: Harness) => (await harness.requests()) as Logged[];
const methods = async (harness: Harness) => (await requests(harness)).map((entry) => entry.method ?? "").filter((method) => method !== "ping" && method !== "list_apps");
const logged = async (harness: Harness, method: string) => (await requests(harness)).filter((entry) => entry.method === method);
const metAfter = (result: { content?: unknown }) => Number(/^met_after_ms: (\d+)$/m.exec(textContent(result))?.[1]);
/** Starts the helper (a first request takes a few hundred ms), so the timings measured after it are the wait's own. Its answer is an error, which is ignored. */
const warm = async (harness: Harness) => void (await harness.client.callTool({ name: "computer_list_apps", arguments: {} }));
/** The tree reads among the requests, in order. */
const treeReads = async (harness: Harness) => (await methods(harness)).filter((method) => method === "find_in_ui_tree" || method === "get_ui_tree");
/**
 * Every request the helper got is one that reads or that approving and showing the window needs (the lock's
 * process check, the app's identity, the session, the overlay): none sends input or changes the foreground.
 */
const NOT_INPUT = new Set(["process_info", "get_window", "app_identity", "session_start", "overlay_show", "find_in_ui_tree", "get_ui_tree"]);
const assertNoInput = async (harness: Harness) => {
  const sent = (await methods(harness)).filter((method) => !NOT_INPUT.has(method));
  assert.deepEqual(sent, [], "a request that is not a read reached the helper");
};

async function withServer<T>(options: HarnessOptions, body: (harness: Harness) => Promise<T>): Promise<T> {
  const harness = await connectServer({ elicitation: "form", ...options });
  try {
    return await body(harness);
  } finally {
    await harness.close();
  }
}

test("element_gone is met by the first poll that does not find the text; every poll is a find_in_ui_tree and nothing is sent to the window", async () => {
  const script = { handlers: { get_window: [{ result: notes }], find_in_ui_tree: [found(), found(), absent()] } };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "element_gone", text: "Loading" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /element_gone met/);
    assert.ok(metAfter(result) >= 600, `met_after_ms ${metAfter(result)}: three reads, two gaps`);
    assert.deepEqual(result.structuredContent, { condition: "element_gone", met_after_ms: metAfter(result) });
    // Approved like a tree read, then only reads: no get_ui_tree (which would keep a tree), no input.
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree", "find_in_ui_tree", "find_in_ui_tree"]);
    await assertNoInput(harness);
    assert.equal((await logged(harness, "get_window")).length, 1, "the window is looked at once, for the approval");
    assert.equal(harness.elicitations.length, 1);
    for (const read of await logged(harness, "find_in_ui_tree")) assert.deepEqual(read.params, { window_id: "7", expected_app: notes.app, query: "Loading" });
  });
});

const windows = (count: number): FakeHandler[] => Array.from({ length: count }, () => ({ result: notes }));
/**
 * `count` answers of `handler`. The default is more than any wait here polls: a 2.5 second wait whose reads
 * take no time polls nine times (every 300 ms from 0 to 2400), and an unscripted read fails as `internal`,
 * which would then be the last read's note (CI's faster reads met that with 8).
 */
const many = (handler: FakeHandler, count = 20): FakeHandler[] => Array.from({ length: count }, () => handler);

test("the tree_id and numbers the model holds stay valid through more than five polls: none is a get_ui_tree, and the latest tree is still the model's", async () => {
  const held = treeOf("t1-aaaaa", element(1, "Save"), element(2, "Cancel"));
  const script = {
    handlers: { get_window: windows(8), get_ui_tree: [held], find_in_ui_tree: [...many(found(), 7), absent()] },
  };
  await withServer({ script }, async (harness) => {
    assert.notEqual((await harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "7" } })).isError, true);
    const result = await waitFor(harness, { condition: "element_gone", text: "Loading" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal((await logged(harness, "find_in_ui_tree")).length, 8, "eight polls: more than the five trees the helper keeps");
    assert.equal((await logged(harness, "get_ui_tree")).length, 1, "only the model's own read");
    // The model's number, with and without its tree_id, still names its tree.
    for (const extra of [{}, { tree_id: "t1-aaaaa" }]) {
      const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "7", element_index: 2, delivery_mode: "foreground", ...extra } });
      assert.notEqual(clicked.isError, true, textContent(clicked));
    }
    const sent = await logged(harness, "click");
    assert.equal(sent.length, 2);
    for (const click of sent) assert.deepEqual(click.params?.element, { tree_id: "t1-aaaaa", index: 2 });
  });
});

test("a read that was cut short and found nothing does not satisfy element_gone; a whole one does", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: [absent(["nodes"]), absent(["timeout"]), absent()] } };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "element_gone", text: "Loading" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.ok(metAfter(result) >= 600, `met_after_ms ${metAfter(result)}: the two cut-short reads did not count`);
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree", "find_in_ui_tree", "find_in_ui_tree"]);
  });
});

test("element_gone that only ever read cut-short trees times out and says it could not be confirmed", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: many(absent(["timeout"])) } };
  await withServer({ script }, async (harness) => {
    await warm(harness);
    const result = await waitFor(harness, { condition: "element_gone", text: "Loading", timeout_seconds: 2.5 });
    assert.equal(result.isError, true);
    assert.equal(code(result), "wait_timeout");
    assert.match(textContent(result), /element_gone "Loading" in window 7/);
    assert.match(textContent(result), /cut short \(timeout\)/);
    assert.match(textContent(result), /could not be confirmed/);
    assert.match(textContent(result), /No input was sent\./);
  });
});

test("the timeout is wait_timeout naming the condition and the time waited, and quotes nothing but the caller's own text", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: many(found()) } };
  await withServer({ script }, async (harness) => {
    await warm(harness);
    const started = Date.now();
    const result = await waitFor(harness, { condition: "element_gone", text: "Loading", timeout_seconds: 2.5 });
    const took = Date.now() - started;
    assert.equal(result.isError, true);
    assert.equal(code(result), "wait_timeout");
    const text = textContent(result);
    assert.match(text, /element_gone "Loading" in window 7 was not met after \d\.\d seconds\./);
    assert.match(text, /still there/);
    assert.match(text, /No input was sent\./);
    assert.ok(!text.includes("Notes"), "the window's title or app name is quoted");
    assert.ok(took >= 2000 && took < 12_000, `took ${took} ms`);
  });
});

test("element_appears: the first poll that finds the text is followed by one normal read, whose tree and tree_id are returned and become the latest", async () => {
  const script = {
    handlers: { get_window: windows(6), find_in_ui_tree: [absent(), found()], get_ui_tree: [treeOf("t2-aaaaa", element(1, "Save as…"), element(2, "Save"))] },
  };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "element_appears", text: "SaVe" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /^element_appears met\.$/m);
    assert.deepEqual(result.structuredContent, { condition: "element_appears", met_after_ms: metAfter(result), tree_id: "t2-aaaaa" });
    assert.match(textContent(result), /tree_id t2-aaaaa/);
    assert.match(textContent(result), /query "SaVe"/, "the tree of the query, as computer_get_ui_tree gives it");
    assert.match(textContent(result), /\[2\] button "Save"/);
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree", "find_in_ui_tree", "get_ui_tree"]);
    // The text is the caller's, handed on as it is: the match rule is the tree's own, not a second one here.
    const [normal] = await logged(harness, "get_ui_tree");
    assert.deepEqual(normal?.params, { window_id: "7", expected_app: notes.app, query: "SaVe" });
    await assertNoInput(harness);
    // The number from it works, as the window's latest tree.
    const clicked = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "7", element_index: 2, delivery_mode: "foreground" } });
    assert.notEqual(clicked.isError, true, textContent(clicked));
    assert.deepEqual((await logged(harness, "click"))[0]?.params?.element, { tree_id: "t2-aaaaa", index: 2 });
  });
});

test("element_appears whose normal read no longer finds the text goes on polling, and that read's tree is not the window's", async () => {
  const script = {
    handlers: {
      get_window: windows(8),
      find_in_ui_tree: [found(), found()],
      get_ui_tree: [treeOf("t2-aaaaa"), treeOf("t3-aaaaa", element(1, "Save"))],
    },
  };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "element_appears", text: "Save" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal((result.structuredContent as { tree_id?: string }).tree_id, "t3-aaaaa");
    assert.ok(metAfter(result) >= 300, `met_after_ms ${metAfter(result)}: a gap after the read that came back empty`);
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree", "get_ui_tree", "find_in_ui_tree", "get_ui_tree"]);
    // The tree of the read that came back empty was never the model's.
    const stale = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "7", element_index: 1, tree_id: "t2-aaaaa", delivery_mode: "foreground" } });
    assert.equal(code(stale), "no_ui_tree", textContent(stale));
    assert.equal((await logged(harness, "click")).length, 0);
  });
});

test("element_appears is met by a match in a tree that was cut short, and the tree says so", async () => {
  const cut: FakeHandler = { result: { tree_id: "t2-aaaaa", window_title: "Notes", truncated: ["nodes"], elements: [element(1, "Save")] } };
  const script = { handlers: { get_window: windows(2), find_in_ui_tree: [found(["nodes"])], get_ui_tree: [cut] } };
  await withServer({ script }, async (harness) => {
    await warm(harness);
    const result = await waitFor(harness, { condition: "element_appears", text: "Save" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree", "get_ui_tree"], "met at the first read");
    assert.match(textContent(result), /--- end of UI tree \(truncated: nodes\) ---/);
  });
});

test("element_appears that only ever read cut-short trees without the text times out and says it could not be confirmed", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: many(absent(["depth"])) } };
  await withServer({ script }, async (harness) => {
    await warm(harness);
    const result = await waitFor(harness, { condition: "element_appears", text: "Save", timeout_seconds: 2.5 });
    assert.equal(code(result), "wait_timeout");
    assert.match(textContent(result), /element_appears "Save" in window 7/);
    assert.match(textContent(result), /cut short \(depth\)/);
    assert.match(textContent(result), /could not be confirmed/);
    assert.deepEqual(await logged(harness, "get_ui_tree"), [], "no tree was read: nothing was found");
  });
});

test("a window that closes is window_not_found, for either condition and from either read; a read that failed is not", async () => {
  const gone = failing("window_not_found", "The window is no longer open.");
  const cases: [string, Record<string, FakeHandler[]>][] = [
    ["element_gone, in a poll", { find_in_ui_tree: [found(), gone] }],
    ["element_appears, in a poll", { find_in_ui_tree: [absent(), gone] }],
    ["element_appears, in the normal read", { find_in_ui_tree: [found()], get_ui_tree: [gone] }],
    ["before the first poll", { get_window: [gone] }],
  ];
  for (const [label, handlers] of cases) {
    const condition = label.startsWith("element_appears") ? "element_appears" : "element_gone";
    await withServer({ script: { handlers: { get_window: windows(4), ...handlers } } }, async (harness) => {
      const result = await waitFor(harness, { condition, text: "Save", timeout_seconds: 5 });
      assert.equal(code(result), "window_not_found", `${label}: ${textContent(result)}`);
    });
  }
  // Reads that failed for the moment end in a timeout that says they could not be confirmed.
  for (const failure of [failing("internal", "SECRET-DETAIL"), failing("helper_timeout", "Helper did not respond within 10000ms.")]) {
    await withServer({ script: { handlers: { get_window: windows(1), find_in_ui_tree: many(failure) } } }, async (harness) => {
      await warm(harness);
      const result = await waitFor(harness, { condition: "element_gone", text: "Save", timeout_seconds: 2.5 });
      assert.equal(code(result), "wait_timeout", textContent(result));
      assert.match(textContent(result), /could not be confirmed/);
      assert.ok(!textContent(result).includes("SECRET-DETAIL"));
    });
  }
});

test("a read the helper refuses for good ends the wait with that error: a window that became another app's is app_changed", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: [failing("app_changed", "The window now belongs to another app.")] } };
  await withServer({ script }, async (harness) => {
    const result = await waitFor(harness, { condition: "element_gone", text: "Save", timeout_seconds: 5 });
    assert.equal(code(result), "app_changed");
    assert.equal((await logged(harness, "find_in_ui_tree")).length, 1);
  });
});

test("a stop during the gap between polls is interrupted at once, with no further read; so is one that arrives during a read", async () => {
  const gap = { handlers: { get_window: windows(1), find_in_ui_tree: [{ ...found(), eventsAfter: [{ event: "stopped", data: { generation: 1 } }] }] } };
  await withServer({ script: gap }, async (harness) => {
    const started = Date.now();
    const result = await waitFor(harness, { condition: "element_gone", text: "Loading", timeout_seconds: 30 });
    assert.equal(code(result), "interrupted");
    assert.ok(textContent(result).includes(INTERRUPTED_MESSAGE));
    assert.ok(Date.now() - started < 15_000, "the timeout was waited for");
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree"]);
  });
  const during = { handlers: { get_window: windows(1), find_in_ui_tree: [{ delayMs: 30, events: [{ event: "stopped" }], ...absent() }] } };
  await withServer({ script: during }, async (harness) => {
    const result = await waitFor(harness, { condition: "element_appears", text: "Save", timeout_seconds: 30 });
    assert.equal(code(result), "interrupted");
    assert.deepEqual(await treeReads(harness), ["find_in_ui_tree"]);
  });
});

test("the parameters that do not fit are invalid_params before anything reaches the helper, and the text is not quoted back", async () => {
  const cases: Record<string, unknown>[] = [
    { condition: "element_gone", text: "Save" },
    { condition: "element_appears", text: "Save", window_id: "" },
    { condition: "element_appears", text: "Save", window_id: 7 },
    { condition: "element_gone", window_id: "7" },
    { condition: "element_gone", window_id: "7", text: "" },
    { condition: "element_appears", window_id: "7", text: "x".repeat(201) },
    { condition: "element_appears", window_id: "7", text: "SECRET-WORD", timeout_seconds: 31 },
    { condition: "element_gone", window_id: "7", text: "SECRET-WORD", timeout_seconds: 0.05 },
  ];
  await withServer({}, async (harness) => {
    for (const args of cases) {
      const result = await harness.client.callTool({ name: "computer_wait_for", arguments: args });
      assert.equal(code(result), "invalid_params", JSON.stringify(args));
      assert.ok(!textContent(result).includes("SECRET-WORD"), "the model's text is quoted back");
    }
    assert.deepEqual(await methods(harness), []);
    assert.equal(harness.elicitations.length, 0);
  });
});

test("approval: a declined app, a blocked app, an administrator window and a minimized window are refused before any read", async () => {
  const cases: [string, HarnessOptions, string][] = [
    ["declined", { onElicit: () => ({ action: "decline" }), script: { handlers: { get_window: windows(1) } } }, "approval_declined"],
    ["blocked", { script: { handlers: { get_window: [{ result: { ...notes, app: "C:\\Windows\\System32\\consent.exe", app_name: "Consent" } }] } } }, "blocked_app"],
    ["administrator", { script: { handlers: { get_window: [{ result: { ...notes, elevated: true } }] } } }, "elevated_target"],
    ["minimized", { script: { handlers: { get_window: [{ result: { ...notes, minimized: true } }] } } }, "window_minimized"],
  ];
  for (const condition of ["element_gone", "element_appears"]) {
    for (const [label, options, expected] of cases) {
      await withServer(options, async (harness) => {
        const result = await waitFor(harness, { condition, text: "Save", timeout_seconds: 5 });
        assert.equal(code(result), expected, `${label}: ${textContent(result)}`);
        assert.deepEqual(await treeReads(harness), [], label);
        if (label === "blocked" || label === "administrator" || label === "minimized") assert.equal(harness.elicitations.length, 0, `${label} was asked about`);
      });
    }
  }
});

test("an app approved for a tree read is not asked about again by a wait on the same app", async () => {
  const script = { handlers: { get_window: windows(3), get_ui_tree: [treeOf("t1-aaaaa", element(1, "Save"))], find_in_ui_tree: [absent()] } };
  await withServer({ script }, async (harness) => {
    await harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "7" } });
    const result = await waitFor(harness, { condition: "element_gone", text: "Save" });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal(harness.elicitations.length, 1);
  });
});

async function lockDirFor(name: string): Promise<string> {
  const dir = join(repoRoot(), "test-tmp", `wait-for-element-${Date.now()}-${name}`);
  await mkdir(dir, { recursive: true });
  return dir;
}

const lockFile = async (lockDir: string) => (await readdir(lockDir)).includes("session.lock");
const first = { pid: 70001, startedAt: "2026-09-24T01:00:00.000Z" };
const second = { pid: 70002, startedAt: "2026-09-24T02:00:00.000Z" };

test("the element conditions need the operator lock, so another session's holds them off; the window conditions never do", async () => {
  const lockDir = await lockDirFor("held");
  const listing = { result: { windows: [{ window_id: "9", app: notes.app, app_name: "Notes", title: "Save As", minimized: false, elevated: false }] } };
  const holder = await connectServer({ elicitation: "form", lockDir, owner: second, script: { handlers: { get_window: windows(1), get_ui_tree: [treeOf("t1-aaaaa", element(1, "Save"))] } } });
  const other = await connectServer({
    elicitation: "form",
    lockDir,
    owner: first,
    script: { handlers: { get_window: windows(4), list_windows: many(listing), find_in_ui_tree: many(absent()), process_info: [{ result: { alive: true, started_ms: Date.parse(second.startedAt) } }] } },
  });
  try {
    assert.notEqual((await holder.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "7" } })).isError, true);
    const free = await waitFor(other, { condition: "window_appears", text: "save as" });
    assert.notEqual(free.isError, true, textContent(free));
    for (const condition of ["element_gone", "element_appears"]) {
      const blocked = await waitFor(other, { condition, text: "Save" });
      assert.equal(code(blocked), "in_use", textContent(blocked));
    }
    const batch = await other.client.callTool({ name: "computer_batch", arguments: { steps: [{ action: "wait_for", condition: "window_appears", text: "save as" }, { action: "wait_for", condition: "element_gone", window_id: "7", text: "Save" }] } });
    assert.equal(code(batch), "in_use", textContent(batch));
    assert.equal((await logged(other, "find_in_ui_tree")).length, 0);
    assert.equal(other.elicitations.length, 0);
  } finally {
    await holder.close();
    await other.close();
  }
});

test("in a batch the lock is taken for an element condition and for no window condition", async () => {
  const listing = { result: { windows: [{ window_id: "9", app: notes.app, app_name: "Notes", title: "Save As", minimized: false, elevated: false }] } };
  const script = { handlers: { get_window: windows(2), list_windows: many(listing), find_in_ui_tree: [absent()] } };
  await withServer({ script }, async (harness) => {
    const lockDir = join(harness.dir, "lock");
    const windowOnly = await harness.client.callTool({ name: "computer_batch", arguments: { steps: [{ action: "wait_for", condition: "window_appears", text: "save as" }, { action: "wait", seconds: 0.1 }] } });
    assert.notEqual(windowOnly.isError, true, textContent(windowOnly));
    assert.equal(await lockFile(lockDir).catch(() => false), false, "the lock was taken for window conditions");
    const withElement = await harness.client.callTool({ name: "computer_batch", arguments: { steps: [{ action: "wait_for", condition: "element_gone", window_id: "7", text: "Save" }, { action: "wait", seconds: 0.1 }] } });
    assert.notEqual(withElement.isError, true, textContent(withElement));
    assert.match(textContent(withElement), /completed 2 of 2 steps/);
    assert.equal(await lockFile(lockDir), true, "the lock was not taken for an element condition");
    assert.equal(harness.elicitations.length, 1);
  });
});

test("a timeout of an element step stops the batch like any failed step", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: many(found()) } };
  await withServer({ script }, async (harness) => {
    const result = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "wait_for", condition: "element_gone", window_id: "7", text: "Save", timeout_seconds: 0.5 }, { action: "wait", seconds: 0.1 }] },
    });
    const lines = textContent(result).split(/\r?\n/);
    assert.equal(result.isError, true);
    assert.equal(lines[0], "wait_timeout");
    assert.equal(lines[1], "stopped at step 1 of 2: wait_timeout");
    assert.match(lines[2] ?? "", /^\[1\] wait_for: wait_timeout \(sent: false\) /);
    assert.equal(lines[3], "[2] wait: not run");
  });
});

test("waiting counts as lock activity: the idle release does not come during a wait, and comes after it", async () => {
  const lockDir = await lockDirFor("idle");
  const readLock = async () => {
    try {
      return JSON.parse(await readFile(join(lockDir, "session.lock"), "utf8")) as Record<string, unknown>;
    } catch {
      return null;
    }
  };
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: many(found(), 12) } };
  // An idle window a third of the wait: a session whose wait did not count would have let go of the lock long before its end.
  await withServer({ lockDir, owner: first, lockIdleMs: 300, overlayIdleMs: 60_000, script }, async (harness) => {
    const waiting = waitFor(harness, { condition: "element_gone", text: "Save", timeout_seconds: 1.5 });
    await new Promise((resolve) => setTimeout(resolve, 1100));
    assert.equal((await readLock())?.pid, first.pid, "the lock was released during the wait");
    assert.ok(!(await methods(harness)).includes("session_end"), "the session was ended during the wait");
    assert.equal(code(await waiting), "wait_timeout");
    const until = Date.now() + 5_000;
    while ((await readLock()) !== null) {
      if (Date.now() > until) assert.fail("the lock was not released after the wait");
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  });
});

test("the log records the call and the approved app, and nothing the caller or the window said", async () => {
  const script = { handlers: { get_window: windows(1), find_in_ui_tree: many(found()) } };
  await withServer({ script }, async (harness) => {
    await waitFor(harness, { condition: "element_gone", text: "CALLER-TEXT", timeout_seconds: 0.5 });
    await harness.finish();
    const dir = join(harness.dir, "logs");
    const raw = (await Promise.all((await readdir(dir)).map((name) => readFile(join(dir, name), "utf8")))).join("\n");
    const events = raw.split("\n").filter(Boolean).map((line) => JSON.parse(line) as Record<string, unknown>);
    const call = events.find((event) => event.event === "tool_call" && event.tool === "computer_wait_for");
    assert.ok(call, raw);
    assert.equal(call.result, "wait_timeout");
    assert.equal((call.app as { path?: string } | null)?.path, notes.app);
    assert.ok(!raw.includes("CALLER-TEXT"));
  });
});
