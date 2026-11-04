import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "../src/server.ts";
import { decideUiLanguage, parseRegMultiSz } from "../src/ui-language.ts";

// Open-source ticket 01: the one language decision of the server and of the settings server. The
// Windows UI language comes from an injected lookup, so the rule is tested without the machine's.

const windows = (...languages: string[]) => async () => languages;

test("a Traditional Chinese Windows UI language gives zh-TW", async () => {
  for (const tag of ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh-Hant-TW", "ZH-tw"]) {
    assert.equal(await decideUiLanguage(undefined, windows(tag)), "zh-TW", tag);
  }
});

test("any other Windows UI language, or none, gives en", async () => {
  for (const tag of ["zh-CN", "zh-SG", "zh-Hans", "zh-Hans-HK", "en-US", "ja-JP", ""]) {
    assert.equal(await decideUiLanguage(undefined, windows(tag)), "en", tag);
  }
  assert.equal(await decideUiLanguage(undefined, windows()), "en", "no language");
});

test("the first (preferred) UI language decides", async () => {
  assert.equal(await decideUiLanguage(undefined, windows("en-US", "zh-TW")), "en");
  assert.equal(await decideUiLanguage(undefined, windows("zh-TW", "en-US")), "zh-TW");
});

test("DESKPASS_LANG en or zh-TW overrides the Windows UI language without looking it up", async () => {
  const never = async (): Promise<string[]> => {
    throw new Error("the Windows UI language was looked up");
  };
  assert.equal(await decideUiLanguage("en", never), "en");
  assert.equal(await decideUiLanguage("zh-TW", never), "zh-TW");
  assert.equal(await decideUiLanguage("en", windows("zh-TW")), "en");
  assert.equal(await decideUiLanguage("zh-TW", windows("en-US")), "zh-TW");
});

test("any other DESKPASS_LANG is ignored", async () => {
  for (const value of ["", "fr", "EN", "zh-tw", "zh-CN", "zh-Hant", " en"]) {
    assert.equal(await decideUiLanguage(value, windows("zh-TW")), "zh-TW", JSON.stringify(value));
    assert.equal(await decideUiLanguage(value, windows("en-US")), "en", JSON.stringify(value));
  }
});

test("a lookup that fails gives en", async () => {
  assert.equal(await decideUiLanguage(undefined, async () => Promise.reject(new Error("reg.exe failed"))), "en");
});

test("the MCP server decides its language once, when it is created", async () => {
  const saved = process.env.DESKPASS_LANG;
  process.env.DESKPASS_LANG = "en";
  const app = createServer();
  process.env.DESKPASS_LANG = "zh-TW";
  const pinned = createServer({ lang: "en" });
  try {
    assert.equal(await app.language, "en");
    assert.equal(await pinned.language, "en");
  } finally {
    if (saved === undefined) delete process.env.DESKPASS_LANG;
    else process.env.DESKPASS_LANG = saved;
    await app.dispose();
    await pinned.dispose();
  }
});

test("reg.exe's REG_MULTI_SZ answer is read as its list of languages", () => {
  const answer = "\r\nHKEY_CURRENT_USER\\Control Panel\\Desktop\\MuiCached\r\n    MachinePreferredUILanguages    REG_MULTI_SZ    zh-TW\\0en-US\r\n\r\n";
  assert.deepEqual(parseRegMultiSz(answer, "MachinePreferredUILanguages"), ["zh-TW", "en-US"]);
  assert.deepEqual(parseRegMultiSz("\r\nHKEY_CURRENT_USER\\Control Panel\\Desktop\r\n    PreferredUILanguages    REG_MULTI_SZ    en-US\r\n", "PreferredUILanguages"), ["en-US"]);
  assert.deepEqual(parseRegMultiSz("ERROR: The system was unable to find the specified registry key or value.", "PreferredUILanguages"), []);
});
