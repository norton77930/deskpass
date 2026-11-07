import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { ASK_USER_DIALOG_MS } from "../src/config.ts";
import { judgeSystemKeys, normalizedCombination, parseChord, SYSTEM_KEY_ENTRIES } from "../src/keys.ts";
import { approvalEvents, connectServer, repoRoot, textContent, type FakeHandler, type Harness, type HarnessOptions } from "./harness.ts";

// Reference-gaps ticket 01 (G1): the key combinations whose effect reaches beyond the approved app
// wait for the user's consent in the helper's Windows dialog, per app and combination, for this
// server session. The judgement is tested by input and output; the rest through the MCP tool call
// with the scripted helper.

const judge = (chord: string) => judgeSystemKeys(parseChord(chord) ?? assert.fail(`${chord} does not parse`));

test("every listed combination is dangerous and named by its own entry", () => {
  assert.deepEqual([...SYSTEM_KEY_ENTRIES].sort(), ["alt+escape", "alt+f11", "alt+f4", "alt+f8", "alt+tab", "ctrl+alt+delete", "ctrl+alt+escape", "ctrl+alt+tab", "ctrl+escape", "ctrl+shift+escape"]);
  for (const entry of SYSTEM_KEY_ENTRIES) assert.deepEqual(judge(entry), { dangerous: true, entry });
});

test("any combination with the Win key is dangerous, the Win key alone too, under any of its names", () => {
  for (const chord of ["win", "WIN", "meta", "super"]) assert.deepEqual(judge(chord), { dangerous: true, entry: "win" }, chord);
  for (const chord of ["win+l", "meta+L", "r+super", "ctrl+win+d", "win+shift+s", "win+v", "win+tab"]) assert.deepEqual(judge(chord), { dangerous: true, entry: "win+*" }, chord);
});

test("order and aliases do not change the judgement", () => {
  const cases: [string, string][] = [
    ["f4+alt", "alt+f4"],
    ["ALT+F4", "alt+f4"],
    ["tab+alt", "alt+tab"],
    ["esc+alt", "alt+escape"],
    ["control+alt+del", "ctrl+alt+delete"],
    ["del+alt+control", "ctrl+alt+delete"],
    ["control+esc", "ctrl+escape"],
    ["esc+shift+control", "ctrl+shift+escape"],
    ["esc+alt+control", "ctrl+alt+escape"],
    ["F8+Alt", "alt+f8"],
    [" alt + f11 ", "alt+f11"],
  ];
  for (const [chord, entry] of cases) assert.deepEqual(judge(chord), { dangerous: true, entry }, chord);
});

test("a superset of a listed combination is dangerous, named by the most specific entry it holds", () => {
  const cases: [string, string][] = [
    ["shift+alt+f4", "alt+f4"],
    ["ctrl+alt+f4", "alt+f4"],
    ["alt+shift+tab", "alt+tab"],
    ["ctrl+alt+shift+tab", "ctrl+alt+tab"],
    ["ctrl+alt+f11", "alt+f11"],
    ["ctrl+alt+shift+delete", "ctrl+alt+delete"],
    ["alt+shift+escape", "alt+escape"],
    ["ctrl+alt+a+delete", "ctrl+alt+delete"],
  ];
  for (const [chord, entry] of cases) assert.deepEqual(judge(chord), { dangerous: true, entry }, chord);
  assert.equal(judge("ctrl+alt+shift+escape").dangerous, true);
});

test("ordinary shortcuts and the parts of a listed combination are not dangerous", () => {
  for (const chord of ["ctrl+s", "ctrl+c", "ctrl+v", "alt+f", "shift+Tab", "return", "f5", "f4", "f8", "f11", "tab", "escape", "delete", "alt", "ctrl", "shift", "ctrl+tab", "ctrl+shift+tab", "ctrl+f4", "shift+f4", "shift+escape", "ctrl+delete", "ctrl+alt+a", "ctrl+shift+f11"]) {
    assert.deepEqual(judge(chord), { dangerous: false, entry: null }, chord);
  }
});

test("the normalized combination does not depend on order or aliases, and keeps a superset apart", () => {
  const combination = (chord: string) => normalizedCombination(parseChord(chord) ?? assert.fail(chord));
  assert.equal(combination("F4+ALT"), "alt+f4");
  assert.equal(combination("f4+shift+alt"), "alt+shift+f4");
  assert.equal(combination("del+alt+control"), "ctrl+alt+delete");
  assert.equal(combination("L+meta"), "win+l");
  assert.equal(combination("super"), "win");
  assert.equal(combination("win+shift+alt+ctrl+s"), "ctrl+alt+shift+win+s");
  for (const entry of SYSTEM_KEY_ENTRIES) assert.equal(combination(entry), entry, "an entry is in the normal form");
});

const demo = { window_id: "5", app: "C:\\Apps\\Demo.exe", app_name: "Demo", title: "Demo", minimized: false, elevated: false, x: 0, y: 0, width: 200, height: 100, pid: 4242 };
const notes = { ...demo, window_id: "6", app: "C:\\Apps\\Notes.exe", app_name: "Notes", title: "Notes", pid: 4343 };

const answer = (value: string): FakeHandler => ({ result: { answer: value } });
const allow = answer("allow");
const deny = answer("deny");
const timedOut = answer("timeout");
const cannotShow: FakeHandler = { error: { code: "prompt_unavailable", message: "There is no interactive desktop." } };
const windows = (count: number, window = demo): FakeHandler[] => Array.from({ length: count }, () => ({ result: window }));

/** What tells the helper to touch the desktop for a key press, a click or a drag. */
const ACTING = ["overlay_show", "activate_window", "press_key", "click", "drag"];

interface Logged {
  method?: string;
  params?: Record<string, unknown>;
}

const press = (harness: Harness, key: string, windowId = "5") => harness.client.callTool({ name: "computer_press_key", arguments: { window_id: windowId, key } });

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

async function methods(harness: Harness): Promise<string[]> {
  return ((await harness.requests()) as Logged[]).map((entry) => entry.method ?? "");
}

async function acting(harness: Harness): Promise<string[]> {
  return (await methods(harness)).filter((method) => ACTING.includes(method));
}

async function asks(harness: Harness): Promise<Record<string, unknown>[]> {
  return ((await harness.requests()) as Logged[]).filter((entry) => entry.method === "ask_user").map((entry) => entry.params ?? {});
}

async function systemKeyAsks(harness: Harness): Promise<Record<string, unknown>[]> {
  return (await asks(harness)).filter((params) => params.kind === "system_key");
}

async function pressedKeys(harness: Harness): Promise<unknown[]> {
  return ((await harness.requests()) as Logged[]).filter((entry) => entry.method === "press_key").map((entry) => entry.params?.keys);
}

async function systemKeyEvents(harness: Harness) {
  return (await approvalEvents(harness)).filter((event) => event.subject === "system_key").map(({ combination, channel, answer }) => ({ combination, channel, answer }));
}

/** The app was asked about through the elicitation, so every `ask_user` below is the combination's. */
async function withHarness(options: HarnessOptions, run: (harness: Harness) => Promise<void>): Promise<void> {
  const harness = await connectServer({ elicitation: "form", ...options });
  try {
    await run(harness);
  } finally {
    await harness.close();
  }
}

test("ordinary shortcuts go through without any dialog", async () => {
  await withHarness({ script: { handlers: { get_window: windows(4) } } }, async (harness) => {
    for (const key of ["ctrl+s", "ctrl+c", "alt+f", "shift+Tab"]) {
      const result = await press(harness, key);
      assert.notEqual(result.isError, true, `${key}: ${textContent(result)}`);
    }
    assert.deepEqual(await asks(harness), []);
    assert.deepEqual(await systemKeyEvents(harness), []);
    assert.equal(harness.elicitations.length, 1, "only the app is asked about");
  });
});

test("a refusal sends nothing, is approval_declined, and is remembered as approval_refused_earlier", async () => {
  await withHarness({ script: { handlers: { get_window: windows(2), ask_user: [deny] } } }, async (harness) => {
    const refused = await press(harness, "alt+f4");
    assert.equal(refused.isError, true);
    assert.equal(code(refused), "approval_declined");
    assert.match(textContent(refused), /alt\+f4/);
    assert.match(textContent(refused), /Demo/);
    assert.match(textContent(refused), /Report this to the user instead of retrying/);
    const again = await press(harness, "f4+ALT");
    assert.equal(again.isError, true);
    assert.equal(code(again), "approval_refused_earlier");
    assert.match(textContent(again), /restart/i);
    assert.match(textContent(again), /Report this to the user instead of retrying/);
    assert.deepEqual(await acting(harness), [], "no overlay, no activation and no input");
    assert.deepEqual(await systemKeyAsks(harness), [{ kind: "system_key", app_name: "Demo", app_path: "C:\\Apps\\Demo.exe", combination: "alt+f4", timeout_ms: ASK_USER_DIALOG_MS }]);
    assert.deepEqual(await systemKeyEvents(harness), [
      { combination: "alt+f4", channel: "dialog", answer: "refuse" },
      { combination: "alt+f4", channel: "session", answer: "refused_earlier" },
    ]);
  });
});

test("the app is approved first, then the combination, and only then the overlay, the activation and the keys", async () => {
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [allow] } } }, async (harness) => {
    const result = await press(harness, "alt+f4");
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /Pressed alt\+f4/);
    assert.equal(harness.elicitations.length, 1);
    assert.doesNotMatch(harness.elicitations[0] ?? "", /alt\+f4/, "the combination is never asked through the client");
    assert.deepEqual((await methods(harness)).filter((method) => ["get_window", "ask_user", ...ACTING].includes(method)), ["get_window", "ask_user", "overlay_show", "activate_window", "press_key"]);
    assert.deepEqual(await pressedKeys(harness), [["alt", "f4"]]);
  });
});

test("a key that does not parse is invalid_params before anything is asked", async () => {
  await withHarness({ script: { handlers: {} } }, async (harness) => {
    assert.equal(code(await press(harness, "alt+f4+nope")), "invalid_params");
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual(await asks(harness), []);
  });
});

test("an app the user refused is refused as an app, without asking about the combination", async () => {
  await withHarness({ onElicit: () => ({ action: "decline" }), script: { handlers: { get_window: windows(1) } } }, async (harness) => {
    assert.equal(code(await press(harness, "alt+f4")), "approval_declined");
    assert.deepEqual(await asks(harness), []);
    assert.deepEqual(await acting(harness), []);
    assert.deepEqual(await systemKeyEvents(harness), []);
  });
});

test("a consent holds for that combination in that app, however it is written, and for nothing else", async () => {
  await withHarness({ script: { handlers: { get_window: [...windows(3), ...windows(1, notes), ...windows(1)], ask_user: [allow, deny, deny, deny] } } }, async (harness) => {
    assert.notEqual((await press(harness, "alt+f4")).isError, true);
    const rewritten = await press(harness, "F4+ALT");
    assert.notEqual(rewritten.isError, true, textContent(rewritten));
    assert.equal((await systemKeyAsks(harness)).length, 1, "the same combination is not asked again");

    assert.equal(code(await press(harness, "alt+shift+f4")), "approval_declined", "a superset is another combination");
    assert.equal(code(await press(harness, "alt+f4", "6")), "approval_declined", "another app is asked again");
    assert.equal(code(await press(harness, "alt+f8")), "approval_declined", "another combination is asked again");

    assert.deepEqual(
      (await systemKeyAsks(harness)).map(({ app_name, combination }) => ({ app_name, combination })),
      [
        { app_name: "Demo", combination: "alt+f4" },
        { app_name: "Demo", combination: "alt+shift+f4" },
        { app_name: "Notes", combination: "alt+f4" },
        { app_name: "Demo", combination: "alt+f8" },
      ],
    );
    assert.deepEqual(await pressedKeys(harness), [["alt", "f4"], ["f4", "alt"]], "the keys are sent as they were asked for");
    assert.deepEqual((await systemKeyEvents(harness)).slice(0, 2), [
      { combination: "alt+f4", channel: "dialog", answer: "allow_once" },
      { combination: "alt+f4", channel: "session", answer: "allow_once" },
    ]);
  });
});

test("the event of a combination names the app and the combination", async () => {
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [allow] } } }, async (harness) => {
    await press(harness, "meta+L");
    const event = (await approvalEvents(harness)).find((entry) => entry.subject === "system_key");
    assert.deepEqual(event?.app, { path: "C:\\Apps\\Demo.exe", key: "c:\\apps\\demo.exe", tier: "normal" });
    assert.equal(event?.combination, "win+l");
    assert.equal(event?.channel, "dialog");
    assert.equal(event?.answer, "allow_once");
  });
});

test("an unanswered question is approval_declined and is not remembered", async () => {
  await withHarness({ script: { handlers: { get_window: windows(2), ask_user: [timedOut, allow] } } }, async (harness) => {
    const unanswered = await press(harness, "win+r");
    assert.equal(code(unanswered), "approval_declined");
    assert.match(textContent(unanswered), /did not answer/i);
    assert.match(textContent(unanswered), /Report this to the user instead of retrying/);
    assert.deepEqual(await acting(harness), []);
    assert.notEqual((await press(harness, "win+r")).isError, true);
    assert.equal((await systemKeyAsks(harness)).length, 2);
    assert.deepEqual(await systemKeyEvents(harness), [
      { combination: "win+r", channel: "dialog", answer: "timeout" },
      { combination: "win+r", channel: "dialog", answer: "allow_once" },
    ]);
  });
});

test("a dialog that cannot be shown is approval_unavailable, never falls back to the client, and is not remembered", async () => {
  await withHarness({ script: { handlers: { get_window: windows(2), ask_user: [cannotShow, allow] } } }, async (harness) => {
    const unavailable = await press(harness, "alt+tab");
    assert.equal(code(unavailable), "approval_unavailable");
    assert.match(textContent(unavailable), /Report this to the user instead of retrying/);
    assert.equal(harness.elicitations.length, 1, "only the app was asked through the client");
    assert.deepEqual(await acting(harness), []);
    assert.notEqual((await press(harness, "alt+tab")).isError, true);
    assert.equal((await systemKeyAsks(harness)).length, 2);
    assert.deepEqual(await systemKeyEvents(harness), [
      { combination: "alt+tab", channel: "dialog", answer: "unavailable" },
      { combination: "alt+tab", channel: "dialog", answer: "allow_once" },
    ]);
  });
});

test("an answer the two-button dialog cannot give is no allow, and is not remembered", async () => {
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [answer("allow_always")] } } }, async (harness) => {
    assert.equal(code(await press(harness, "alt+f4")), "approval_unavailable");
    assert.deepEqual(await acting(harness), []);
    assert.deepEqual(await systemKeyEvents(harness), [{ combination: "alt+f4", channel: "dialog", answer: "unavailable" }]);
  });
});

test("after a stop the call is interrupted and the user is not asked", async () => {
  await withHarness(
    {
      script: {
        handlers: {
          get_window: [{ result: demo }, { result: demo, events: [{ event: "stopped", data: { generation: 1 } }] }],
        },
      },
    },
    async (harness) => {
      assert.notEqual((await press(harness, "ctrl+s")).isError, true);
      const stopped = await press(harness, "alt+f4");
      assert.equal(code(stopped), "interrupted");
      assert.deepEqual(await asks(harness), []);
      assert.deepEqual(await pressedKeys(harness), [["ctrl", "s"]]);
      assert.deepEqual(await systemKeyEvents(harness), []);
    },
  );
});

test("a stop while the question is open is interrupted and is not remembered as a refusal", async () => {
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [{ error: { code: "interrupted", message: "The user stopped it. End this turn." } }] } } }, async (harness) => {
    assert.equal(code(await press(harness, "alt+f4")), "interrupted");
    assert.deepEqual(await acting(harness), []);
    assert.deepEqual(await systemKeyEvents(harness), []);
  });
});

test("a batch step goes through the same check: refused, it stops the batch and the later steps do not run", async () => {
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [deny] } } }, async (harness) => {
    const result = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "press_key", window_id: "5", key: "alt+f4" }, { action: "press_key", window_id: "5", key: "ctrl+s" }] },
    });
    assert.equal(result.isError, true);
    const text = textContent(result);
    assert.match(text, /\[1\] press_key: approval_declined \(sent: false\) /);
    assert.match(text, /\[2\] press_key: not run/);
    assert.deepEqual(await acting(harness), []);
  });
});

test("a batch step with the user's consent runs, and the consent holds for the steps after it", async () => {
  await withHarness({ script: { handlers: { get_window: windows(2), ask_user: [allow] } } }, async (harness) => {
    const result = await harness.client.callTool({
      name: "computer_batch",
      arguments: { steps: [{ action: "press_key", window_id: "5", key: "alt+tab" }, { action: "press_key", window_id: "5", key: "tab+alt" }] },
    });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal((await systemKeyAsks(harness)).length, 1);
    assert.deepEqual(await pressedKeys(harness), [["alt", "tab"], ["tab", "alt"]]);
  });
});

const SIGNED: FakeHandler = { result: { signed: true, signer: "CN=Contoso", file_name: "demo.exe", original_name: "demo.exe" } };

test("an always-allowed app is still asked, and the consent is never written to the grants", async () => {
  // An app that can be always allowed is asked in the dialog, whatever the client declares.
  await mkdir(join(repoRoot(), "test-tmp"), { recursive: true });
  const grantsDir = await mkdtemp(join(repoRoot(), "test-tmp", "system-key-"));
  try {
    const handlers = () => ({ get_window: windows(4), app_identity: Array.from({ length: 4 }, () => SIGNED) });
    await withHarness({ grantsDir, script: { handlers: { ...handlers(), ask_user: [answer("allow_always"), allow] } } }, async (harness) => {
      assert.notEqual((await press(harness, "ctrl+s")).isError, true);
      assert.notEqual((await press(harness, "alt+f4")).isError, true);
      assert.deepEqual((await asks(harness)).map(({ kind }) => kind), ["approve_app", "system_key"]);
      assert.deepEqual(await pressedKeys(harness), [["ctrl", "s"], ["alt", "f4"]]);
    });
    const grants = await readFile(join(grantsDir, "grants.json"), "utf8");
    assert.match(grants, /signer:CN=Contoso\|demo\.exe/);
    assert.doesNotMatch(grants, /alt\+|system_key|combination/);

    // A new server session: the app is still always allowed, the consent is gone.
    await withHarness({ grantsDir, script: { handlers: { ...handlers(), ask_user: [deny] } } }, async (harness) => {
      assert.notEqual((await press(harness, "ctrl+s")).isError, true);
      assert.equal(code(await press(harness, "alt+f4")), "approval_declined");
      assert.deepEqual((await asks(harness)).map(({ kind }) => kind), ["system_key"]);
    });
  } finally {
    await rm(grantsDir, { recursive: true, force: true });
  }
});

test("a consent writes no grants file for an app that only has the session's approval", async () => {
  await withHarness({ script: { handlers: { get_window: windows(1), ask_user: [allow] } } }, async (harness) => {
    assert.notEqual((await press(harness, "win")).isError, true);
    assert.equal(existsSync(join(harness.dir, "grants.json")), false);
  });
});

// Reference-gaps ticket 02 (G1): the Win key held through `modifiers` goes through the consent of
// computer_press_key, under the same combination, so the answers are shared.

const picture = { image_base64: "QQ==", original_width: 200, original_height: 100, window_x: 0, window_y: 0, window_width: 200, window_height: 100 };
const tree = { tree_id: "t1-aaaaa", window_title: "Demo", elements: [{ depth: 0, index: 1, role: "button", name: "OK", bounds: { x: 10, y: 10, width: 60, height: 20 }, states: [], actions: ["click", "invoke"] }] };

interface PointerCall {
  label: string;
  name: string;
  method: string;
  args: Record<string, unknown>;
}

const CLICK_AT_POINT: PointerCall = { label: "a click at a point", name: "computer_click", method: "click", args: { window_id: "5", x: 20, y: 30, delivery_mode: "foreground" } };
const CLICK_ON_ELEMENT: PointerCall = { label: "a click on an element", name: "computer_click", method: "click", args: { window_id: "5", element_index: 1, delivery_mode: "foreground" } };
const DRAG: PointerCall = { label: "a drag", name: "computer_drag", method: "drag", args: { window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50 } };
const POINTER_CALLS = [CLICK_AT_POINT, CLICK_ON_ELEMENT, DRAG];

const hold = (harness: Harness, call: PointerCall, modifiers: string[]) => harness.client.callTool({ name: call.name, arguments: { ...call.args, modifiers } });

/**
 * What the pointer tools need: a screenshot and a tree of the approved app. `since` is what the
 * helper was asked after them, and a `get_window` in `handlers` is what it answers after them.
 */
async function withPreparedWindow(handlers: Record<string, FakeHandler[]>, run: (harness: Harness, since: () => Promise<string[]>) => Promise<void>): Promise<void> {
  const script = { handlers: { capture_window: [{ result: picture }], get_ui_tree: [{ result: tree }], ...handlers, get_window: [...windows(2), ...(handlers.get_window ?? windows(20))] } };
  await withHarness({ script }, async (harness) => {
    assert.notEqual((await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } })).isError, true);
    assert.notEqual((await harness.client.callTool({ name: "computer_get_ui_tree", arguments: { window_id: "5" } })).isError, true);
    const before = (await methods(harness)).length;
    await run(harness, async () => (await methods(harness)).slice(before));
  });
}

for (const call of POINTER_CALLS) {
  test(`${call.label} holding win: a refusal sends nothing, is remembered, and names win and the app`, async () => {
    await withPreparedWindow({ ask_user: [deny] }, async (harness, since) => {
      const refused = await hold(harness, call, ["win"]);
      assert.equal(refused.isError, true);
      assert.equal(code(refused), "approval_declined");
      assert.match(textContent(refused), /\bwin\b/);
      assert.match(textContent(refused), /Demo/);
      assert.match(textContent(refused), /nothing was sent/i);
      assert.match(textContent(refused), /Report this to the user instead of retrying/);
      const again = await hold(harness, call, ["win"]);
      assert.equal(code(again), "approval_refused_earlier");
      assert.match(textContent(again), /nothing was sent/i);
      assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), [], "no overlay, no activation and no input");
      assert.deepEqual(await systemKeyAsks(harness), [{ kind: "system_key", app_name: "Demo", app_path: "C:\\Apps\\Demo.exe", combination: "win", timeout_ms: ASK_USER_DIALOG_MS }]);
      assert.deepEqual(await systemKeyEvents(harness), [
        { combination: "win", channel: "dialog", answer: "refuse" },
        { combination: "win", channel: "session", answer: "refused_earlier" },
      ]);
    });
  });

  test(`${call.label} holding win: the combination is asked after the app's approval, and only then the overlay, the activation and the input`, async () => {
    await withPreparedWindow({ ask_user: [allow] }, async (harness, since) => {
      const result = await hold(harness, call, ["win"]);
      assert.notEqual(result.isError, true, textContent(result));
      assert.deepEqual((await since()).filter((method) => ["get_window", "ask_user", ...ACTING].includes(method)), ["get_window", "ask_user", "get_window", "overlay_show", "activate_window", call.method]);
      const sent = ((await harness.requests()) as Logged[]).find((entry) => entry.method === call.method);
      assert.deepEqual(sent?.params?.modifiers, ["win"]);
      assert.deepEqual(await systemKeyEvents(harness), [{ combination: "win", channel: "dialog", answer: "allow_once" }]);
    });
  });

  test(`${call.label} with only ctrl, shift and alt shows no dialog`, async () => {
    await withPreparedWindow({}, async (harness, since) => {
      const result = await hold(harness, call, ["ctrl", "shift", "alt"]);
      assert.notEqual(result.isError, true, textContent(result));
      assert.deepEqual(await asks(harness), []);
      assert.deepEqual(await systemKeyEvents(harness), []);
      assert.ok((await since()).includes(call.method));
    });
  });

  test(`${call.label} holding win: the answer is shared with computer_press_key, in both directions`, async () => {
    await withPreparedWindow({ ask_user: [allow] }, async (harness) => {
      assert.notEqual((await press(harness, "win")).isError, true);
      const held = await hold(harness, call, ["win"]);
      assert.notEqual(held.isError, true, textContent(held));
      assert.equal((await systemKeyAsks(harness)).length, 1, "a consent from computer_press_key holds");
    });
    await withPreparedWindow({ ask_user: [deny] }, async (harness, since) => {
      assert.equal(code(await press(harness, "meta")), "approval_declined");
      assert.equal(code(await hold(harness, call, ["win"])), "approval_refused_earlier", "a refusal from computer_press_key holds");
      assert.equal((await systemKeyAsks(harness)).length, 1);
      assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
    });
    await withPreparedWindow({ ask_user: [allow] }, async (harness) => {
      assert.notEqual((await hold(harness, call, ["win"])).isError, true);
      const pressed = await press(harness, "win");
      assert.notEqual(pressed.isError, true, textContent(pressed));
      assert.equal((await systemKeyAsks(harness)).length, 1, "a consent from this call holds for computer_press_key");
    });
    await withPreparedWindow({ ask_user: [deny] }, async (harness) => {
      assert.equal(code(await hold(harness, call, ["win"])), "approval_declined");
      assert.equal(code(await press(harness, "win")), "approval_refused_earlier", "a refusal from this call holds for computer_press_key");
      assert.deepEqual(await pressedKeys(harness), []);
    });
  });

  test(`${call.label}: the modifiers are the combination, under any name and in any order, and more of them is another combination`, async () => {
    await withPreparedWindow({ ask_user: [allow, deny] }, async (harness, since) => {
      assert.notEqual((await hold(harness, call, ["META"])).isError, true);
      assert.notEqual((await hold(harness, call, ["super"])).isError, true);
      assert.equal(code(await hold(harness, call, ["win", "Control"])), "approval_declined", "a consent for win does not cover ctrl+win");
      assert.equal(code(await hold(harness, call, ["ctrl", "meta"])), "approval_refused_earlier");
      assert.equal(code(await press(harness, "ctrl+win")), "approval_refused_earlier", "computer_press_key's ctrl+win is the same combination");
      assert.deepEqual((await systemKeyAsks(harness)).map(({ combination }) => combination), ["win", "ctrl+win"]);
      assert.equal((await since()).filter((method) => method === call.method).length, 2);
    });
  });
}

for (const [label, args] of [
  ["a click at a point", { x: 20, y: 30 }],
  ["a click on an element", { element_index: 1 }],
] as const) {
  for (const delivery of [{}, { delivery_mode: "background" }]) {
    test(`${label} in the background (${JSON.stringify(delivery)}) holding win stays needs_pointer, with no dialog`, async () => {
      await withPreparedWindow({ ask_user: [allow] }, async (harness, since) => {
        const refused = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", ...args, ...delivery, modifiers: ["win"] } });
        assert.equal(code(refused), "background_unavailable");
        assert.deepEqual(refused.structuredContent, { reason: "needs_pointer", recommended: "foreground" });
        assert.deepEqual(await asks(harness), [], "the refusal is not preceded by a dialog");
        assert.deepEqual(await systemKeyEvents(harness), []);
        assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
        // Only the foreground retry asks.
        const retried = await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", ...args, delivery_mode: "foreground", modifiers: ["win"] } });
        assert.notEqual(retried.isError, true, textContent(retried));
        assert.equal((await systemKeyAsks(harness)).length, 1);
      });
    });
  }
}

test("while the user has background delivery off, a click holding win is delivered in the foreground, so it asks", async () => {
  await withPreparedWindow({ ask_user: [deny] }, async (harness, since) => {
    await writeFile(join(harness.dir, "settings.json"), JSON.stringify({ allow_background: false }));
    assert.equal(code(await harness.client.callTool({ name: "computer_click", arguments: { window_id: "5", x: 20, y: 30, modifiers: ["win"] } })), "approval_declined");
    assert.equal((await systemKeyAsks(harness)).length, 1);
    assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
  });
});

test("a call refused without the user is refused before the question: a stale screenshot, an unknown modifier", async () => {
  await withPreparedWindow({ get_window: Array.from({ length: 2 }, () => ({ result: { ...demo, width: 300 } })) }, async (harness) => {
    assert.equal(code(await hold(harness, CLICK_AT_POINT, ["win"])), "stale_screenshot");
    assert.equal(code(await hold(harness, DRAG, ["win"])), "stale_screenshot");
    for (const call of POINTER_CALLS) assert.equal(code(await hold(harness, call, ["win", "hyper"])), "invalid_params");
    assert.deepEqual(await asks(harness), []);
  });
});

for (const call of [CLICK_AT_POINT, DRAG]) {
  test(`${call.label}: a window resized while the question was open is stale_screenshot, and nothing is sent`, async () => {
    await withPreparedWindow({ get_window: [...windows(1), { result: { ...demo, width: 300 } }], ask_user: [allow] }, async (harness, since) => {
      assert.equal(code(await hold(harness, call, ["win"])), "stale_screenshot");
      assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
    });
  });
}

for (const call of POINTER_CALLS) {
  test(`${call.label}: a window that became another app's while the question was open is app_changed, and nothing is sent`, async () => {
    await withPreparedWindow({ get_window: [...windows(1), { result: { ...notes, window_id: "5" } }], ask_user: [allow] }, async (harness, since) => {
      const changed = await hold(harness, call, ["win"]);
      assert.equal(code(changed), "app_changed");
      assert.match(textContent(changed), /Notes/);
      assert.match(textContent(changed), /nothing was sent/);
      assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
    });
  });
}

test("a stop while the question of a click is open is interrupted, sends nothing and is not remembered", async () => {
  await withPreparedWindow({ ask_user: [{ error: { code: "interrupted", message: "The user stopped it. End this turn." } }] }, async (harness, since) => {
    assert.equal(code(await hold(harness, CLICK_AT_POINT, ["win"])), "interrupted");
    assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
    assert.deepEqual(await systemKeyEvents(harness), []);
  });
});

for (const [label, step] of [
  ["a click at a point", { action: "click", window_id: "5", x: 20, y: 30, delivery_mode: "foreground", modifiers: ["win"] }],
  ["a click on an element", { action: "click", window_id: "5", element_index: 1, delivery_mode: "foreground", modifiers: ["win"] }],
  ["a drag", { action: "drag", window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50, modifiers: ["win"] }],
] as const) {
  test(`a batch step of ${label} holding win goes through the same check: refused, it stops the batch and the later steps do not run`, async () => {
    await withPreparedWindow({ ask_user: [deny] }, async (harness, since) => {
      const result = await harness.client.callTool({ name: "computer_batch", arguments: { steps: [step, { action: "press_key", window_id: "5", key: "ctrl+s" }] } });
      assert.equal(result.isError, true);
      const text = textContent(result);
      assert.match(text, new RegExp(`\\[1\\] ${step.action}: approval_declined \\(sent: false\\) `));
      assert.match(text, /\[2\] press_key: not run/);
      assert.deepEqual((await since()).filter((method) => ACTING.includes(method)), []);
      assert.deepEqual((await systemKeyAsks(harness)).map(({ combination }) => combination), ["win"]);
    });
  });
}

test("a batch step holding win with the user's consent runs, and the consent holds for the steps after it, computer_press_key's too", async () => {
  await withPreparedWindow({ ask_user: [allow] }, async (harness) => {
    const result = await harness.client.callTool({
      name: "computer_batch",
      arguments: {
        steps: [
          { action: "click", window_id: "5", x: 20, y: 30, delivery_mode: "foreground", modifiers: ["win"] },
          { action: "drag", window_id: "5", from_x: 10, from_y: 10, to_x: 40, to_y: 50, modifiers: ["super"] },
          { action: "press_key", window_id: "5", key: "win" },
        ],
      },
    });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal((await systemKeyAsks(harness)).length, 1);
    assert.deepEqual(await pressedKeys(harness), [["win"]]);
  });
});
