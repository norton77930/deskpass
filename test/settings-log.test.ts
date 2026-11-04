import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { LOG_MAX_BYTES, LOG_RETENTION_DAYS } from "../src/config.ts";
import { latestLogDay, logDay, logDays, retentionSentence } from "../src/settings-log.ts";

// Ticket 117 (first half): the Node side of the settings page's Log tab. Seam: fixture
// `logs/YYYY-MM-DD.jsonl` files in a temp data folder in, the grouped rows of a day out. Sentences
// follow the native panel's log tab (ticket 105); a result is a `tag`, not words of the sentence.

const folders: string[] = [];

async function dataDir(days: Record<string, (object | string)[]>): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "deskpass-settings-log-"));
  folders.push(dir);
  await mkdir(join(dir, "logs"));
  for (const [day, lines] of Object.entries(days)) {
    const text = lines.map((line) => (typeof line === "string" ? line : JSON.stringify(line))).join("\n");
    await writeFile(join(dir, "logs", `${day}.jsonl`), `${text}\n`, "utf8");
  }
  return dir;
}

after(async () => {
  await Promise.all(folders.map((dir) => rm(dir, { recursive: true, force: true })));
});

const DEMO = { path: "C:\\Apps\\Demo.exe", key: "signer:CN=Demo|demo.exe", tier: "normal" };
const NOTES = { path: "C:\\Apps\\Notes.exe", key: "signer:CN=Notes|notes.exe", tier: "normal" };

const NAMES: Record<string, string> = { [DEMO.key]: "Demo", [NOTES.key]: "記事本" };
/** The test's clock is UTC, so a case does not depend on the machine's time zone. */
const read = {
  localTime: (iso: string) => iso.slice(11, 16),
  appName: (key: string, path: string | null) => NAMES[key] ?? path ?? key,
};

const at = (time: string) => `2026-10-20T${time}:00.000Z`;

function call(time: string, app: object | null, result = "ok", tool = "computer_click", summary: string | null = null) {
  return { time: at(time), session: "s1", event: "tool_call", tool, step: null, app, action_summary: summary, result, duration_ms: 5 };
}

function approval(time: string, app: object | null, channel: string | null, answer: string, extra: object = {}) {
  return { time: at(time), session: "s1", event: "approval", app, channel, answer, ...extra };
}

test("the days offered are today, always first, then the other day files of the last 7 days, newest first", async () => {
  const dir = await dataDir({
    "2026-10-21": [call("08:00", DEMO)],
    "2026-10-20": [call("08:00", DEMO)],
    "2026-10-17": [call("08:00", DEMO)],
    "2026-10-14": [call("08:00", DEMO)],
    "2026-10-13": [call("08:00", DEMO)],
    "2026-10-01": [call("08:00", DEMO)],
  });
  assert.deepEqual(await logDays(dir, "2026-10-21"), ["2026-10-21", "2026-10-20", "2026-10-17", "2026-10-14"]);
  // Today has no file yet: still offered, and first.
  // Seven days before today are looked at: a day file one day too old drops out as the days pass.
  assert.deepEqual(await logDays(dir, "2026-10-22"), ["2026-10-22", "2026-10-21", "2026-10-20", "2026-10-17"]);
});

test("a missing log folder and files that are not day files give only today", async () => {
  const none = await mkdtemp(join(tmpdir(), "deskpass-settings-log-"));
  folders.push(none);
  assert.deepEqual(await logDays(none, "2026-10-21"), ["2026-10-21"]);
  const dir = await dataDir({ "2026-10-20": [call("08:00", DEMO)] });
  await writeFile(join(dir, "logs", "notes.jsonl"), "{}\n");
  await writeFile(join(dir, "logs", "2026-10-19.txt"), "{}\n");
  assert.deepEqual(await logDays(dir, "2026-10-21"), ["2026-10-21", "2026-10-20"]);
});

test("consecutive events of one app are one group; another app between them breaks it", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      call("09:00", DEMO, "ok", "computer_screenshot"),
      call("09:01", DEMO),
      call("09:05", NOTES),
      call("09:07", DEMO),
      call("09:09", DEMO),
      call("09:10", DEMO),
    ],
  });
  const { groups } = await logDay(dir, "2026-10-20", read);
  assert.deepEqual(
    groups.map(({ key, app, count, first, last }) => ({ key, app, count, first, last })),
    [
      { key: DEMO.key, app: "Demo", count: 2, first: "09:00", last: "09:01" },
      { key: NOTES.key, app: "記事本", count: 1, first: "09:05", last: "09:05" },
      { key: DEMO.key, app: "Demo", count: 3, first: "09:07", last: "09:10" },
    ],
  );
  assert.deepEqual(groups[0]!.rows.map((row) => row.time), ["09:00", "09:01"]);
  assert.equal(groups[0]!.rows.length, groups[0]!.count);
});

test("a tool call is a sentence by the tool's name with its result as the tag", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      call("09:00", DEMO, "ok", "computer_screenshot", "看一下"),
      call("09:01", DEMO, "approval_declined", "computer_paste_text", "  "),
      call("09:02", DEMO, "helper_timeout", "computer_press_key"),
      call("09:03", DEMO, "blocked_app", "computer_get_ui_tree"),
      call("09:04", DEMO, "new_code", "computer_new_tool"),
      call("09:05", null, "ok", "computer_list_apps"),
    ],
  });
  const { groups } = await logDay(dir, "2026-10-20", read);
  assert.deepEqual(groups[0]!.rows, [
    { time: "09:00", sentence: "截圖 · 說明:看一下", tag: "成功" },
    { time: "09:01", sentence: "貼上文字", tag: "你拒絕" },
    { time: "09:02", sentence: "按鍵", tag: "逾時" },
    { time: "09:03", sentence: "讀取元素樹", tag: "被封鎖" },
    { time: "09:04", sentence: "computer_new_tool", tag: "new_code" },
  ]);
  // A call about no app is its own group, named with a dash.
  assert.deepEqual(groups[1]!.app, "—");
  assert.equal(groups[1]!.key, null);
  assert.deepEqual(groups[1]!.rows, [{ time: "09:05", sentence: "列出 App", tag: "成功" }]);
});

test("ticket 146: a tool call that has a delivery is marked 背景 or 前景; a refused background action is listed with its tag, still marked 背景", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      { ...call("09:00", DEMO, "ok", "computer_element_action"), delivery: "background" },
      { ...call("09:01", DEMO, "ok", "computer_set_value"), delivery: "foreground" },
      { ...call("09:02", DEMO, "background_unavailable", "computer_click"), delivery: "background", reason: "needs_pointer" },
      { ...call("09:03", DEMO, "ok", "computer_click"), delivery: "foreground" },
      // A call that decided no delivery (a read, a batch, an older log) has no mark.
      call("09:04", DEMO, "ok", "computer_screenshot"),
      { ...call("09:05", DEMO, "ok", "computer_click"), delivery: "sideways" },
    ],
  });
  const { groups } = await logDay(dir, "2026-10-20", read);
  assert.deepEqual(groups[0]!.rows, [
    { time: "09:00", sentence: "元素操作", tag: "成功", delivery: "background", mark: "背景" },
    { time: "09:01", sentence: "設定欄位值", tag: "成功", delivery: "foreground", mark: "前景" },
    { time: "09:02", sentence: "點擊", tag: "背景操作不可用", delivery: "background", mark: "背景" },
    { time: "09:03", sentence: "點擊", tag: "成功", delivery: "foreground", mark: "前景" },
    { time: "09:04", sentence: "截圖", tag: "成功" },
    { time: "09:05", sentence: "點擊", tag: "成功" },
  ]);
});

test("an approval is a sentence by its channel with the answer as the tag", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      approval("09:00", DEMO, "dialog", "allow_once"),
      approval("09:01", DEMO, "dialog", "allow_always"),
      approval("09:02", DEMO, "dialog", "refuse"),
      approval("09:03", DEMO, "dialog", "timeout"),
      approval("09:04", DEMO, "elicitation", "allow_once"),
      approval("09:05", DEMO, "grant", "allow_always"),
      approval("09:06", DEMO, "session", "allow_once"),
      approval("09:07", DEMO, "session", "refused_earlier"),
      approval("09:08", DEMO, null, "blocked"),
    ],
  });
  const [group] = (await logDay(dir, "2026-10-20", read)).groups;
  assert.deepEqual(
    group!.rows.map(({ sentence, tag }) => [sentence, tag]),
    [
      ["詢問核准", "你允許一次"],
      ["詢問核准", "你永遠允許"],
      ["詢問核准", "你拒絕"],
      ["詢問核准", "逾時未回答"],
      ["在 MCP 用戶端詢問核准", "你允許一次"],
      ["核准", "依你的永遠允許"],
      ["核准", "沿用你這次的允許"],
      ["核准", "先前已拒絕"],
      ["核准", "封鎖"],
    ],
  );
});

test("the clipboard's approval and read are about the app 剪貼簿 and a read says how many characters", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      approval("09:00", null, "dialog", "allow_once", { subject: "clipboard" }),
      { time: at("09:01"), session: "s1", event: "clipboard_read", chars: 1234, truncated: false },
      { time: at("09:02"), session: "s1", event: "clipboard_read", chars: 20000, truncated: true },
    ],
  });
  const day = await logDay(dir, "2026-10-20", read);
  assert.equal(day.groups.length, 2, "the read is about no app, the approval about the clipboard");
  assert.equal(day.groups[0]!.app, "剪貼簿");
  assert.deepEqual(day.groups[0]!.rows, [{ time: "09:00", sentence: "詢問核准", tag: "你允許一次" }]);
  assert.deepEqual(day.groups[1]!.rows, [
    { time: "09:01", sentence: "讀剪貼簿", tag: "1234 字" },
    { time: "09:02", sentence: "讀剪貼簿", tag: "20000 字(已截斷)" },
  ]);
  assert.deepEqual(day.apps.map((app) => app.name), ["剪貼簿"]);
});

test("grant events of the server are sentences with their reason or count as the tag", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      { time: at("09:00"), session: "s1", event: "grant_invalid", entry: "raised", key: DEMO.key, reason: "not_higher" },
      { time: at("09:01"), session: "s1", event: "grant_invalid", entry: "grant", key: DEMO.key, reason: "bad_mac" },
      { time: at("09:02"), session: "s1", event: "grant_invalid", entry: "file", key: null, reason: "rollback" },
      { time: at("09:03"), session: "s1", event: "grant_key_lost", entries: 2 },
      { time: at("09:04"), session: "s1", event: "grant_mismatch", key: DEMO.key, current_key: "signer:CN=Other|demo.exe", path: DEMO.path },
      { time: at("09:05"), session: "s1", event: "tier_raised", key: DEMO.key, tier: "high", grant_removed: true },
      { time: at("09:06"), session: "s1", event: "grant_revoked", key: DEMO.key },
      { time: at("09:07"), session: "s1", event: "tier_raise_removed", key: DEMO.key },
    ],
  });
  const { groups } = await logDay(dir, "2026-10-20", read);
  const rows = groups.map((group) => [group.app, group.rows.map(({ sentence, tag }) => [sentence, tag])]);
  assert.deepEqual(rows, [
    ["Demo", [["授權檔裡的調高無效", "沒有高於內建等級"], ["授權檔裡的永遠允許無效", "簽章不符,可能被修改過"]]],
    ["—", [["授權檔無效", "比先前讀過的版本舊"], ["授權金鑰遺失", "2 筆項目失效"]]],
    [
      "Demo",
      [
        ["永遠允許不符", "App 的身分已改變,改為詢問你"],
        ["調高等級", "設為高風險,並取消它的永遠允許"],
        ["取消永遠允許", undefined],
        ["移除調高", "恢復預設"],
      ],
    ],
  ]);
});

test("changes made on the settings page read 「你在設定網頁…」, and old panel lines 「你在控制台…」", async () => {
  const settings = (time: string, extra: object) => ({ time: at(time), session: "settings", key: DEMO.key, ...extra });
  const panel = (time: string, extra: object) => ({ time: at(time), session: "panel", key: DEMO.key, ...extra });
  const dir = await dataDir({
    "2026-10-20": [
      settings("09:00", { event: "tier_raised", tier: "blocked", grant_removed: false }),
      settings("09:01", { event: "tier_raised", tier: "blocked", grant_removed: true, path: DEMO.path }),
      settings("09:02", { event: "tier_raised", tier: "high", grant_removed: false }),
      settings("09:03", { event: "grant_revoked" }),
      settings("09:04", { event: "tier_raise_removed" }),
      panel("09:05", { event: "tier_raised", tier: "blocked", grant_removed: true }),
      panel("09:06", { event: "grant_revoked" }),
      panel("09:07", { event: "tier_raise_removed" }),
      panel("09:08", { event: "panel_new" }),
    ],
  });
  const { groups } = await logDay(dir, "2026-10-20", read);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0]!.rows, [
    { time: "09:00", sentence: "你在設定網頁把Demo設為封鎖" },
    { time: "09:01", sentence: "你在設定網頁把Demo設為封鎖,並取消它的永遠允許" },
    { time: "09:02", sentence: "你在設定網頁把Demo設為高風險" },
    { time: "09:03", sentence: "你在設定網頁取消Demo的永遠允許" },
    { time: "09:04", sentence: "你在設定網頁把Demo恢復預設" },
    { time: "09:05", sentence: "你在控制台把Demo設為封鎖,並取消它的永遠允許" },
    { time: "09:06", sentence: "你在控制台取消Demo的永遠允許" },
    { time: "09:07", sentence: "你在控制台把Demo恢復預設" },
  ]);
});

test("unknown events and malformed lines are skipped without breaking the day", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      call("09:00", DEMO),
      "{not json",
      "",
      "[1,2]",
      "42",
      JSON.stringify({ time: at("09:01"), event: "future_event", app: NOTES, secret: "s" }),
      JSON.stringify({ time: at("09:02") }),
      call("09:03", DEMO),
    ],
  });
  const day = await logDay(dir, "2026-10-20", read);
  assert.equal(day.groups.length, 1, "a skipped line between two events of one app does not split them");
  assert.deepEqual(day.groups[0]!.rows.map((row) => row.time), ["09:00", "09:03"]);
  assert.deepEqual(day.apps, [{ key: DEMO.key, name: "Demo" }]);
});

test("a day with no file, or an unreadable one, is an empty day", async () => {
  const dir = await dataDir({});
  assert.deepEqual(await logDay(dir, "2026-10-21", read), { day: "2026-10-21", groups: [], apps: [], truncated: false });
  await mkdir(join(dir, "logs", "2026-10-20.jsonl"));
  assert.deepEqual((await logDay(dir, "2026-10-20", read)).groups, []);
});

test("the app list holds only the apps of that day, and the filter keeps only that app's groups", async () => {
  const dir = await dataDir({
    "2026-10-20": [call("09:00", DEMO), call("09:01", NOTES), call("09:02", DEMO), call("09:03", null)],
    "2026-10-19": [call("09:00", { path: "C:\\Apps\\Old.exe", key: "signer:CN=Old|old.exe", tier: "normal" })],
  });
  const all = await logDay(dir, "2026-10-20", read);
  assert.deepEqual(all.apps, [
    { key: DEMO.key, name: "Demo" },
    { key: NOTES.key, name: "記事本" },
  ]);
  const demo = await logDay(dir, "2026-10-20", { ...read, app: DEMO.key });
  assert.deepEqual(demo.groups.map((group) => [group.key, group.count, group.first]), [[DEMO.key, 1, "09:00"], [DEMO.key, 1, "09:02"]], "groups keep the breaks the other app made");
  assert.deepEqual(demo.apps, all.apps, "the filter list does not shrink to the chosen app");
  assert.deepEqual((await logDay(dir, "2026-10-20", { ...read, app: "signer:CN=Old|old.exe" })).groups, []);
  assert.deepEqual((await logDay(dir, "2026-10-19", read)).apps, [{ key: "signer:CN=Old|old.exe", name: "C:\\Apps\\Old.exe" }]);
});

test("the app list is ordered by name without regard to case", async () => {
  const dir = await dataDir({ "2026-10-20": [call("09:00", NOTES), call("09:01", DEMO)] });
  assert.deepEqual((await logDay(dir, "2026-10-20", read)).apps.map((app) => app.name), ["Demo", "記事本"]);
});

test("without a Start Menu name an app is named by its file", async () => {
  const dir = await dataDir({
    "2026-10-20": [
      call("09:00", DEMO),
      { time: at("09:01"), session: "settings", event: "grant_revoked", key: "package:fabrikam.paint_xyz" },
      { time: at("09:02"), session: "settings", event: "grant_revoked", key: "signer:CN=Contoso|tool.exe" },
    ],
  });
  const { groups } = await logDay(dir, "2026-10-20", { localTime: read.localTime });
  assert.deepEqual(groups.map((group) => group.app), ["Demo.exe", "fabrikam.paint", "tool.exe"]);
});

test("the newest 2000 rows of a day are shown and the day says it was cut", async () => {
  const lines = Array.from({ length: 2003 }, (_, index) => call("09:00", DEMO, "ok", "computer_click", `n${index}`));
  const dir = await dataDir({ "2026-10-20": lines });
  const day = await logDay(dir, "2026-10-20", read);
  assert.equal(day.truncated, true);
  assert.equal(day.groups[0]!.count, 2000);
  assert.equal(day.groups[0]!.rows[0]!.sentence, "點擊 · 說明:n3");
});

test("a row never holds anything but the event's fixed fields", async () => {
  const dir = await dataDir({
    "2026-10-20": [{ ...call("09:00", DEMO), text: "hunter2", keys: "ctrl+v", clipboard: "secret", title: "Bank" }],
  });
  const text = JSON.stringify(await logDay(dir, "2026-10-20", read));
  for (const secret of ["hunter2", "ctrl+v", "secret", "Bank"]) assert.ok(!text.includes(secret), secret);
});

test("times are the local hour and minute; the default clock follows the machine's zone", async () => {
  const when = new Date(2026, 9, 20, 9, 5, 30);
  const dir = await dataDir({
    "2026-10-20": [{ ...call("09:00", DEMO), time: when.toISOString() }, { ...call("09:00", DEMO), time: "yesterday-ish" }],
  });
  const { groups } = await logDay(dir, "2026-10-20", { appName: read.appName });
  assert.deepEqual(groups[0]!.rows.map((row) => row.time), ["09:05", "--:--"]);
});

test("the latest day with records within 7 days is offered when today is empty", async () => {
  const dir = await dataDir({
    "2026-10-21": [],
    "2026-10-19": [call("09:00", DEMO)],
    "2026-10-17": [call("09:00", DEMO)],
  });
  assert.equal(await latestLogDay(dir, "2026-10-21", read), "2026-10-19");
  const today = await dataDir({ "2026-10-21": [call("09:00", DEMO)], "2026-10-19": [call("09:00", DEMO)] });
  assert.equal(await latestLogDay(today, "2026-10-21", read), "2026-10-21");
});

test("a day file with nothing readable in it is not a day with records", async () => {
  const dir = await dataDir({ "2026-10-20": ["{broken", JSON.stringify({ time: at("09:00"), event: "future_event" })], "2026-10-19": [call("09:00", DEMO)] });
  assert.equal(await latestLogDay(dir, "2026-10-21", read), "2026-10-19");
});

test("with no records in the last 7 days there is no day to offer", async () => {
  const dir = await dataDir({ "2026-10-10": [call("09:00", DEMO)] });
  assert.equal(await latestLogDay(dir, "2026-10-21", read), null);
  const empty = await mkdtemp(join(tmpdir(), "deskpass-settings-log-"));
  folders.push(empty);
  assert.equal(await latestLogDay(empty, "2026-10-21", read), null);
});

test("the retention sentence follows the real log settings", () => {
  const sentence = retentionSentence();
  assert.ok(sentence.includes(`${LOG_RETENTION_DAYS} 天`), sentence);
  assert.ok(sentence.includes(`${Math.round(LOG_MAX_BYTES / 1024 / 1024)} MB`), sentence);
  assert.ok(sentence.includes("只存在你的電腦上"), sentence);
});
