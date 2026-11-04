import assert from "node:assert/strict";
import { test } from "node:test";
import type { GrantVerdict } from "../src/grants.ts";
import { BUILT_IN_TIERS } from "../src/policy.ts";
import { grantsState } from "../src/settings-apps.ts";
import { fill, SETTINGS_TEXT, SETTINGS_TEXT_EN, settingsText } from "../src/settings-strings.ts";
import { CJK } from "./harness.ts";

// Open-source ticket 01: the settings page's text is one table per language, of one shape. A text
// added to one table and not the other, or Chinese left in the English one, fails here.

/** Every leaf of `value` as `path = text`. */
function leaves(value: unknown, path = ""): [string, string][] {
  if (typeof value === "string") return [[path, value]];
  if (value === null || typeof value !== "object") return [[path, String(value)]];
  return Object.entries(value).flatMap(([key, child]) => leaves(child, path === "" ? key : `${path}.${key}`));
}

test("the English and zh-TW settings tables have the same keys", () => {
  const zh = leaves(SETTINGS_TEXT).map(([path]) => path);
  const en = leaves(SETTINGS_TEXT_EN).map(([path]) => path);
  assert.deepEqual(
    en.filter((path) => !zh.includes(path)),
    [],
    "keys only in the English table",
  );
  assert.deepEqual(
    zh.filter((path) => !en.includes(path)),
    [],
    "keys only in the zh-TW table",
  );
});

test("the English settings table has no CJK character and no empty text", () => {
  for (const [path, text] of leaves(SETTINGS_TEXT_EN)) {
    assert.doesNotMatch(text, CJK, path);
    assert.ok(text.length > 0, path);
  }
});

test("both tables keep the same {placeholders} in each text", () => {
  const zh = new Map(leaves(SETTINGS_TEXT));
  const names = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
  for (const [path, text] of leaves(SETTINGS_TEXT_EN)) assert.deepEqual(names(text), names(zh.get(path) ?? ""), path);
});

test("every built-in tier group has its name and reason in both tables", () => {
  for (const group of [...BUILT_IN_TIERS.blocked, ...BUILT_IN_TIERS.high]) {
    for (const table of [SETTINGS_TEXT, SETTINGS_TEXT_EN]) {
      const text = (table.builtInTiers as Record<string, { label: string; reason: string } | undefined>)[group.id];
      assert.ok(text && text.label.length > 0 && text.reason.length > 0, group.id);
    }
  }
  assert.deepEqual(Object.keys(SETTINGS_TEXT.builtInTiers).sort(), [...BUILT_IN_TIERS.blocked, ...BUILT_IN_TIERS.high].map((group) => group.id).sort());
});

// Open-source ticket 02: the log tab's row sentences are in the same tables (`logRows`), so the
// tests above cover them; these pin what they must hold.

test("the log row texts are in both tables, with the same tools, results, answers and reasons", () => {
  for (const table of [SETTINGS_TEXT, SETTINGS_TEXT_EN]) {
    const rows = (table as { logRows?: typeof SETTINGS_TEXT.logRows }).logRows;
    assert.ok(rows, "the table has logRows");
    for (const group of [rows.tools, rows.results, rows.delivery, rows.approval.answers, rows.grantInvalid.what, rows.grantInvalid.reasons, rows.tiers]) {
      assert.ok(Object.keys(group).length > 0);
    }
  }
  assert.deepEqual(Object.keys(SETTINGS_TEXT_EN.logRows.tools).sort(), Object.keys(SETTINGS_TEXT.logRows.tools).sort());
  assert.deepEqual(Object.keys(SETTINGS_TEXT_EN.logRows.results).sort(), Object.keys(SETTINGS_TEXT.logRows.results).sort());
  assert.ok(Object.keys(SETTINGS_TEXT.logRows.tools).includes("computer_read_clipboard"));
  assert.ok(Object.keys(SETTINGS_TEXT.logRows.results).includes("approval_declined"));
});

test("in each language the tags of the log tab's three tones do not overlap", () => {
  for (const table of [SETTINGS_TEXT, SETTINGS_TEXT_EN]) {
    const { results, approval, grantInvalid, mismatch } = table.logRows;
    const ok = [results.ok!];
    const ask = [approval.answers.allow_once!, approval.answers.allow_always!, approval.grantAllowAlways, approval.sessionAllowOnce];
    const bad = [...Object.entries(results).filter(([name]) => name !== "ok").map(([, tag]) => tag), ...Object.values(grantInvalid.reasons), mismatch.result];
    for (const tag of ok) assert.ok(!ask.includes(tag) && !bad.includes(tag), tag);
    for (const tag of ask) assert.ok(!bad.includes(tag), tag);
  }
});

// A text that shows a number has a `...One` twin for exactly one, in both tables (zh-TW's is the
// same text: it has one form); English reads "1 entry" and "2 entries".

test("every text that shows a count has a singular twin in both tables, with the same placeholders", () => {
  for (const table of [SETTINGS_TEXT, SETTINGS_TEXT_EN]) {
    const all = new Map(leaves(table));
    const twins = [...all.keys()].filter((path) => /One$/.test(path));
    assert.deepEqual(twins.map((path) => path.replace(/One$/, "")).sort(), ["grants.ignored", "log.summary", "logRows.clipboardRead.result", "logRows.keyLost.result", "page.count"]);
    for (const path of twins) assert.deepEqual([...(all.get(path) ?? "").matchAll(/\{(\w+)\}/g)].map((m) => m[1]), [...(all.get(path.replace(/One$/, "")) ?? "").matchAll(/\{(\w+)\}/g)].map((m) => m[1]), path);
  }
});

test("English counts read 1 entry and 2 entries, and zh-TW keeps its one form", () => {
  const E = SETTINGS_TEXT_EN;
  const pick = (one: string, many: string, n: number) => fill(n === 1 ? one : many, { n, first: "14:35" });
  assert.equal(pick(E.log.summaryOne, E.log.summary, 1), "1 entry · 14:35");
  assert.equal(pick(E.log.summaryOne, E.log.summary, 2), "2 entries · 14:35");
  assert.equal(pick(E.page.countOne, E.page.count, 1), "Showing 1 app");
  assert.equal(pick(E.page.countOne, E.page.count, 0), "Showing 0 apps");
  assert.equal(pick(E.logRows.keyLost.resultOne, E.logRows.keyLost.result, 1), "1 entry no longer valid");
  assert.equal(pick(E.logRows.keyLost.resultOne, E.logRows.keyLost.result, 3), "3 entries no longer valid");
  assert.equal(pick(E.logRows.clipboardRead.resultOne, E.logRows.clipboardRead.result, 1), "1 char");
  assert.equal(pick(E.logRows.clipboardRead.resultOne, E.logRows.clipboardRead.result, 42), "42 chars");
  assert.equal(pick(E.grants.ignoredOne, E.grants.ignored, 1), "1 entry of the grants file did not pass verification and was ignored.");
  assert.equal(pick(E.grants.ignoredOne, E.grants.ignored, 2), "2 entries of the grants file did not pass verification and were ignored.");
  for (const [one, many] of [[SETTINGS_TEXT.log.summaryOne, SETTINGS_TEXT.log.summary], [SETTINGS_TEXT.page.countOne, SETTINGS_TEXT.page.count], [SETTINGS_TEXT.logRows.keyLost.resultOne, SETTINGS_TEXT.logRows.keyLost.result], [SETTINGS_TEXT.logRows.clipboardRead.resultOne, SETTINGS_TEXT.logRows.clipboardRead.result], [SETTINGS_TEXT.grants.ignoredOne, SETTINGS_TEXT.grants.ignored]] as const) {
    assert.equal(one, many);
  }
});

test("the grants file's ignored-entries note uses the singular for one entry", () => {
  const note = (ignored: number) => grantsState({ state: "valid", ignored } as unknown as GrantVerdict, SETTINGS_TEXT_EN).message;
  assert.equal(note(1), "1 entry of the grants file did not pass verification and was ignored.");
  assert.equal(note(2), "2 entries of the grants file did not pass verification and were ignored.");
  assert.equal(note(0), null);
});

test("the table of a language", () => {
  assert.equal(settingsText("zh-TW"), SETTINGS_TEXT);
  assert.equal(settingsText("en"), SETTINGS_TEXT_EN);
});
