import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { test } from "node:test";
import type { UiLanguage } from "../src/ui-language.ts";
import { CJK, connectServer, launchScriptedHelper, retryingSpawn, textContent, type FakeScript } from "./harness.ts";

// Open-source ticket 03: the helper shows its dialogs and notices in the language the server
// decided. The server passes it as `--lang <en|zh-TW>` when it starts the helper (the helper never
// detects it, and shows English without one), and the question after Esc that the client
// elicitation asks is in the same language. The fake helper records the arguments it was started
// with; the real helper's own texts are tested in helper/src/strings.rs and, on a desktop, in
// helper/tests/language_en.rs.

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

const CONTINUE_EN = "You pressed Esc to stop Deskpass earlier. Let the AI continue?";
const CONTINUE_ZH = "你先前按了 Esc 停止操作,要讓 AI 繼續嗎?";

/** The script of a session stopped by Esc whose next call asks to continue (and then, for an allow, to approve Demo). */
const stoppedScript = (extra: FakeScript["handlers"] = {}): FakeScript => ({
  handlers: {
    list_windows: [{ events: [{ event: "stopped" }], result: { windows: [] } }],
    get_window: [{ result: openWindow }],
    capture_window: [{ result: captured }],
    ...extra,
  },
});

function langOf(args: string[]): string | undefined {
  const at = args.indexOf("--lang");
  return at === -1 ? undefined : args[at + 1];
}

for (const [lang, question] of [["en", CONTINUE_EN], ["zh-TW", CONTINUE_ZH]] as [UiLanguage, string][]) {
  test(`a ${lang} server starts the helper with --lang ${lang} and asks the continue question in ${lang} through the client`, async () => {
    const harness = await connectServer({ lang, elicitation: "form", script: stoppedScript() });
    try {
      const interrupted = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
      assert.equal(textContent(interrupted).split(/\r?\n/)[0], "interrupted");
      const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
      assert.notEqual(shot.isError, true, textContent(shot));
      const starts = await harness.spawnArgs();
      assert.equal(starts.length, 1, JSON.stringify(starts));
      assert.equal(langOf(starts[0] ?? []), lang);
      assert.equal(harness.elicitations[0], question);
      // The approval question that follows is the server's own English one in both languages.
      assert.match(harness.elicitations[1] ?? "", /demo\.exe/i);
      if (lang === "en") assert.doesNotMatch(harness.elicitations.join("\n"), CJK, "an English server asks nothing in Chinese");
    } finally {
      await harness.close();
    }
  });
}

test("a client that cannot ask gets the continue question from the helper dialog, which was started in the server's language", async () => {
  for (const lang of ["en", "zh-TW"] as UiLanguage[]) {
    const harness = await connectServer({
      lang,
      script: stoppedScript({ ask_user: [{ result: { answer: "allow" } }, { result: { answer: "allow" } }], resume: [{ result: {} }], session_start: [{ result: {} }] }),
    });
    try {
      await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
      const shot = await harness.client.callTool({ name: "computer_screenshot", arguments: { window_id: "5" } });
      assert.notEqual(shot.isError, true, textContent(shot));
      const asked = (await harness.requests()).filter((request) => (request as { method?: string }).method === "ask_user") as { params: { kind: string } }[];
      assert.equal(asked[0]?.params.kind, "continue", JSON.stringify(asked));
      assert.deepEqual((await harness.spawnArgs()).map(langOf), [lang]);
      assert.equal(harness.elicitations.length, 0, "nothing went through the client");
    } finally {
      await harness.close();
    }
  }
});

test("a helper started with no language is given none, so it shows English", async () => {
  const started: string[][] = [];
  const inner = retryingSpawn();
  const { helper, close } = await launchScriptedHelper({}, {
    spawn: ((...args: Parameters<typeof spawn>) => {
      started.push([...((args[1] as string[] | undefined) ?? [])]);
      return inner(...args);
    }) as typeof spawn,
  });
  try {
    await helper.request("ping", {});
    assert.equal(started.length, 1);
    assert.equal(started[0]?.includes("--lang"), false, JSON.stringify(started));
  } finally {
    await close();
  }
});

test("a helper client given a language (or a promise of one) starts the helper with it", async () => {
  for (const lang of ["en", Promise.resolve("zh-TW" as UiLanguage)]) {
    const started: string[][] = [];
    const inner = retryingSpawn();
    const { helper, close } = await launchScriptedHelper({}, {
      lang,
      spawn: ((...args: Parameters<typeof spawn>) => {
        started.push([...((args[1] as string[] | undefined) ?? [])]);
        return inner(...args);
      }) as typeof spawn,
    });
    try {
      await helper.request("ping", {});
      assert.deepEqual(started.map(langOf), [await lang]);
      assert.ok(started[0]?.includes("--server-pid"), "the server's own arguments are still there");
    } finally {
      await close();
    }
  }
});
