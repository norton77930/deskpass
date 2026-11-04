import assert from "node:assert/strict";
import type { ChildProcess } from "node:child_process";
import { createHmac } from "node:crypto";
import { request } from "node:http";
import { createServer as createNetServer, type AddressInfo } from "node:net";
import { mkdir, mkdtemp, readdir, readFile, rm, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { GrantStore, type GrantMac, type MacEntry } from "../src/grants.ts";
import { readAllowBackground } from "../src/settings-file.ts";
import { launchSettings, SETTINGS_IDLE_MS, SETTINGS_POLL_MS, type SettingsServer } from "../src/settings-server.ts";
import { SETTINGS_TEXT, SETTINGS_TEXT_EN } from "../src/settings-strings.ts";
import type { UiLanguage } from "../src/ui-language.ts";
import { CJK, repoRoot, retryingSpawn, type FakeScript } from "./harness.ts";

// Ticket 112: `deskpass settings`, the local settings web page (read-only in this ticket). Seam:
// real HTTP requests to a settings server started in this process, its helper the scripted
// fake-helper (which signs grants.json entries with its fixed test key), its data folder the case's.

interface LaunchOptions {
  idleMs?: number;
  browserFails?: boolean;
  log?: (line: string) => void;
  menuRetryMs?: number;
  lang?: UiLanguage | null;
}

interface Case {
  dir: string;
  /** The URLs the server asked to open in the browser, in order. */
  opened: string[];
  /** Ticket 123: every helper process the server started (its own, the Start Menu's, each confirm's). */
  children: ChildProcess[];
  /** `lang` is the page's language, zh-TW unless given; null leaves it to the server's own decision (DESKPASS_LANG, Windows). */
  launch: (options?: LaunchOptions) => ReturnType<typeof launchSettings>;
  close: () => Promise<void>;
}

const servers: SettingsServer[] = [];

async function makeCase(script: FakeScript = {}): Promise<Case> {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const dir = await mkdtemp(join(root, "settings-"));
  const scriptPath = join(dir, "script.json");
  await writeFile(scriptPath, JSON.stringify({ ...script, logPath: join(dir, "requests.log") }));
  const opened: string[] = [];
  const children: ChildProcess[] = [];
  const spawnHelper = retryingSpawn();
  const fakeHelper = fileURLToPath(new URL("./fake-helper.mjs", import.meta.url));
  return {
    dir,
    opened,
    children,
    async launch({ idleMs = 60_000, browserFails = false, log, menuRetryMs, lang = "zh-TW" }: LaunchOptions = {}) {
      const result = await launchSettings({
        dataDir: dir,
        helperPath: process.execPath,
        helperArgs: [fakeHelper, "--script", scriptPath],
        spawn: ((...args: Parameters<typeof spawnHelper>) => {
          const child = spawnHelper(...args);
          children.push(child);
          return child;
        }) as typeof spawnHelper,
        openBrowser: async (url) => {
          opened.push(url);
          if (browserFails) throw new Error("no browser");
        },
        idleMs,
        ...(log ? { log } : {}),
        ...(menuRetryMs === undefined ? {} : { menuRetryMs }),
        ...(lang === null ? {} : { lang }),
      });
      if (result.started) servers.push(result.server);
      return result;
    },
    async close() {
      for (const server of servers.splice(0)) await server.close();
      await rm(dir, { recursive: true, force: true });
    },
  };
}

interface Reply {
  status: number;
  headers: Record<string, string | string[] | undefined>;
  body: string;
}

/** One HTTP request to 127.0.0.1:`port`, with exactly the headers given (Host included). */
function send(port: number, path: string, { method = "GET", headers = {}, body }: { method?: string; headers?: Record<string, string>; body?: string } = {}): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const req = request({ host: "127.0.0.1", port, path, method, headers: { host: `127.0.0.1:${port}`, ...headers } }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk: string) => {
        body += chunk;
      });
      res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body }));
    });
    req.on("error", reject);
    req.end(body);
  });
}

function portOf(url: string): number {
  return Number(new URL(url).port);
}

/** Exchanges the one-time URL `url` for the session cookie (`name=value`). */
async function exchange(url: string): Promise<{ reply: Reply; cookie: string }> {
  const parsed = new URL(url);
  const reply = await send(portOf(url), `${parsed.pathname}${parsed.search}`);
  const setCookie = reply.headers["set-cookie"];
  const first = Array.isArray(setCookie) ? setCookie[0] : setCookie;
  return { reply, cookie: (first ?? "").split(";")[0] ?? "" };
}

test("the browser is opened at a one-time URL on 127.0.0.1; its token becomes an HttpOnly SameSite=Strict cookie and the redirect drops it", async () => {
  const c = await makeCase();
  try {
    const result = await c.launch();
    assert.equal(result.started, true);
    assert.equal(c.opened.length, 1);
    const url = new URL(c.opened[0] ?? "");
    assert.equal(url.protocol, "http:");
    assert.equal(url.hostname, "127.0.0.1");
    assert.equal(url.pathname, "/open");
    const token = url.searchParams.get("t") ?? "";
    assert.ok(Buffer.from(token, "base64url").length >= 32, "the token has at least 32 random bytes");

    const { reply, cookie } = await exchange(c.opened[0] ?? "");
    assert.ok(reply.status === 302 || reply.status === 303, `a redirect, not ${reply.status}`);
    assert.equal(reply.headers.location, "/");
    const setCookie = String(reply.headers["set-cookie"]);
    assert.match(setCookie, /HttpOnly/i);
    assert.match(setCookie, /SameSite=Strict/i);
    assert.ok(!setCookie.includes(token), "the cookie is not the token");
    assert.ok(cookie.length > 0);

    const page = await send(url.port ? Number(url.port) : 80, "/", { headers: { cookie } });
    assert.equal(page.status, 200);
    assert.match(String(page.headers["content-type"]), /text\/html/);
    assert.ok(page.body.includes("設定已生效"));
  } finally {
    await c.close();
  }
});

/** No response carries a cross-origin header. */
function assertNoCors(reply: Reply, what: string): void {
  const cors = Object.keys(reply.headers).filter((name) => name.toLowerCase().startsWith("access-control-"));
  assert.deepEqual(cors, [], `${what}: no cross-origin headers`);
}

test("a used, made-up or missing token is refused, and the refusal does not say why", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const url = c.opened[0] ?? "";
    const port = portOf(url);
    const { reply: first } = await exchange(url);
    assert.equal(first.status, 303);
    const again = await exchange(url);
    assert.equal(again.reply.status, 403, "a token works once");
    assert.equal(again.reply.headers["set-cookie"], undefined);
    const madeUp = await send(port, `/open?t=${Buffer.alloc(32, 7).toString("base64url")}`);
    const missing = await send(port, "/open");
    assert.equal(madeUp.status, 403);
    assert.equal(missing.status, 403);
    assert.equal(madeUp.body, again.reply.body, "every refusal reads the same");
    assert.equal(missing.body, again.reply.body);
  } finally {
    await c.close();
  }
});

test("without the session cookie, or with a made-up one, nothing is served", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const port = portOf(c.opened[0] ?? "");
    for (const route of ["/", "/app.js", "/app.css", "/api/status", "/api/apps"]) {
      assert.equal((await send(port, route)).status, 403, `${route} without a cookie`);
      assert.equal((await send(port, route, { headers: { cookie: "deskpass_session=made-up" } })).status, 403, `${route} with a made-up cookie`);
    }
  } finally {
    await c.close();
  }
});

test("a request whose Host is not 127.0.0.1:<port> is refused, even with the cookie or a valid token", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const port = portOf(c.opened[0] ?? "");
    const { cookie } = await exchange(c.opened[0] ?? "");
    for (const host of [`localhost:${port}`, "127.0.0.1", `127.0.0.1:${port + 1}`, `evil.example:${port}`]) {
      assert.equal((await send(port, "/", { headers: { host, cookie } })).status, 403, `Host ${host}`);
      assert.equal((await send(port, "/api/apps", { headers: { host, cookie } })).status, 403, `Host ${host}`);
    }
    // A second tab's token, sent under a rebinding name, is refused.
    const second = await c.launch();
    assert.equal(second.started, false);
    const token = new URL(c.opened[1] ?? "").search;
    assert.equal((await send(port, `/open${token}`, { headers: { host: `rebound.example:${port}` } })).status, 403);
    assert.equal((await send(port, "/", { headers: { cookie } })).status, 200, "the right Host is served");
  } finally {
    await c.close();
  }
});

test("a POST needs the page's own Origin and the cookie; no response allows another origin", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const port = portOf(c.opened[0] ?? "");
    const origin = `http://127.0.0.1:${port}`;
    const { cookie } = await exchange(c.opened[0] ?? "");
    const post = (headers: Record<string, string>) => send(port, "/api/apps", { method: "POST", headers });
    const refused = [
      await post({ cookie }),
      await post({ cookie, origin: "http://evil.example" }),
      await post({ cookie, origin: `http://localhost:${port}` }),
      await post({ cookie, origin: "null" }),
      await post({ origin }),
    ];
    for (const reply of refused) assert.equal(reply.status, 403);
    // Past the checks there is no POST route yet (ticket 113 adds them): not found, not refused.
    const passed = await post({ cookie, origin });
    assert.equal(passed.status, 404);
    // A browser's preflight carries no cookie, and is refused without allowing anything.
    const preflight = await send(port, "/api/apps", { method: "OPTIONS", headers: { origin: "http://evil.example", "access-control-request-method": "POST" } });
    assert.equal(preflight.status, 403);
    const reads = [await send(port, "/", { headers: { cookie, origin: "http://evil.example" } }), await send(port, "/api/apps", { headers: { cookie, origin: "http://evil.example" } })];
    for (const [index, reply] of [...refused, passed, preflight, ...reads].entries()) assertNoCors(reply, `response ${index}`);
  } finally {
    await c.close();
  }
});

async function lockOf(dir: string): Promise<Record<string, unknown> | null> {
  try {
    return JSON.parse(await readFile(join(dir, "settings.lock"), "utf8")) as Record<string, unknown>;
  } catch {
    return null;
  }
}

/** A port nothing listens on any more. */
async function deadPort(): Promise<number> {
  const probe = createNetServer();
  await new Promise<void>((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const { port } = probe.address() as AddressInfo;
  await new Promise<void>((resolve) => probe.close(() => resolve()));
  return port;
}

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

test("one settings server at a time: a second launch asks the first for a new token, opens a tab and leaves the lock alone", async () => {
  const c = await makeCase();
  try {
    const first = await c.launch();
    assert.equal(first.started, true);
    const lock = await lockOf(c.dir);
    assert.ok(lock, "the lock is in the data folder");
    assert.equal(lock.port, portOf(c.opened[0] ?? ""));
    assert.equal(lock.pid, process.pid);
    assert.match(String(lock.secret), /^[A-Za-z0-9_-]{43}$/, "a 32-byte launch secret");

    const second = await c.launch();
    assert.equal(second.started, false);
    assert.equal(c.opened.length, 2);
    assert.equal(portOf(c.opened[1] ?? ""), portOf(c.opened[0] ?? ""), "the same server");
    assert.notEqual(new URL(c.opened[1] ?? "").searchParams.get("t"), new URL(c.opened[0] ?? "").searchParams.get("t"));
    for (const url of c.opened) assert.ok(!url.includes(String(lock.secret)), "the launch secret never leaves the lock");
    assert.deepEqual(await lockOf(c.dir), lock);

    const { reply, cookie } = await exchange(c.opened[1] ?? "");
    assert.equal(reply.status, 303, "the new tab's token works");
    assert.equal((await send(portOf(c.opened[1] ?? ""), "/", { headers: { cookie } })).status, 200);
    // The launch route wants the secret: without it, or with another, no token.
    const port = portOf(c.opened[0] ?? "");
    assert.equal((await send(port, "/launch", { method: "POST" })).status, 403);
    assert.equal((await send(port, "/launch", { method: "POST", headers: { "x-deskpass-launch": Buffer.alloc(32, 1).toString("base64url") } })).status, 403);
    assert.equal((await send(port, "/launch", { method: "POST", headers: { "x-deskpass-launch": String(lock.secret), origin: "http://evil.example" } })).status, 403);
  } finally {
    await c.close();
  }
});

test("a lock whose server is gone, or whose port answers without its secret, or that is not a lock, is cleared and a new server starts", async () => {
  const c = await makeCase();
  const other = await makeCase();
  try {
    const secret = Buffer.alloc(32, 9).toString("base64url");
    await writeFile(join(c.dir, "settings.lock"), JSON.stringify({ pid: 999_999, port: await deadPort(), secret, started_at: "2026-10-01T00:00:00.000Z" }));
    const started = await c.launch();
    assert.ok(started.started);
    const lock = await lockOf(c.dir);
    assert.equal(lock?.port, started.server.port);
    assert.notEqual(lock?.secret, secret);
    assert.equal(c.opened.length, 1);

    // A live port that does not know the lock's secret (another settings server) is not the lock's server.
    await writeFile(join(other.dir, "settings.lock"), JSON.stringify({ pid: process.pid, port: started.server.port, secret, started_at: "2026-10-01T00:00:00.000Z" }));
    const own = await other.launch();
    assert.ok(own.started, "started its own instead of opening the other server's tab");
    assert.notEqual(own.server.port, started.server.port);
    assert.equal((await lockOf(other.dir))?.port, own.server.port);
    await own.server.close();

    await writeFile(join(other.dir, "settings.lock"), "not a lock");
    const fresh = await other.launch();
    assert.ok(fresh.started);
    assert.equal((await lockOf(other.dir))?.port, fresh.server.port);
  } finally {
    await c.close();
    await other.close();
  }
});

test("with no authenticated request for the idle time, the server stops and removes its lock; the page's polling keeps it running", async () => {
  const c = await makeCase();
  try {
    const result = await c.launch({ idleMs: 500 });
    assert.ok(result.started);
    const { server } = result;
    const { cookie } = await exchange(c.opened[0] ?? "");
    let stopped = false;
    void server.closed.then(() => {
      stopped = true;
    });
    for (let index = 0; index < 10; index += 1) {
      await delay(150);
      assert.equal((await send(server.port, "/api/status", { headers: { cookie } })).status, 200, "still running while polled");
    }
    // Refused requests are not activity.
    for (let index = 0; index < 3 && !stopped; index += 1) {
      await delay(150);
      await send(server.port, "/api/status").catch(() => undefined);
    }
    await Promise.race([server.closed, delay(5_000).then(() => assert.fail("the server did not stop"))]);
    assert.equal(await lockOf(c.dir), null, "the lock is removed");
    await assert.rejects(send(server.port, "/api/status", { headers: { cookie } }), "nothing listens any more");
  } finally {
    await c.close();
  }
});

test("ticket 132: the idle stop waits 3 minutes, past a background tab's throttled poll (about once a minute), and the page polls at once when it is shown again", async () => {
  assert.equal(SETTINGS_IDLE_MS, 180_000);
  assert.ok(SETTINGS_IDLE_MS >= 2 * 60_000 + SETTINGS_POLL_MS, "two throttled polls fit in the idle time");
  const c = await makeCase();
  try {
    await c.launch();
    const tab = await tabOf(c);
    const script = (await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } })).body;
    assert.match(script, /addEventListener\("visibilitychange", function \(\) \{\s*if \(document\.visibilityState === "visible"\) poll\(\);/);
  } finally {
    await c.close();
  }
});

/** The fake helper's key (test/fake-helper.mjs `GRANT_KEY`), as in test/grants.test.ts. Test-only HMAC key, not a real key. */
const FAKE_GRANT_KEY = "0f1e2d3c4b5a69788796a5b4c3d2e1f000112233445566778899aabbccddeeff";
type Entry = Record<string, string>;
const hmac = (text: string) => createHmac("sha256", Buffer.from(FAKE_GRANT_KEY, "hex")).update(text, "utf8").digest("hex");
const canonical = (fields: Record<string, unknown>) => JSON.stringify(Object.fromEntries(Object.entries(fields).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))));
const sign = (entry: Entry): Entry => ({ ...entry, mac: hmac(canonical(entry)) });
function sealed(generation: number, grants: Entry[], raised: Entry[] = []) {
  const fields = { version: 1, generation, grant_macs: grants.map((entry) => entry.mac), raised_macs: raised.map((entry) => entry.mac) };
  return { version: 1, generation, grants, raised, file_mac: hmac(canonical(fields)) };
}

const CONTOSO = "C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd";
const MICROSOFT = "C=US, S=Washington, L=Redmond, O=Microsoft Corporation, CN=Microsoft Corporation";
const DEMO_KEY = `signer:${CONTOSO}|demo.exe`;
const NOTES_KEY = `signer:${CONTOSO}|notes.exe`;
const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const NOTEPAD = "C:\\Program Files\\WindowsApps\\Microsoft.WindowsNotepad_11.2409.9.0_x64__8wekyb3d8bbwe\\Notepad\\Notepad.exe";

const grantFile = () =>
  sealed(
    3,
    [
      sign({ key: DEMO_KEY, display_name: "Demo App", path: "C:\\Apps\\Demo.exe", created_at: "2026-09-01T00:00:00.000Z", last_used_at: "2026-10-15T08:00:00.000Z" }),
      sign({ key: NOTES_KEY, display_name: "Notes", path: "C:\\Apps\\Notes.exe", created_at: "2026-09-02T00:00:00.000Z", last_used_at: "2026-10-10T08:00:00.000Z" }),
    ],
    [
      sign({ key: "package:contoso.sap_abc123", tier: "blocked", created_at: "2026-09-03T00:00:00.000Z" }),
      sign({ key: NOTES_KEY, tier: "high", created_at: "2026-09-04T00:00:00.000Z" }),
      sign({ key: "package:aaa.zed_x1", tier: "high", created_at: "2026-09-05T00:00:00.000Z" }),
    ],
  );

const toolCall = (time: string, appPath: string, key: string) => JSON.stringify({ time, session: "s", event: "tool_call", tool: "computer_click", step: null, app: { path: appPath, key, tier: null }, action_summary: null, result: "ok", duration_ms: 1 });

/**
 * The name of the day file `daysAgo` local days before today (`YYYY-MM-DD`, as the log names them),
 * so the fixtures stay within the log's 30-day retention, which a settings write's log sweeps, on any
 * date. Only the file names are relative: the events' own times are what the assertions compare.
 */
function logDay(daysAgo: number): string {
  const date = new Date();
  date.setDate(date.getDate() - daysAgo);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/** Eight day files, the newest today: the seven newest name the apps; the oldest one's app is not listed. */
async function writeLogs(dir: string): Promise<void> {
  const logs = join(dir, "logs");
  await mkdir(logs, { recursive: true });
  const days: Record<string, string[]> = {
    [logDay(7)]: [toolCall("2026-10-17T00:00:00.000Z", "C:\\Old\\old.exe", "c:\\old\\old.exe")],
    [logDay(6)]: [toolCall("2026-10-09T00:00:00.000Z", "C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.21.0.0_x64__8wekyb3d8bbwe\\WindowsTerminal.exe", "package:microsoft.windowsterminal_8wekyb3d8bbwe")],
    [logDay(5)]: [toolCall("2026-10-13T00:00:00.000Z", "C:\\Windows\\explorer.exe", "c:\\windows\\explorer.exe")],
    [logDay(4)]: ["not json", JSON.stringify({ time: "2026-10-04T00:00:00.000Z", session: "s", event: "clipboard_read", chars: 3, truncated: false })],
    [logDay(3)]: [toolCall("2026-10-11T00:00:00.000Z", "C:\\Tools\\beta.exe", "c:\\tools\\beta.exe"), toolCall("2026-10-11T00:00:00.000Z", "C:\\Tools\\Alpha.exe", "c:\\tools\\alpha.exe")],
    [logDay(2)]: [toolCall("2026-10-12T00:00:00.000Z", NOTEPAD, "package:microsoft.windowsnotepad_8wekyb3d8bbwe")],
    [logDay(1)]: [toolCall("2026-10-14T00:00:00.000Z", "C:\\Apps\\Demo.exe", DEMO_KEY)],
    [logDay(0)]: [toolCall("2026-10-16T09:00:00.000Z", EDGE, `signer:${MICROSOFT}|msedge.exe`), toolCall("2026-10-16T08:00:00.000Z", EDGE, `signer:${MICROSOFT}|msedge.exe`)],
  };
  for (const [day, lines] of Object.entries(days)) await writeFile(join(logs, `${day}.jsonl`), `${lines.join("\n")}\n`);
}

/** The helper's `list_start_menu_apps` (ticket 116): every app here is one the AI used too. */
const startMenu = {
  apps: [
    { app_id: "uwp:Microsoft.WindowsNotepad_8wekyb3d8bbwe!App", name: "記事本", key: "package:microsoft.windowsnotepad_8wekyb3d8bbwe" },
    { app_id: "uwp:Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", name: "終端機", key: "package:microsoft.windowsterminal_8wekyb3d8bbwe" },
    { app_id: `exe:${EDGE}`, name: "Microsoft Edge", key: `signer:${MICROSOFT}|msedge.exe`, program: EDGE },
    { app_id: "shell:::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", name: "檔案總管", key: "c:\\windows\\explorer.exe", program: "C:\\Windows\\explorer.exe" },
  ],
};

const cookies = new WeakMap<Case, string>();

/** `/api/apps`, as the case's first tab (its token exchanged once). */
async function appsOf(c: Case): Promise<{ grants: { state: string; message: string | null }; apps: Record<string, unknown>[] }> {
  let cookie = cookies.get(c);
  if (cookie === undefined) {
    cookie = (await exchange(c.opened[0] ?? "")).cookie;
    cookies.set(c, cookie);
  }
  const reply = await send(portOf(c.opened[0] ?? ""), "/api/apps", { headers: { cookie } });
  assert.equal(reply.status, 200);
  assert.match(String(reply.headers["content-type"]), /application\/json/);
  return JSON.parse(reply.body);
}

test("the App list: grants, raises and the newest seven log days, each app once, newest use first, unused last, ties by name", async () => {
  const c = await makeCase({ handlers: { list_start_menu_apps: [{ result: startMenu }] } });
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    await writeFile(join(c.dir, "grants.json"), JSON.stringify(grantFile()));
    await writeLogs(c.dir);
    await c.launch();
    // The Start Menu's names are read in the background; the list is asked again until they are in.
    let listed = await appsOf(c);
    for (let attempt = 0; attempt < 50 && !listed.apps.some((app) => app.name === "記事本"); attempt += 1) {
      await delay(100);
      listed = await appsOf(c);
    }
    const { grants, apps } = listed;
    assert.deepEqual(grants, { state: "valid", message: null });
    const row = (app: Record<string, unknown>) => [app.name, app.publisher, app.tier, app.source, app.alwaysAllow, app.lastUsedAt];
    assert.deepEqual(apps.map(row), [
      ["Microsoft Edge", "Microsoft Corporation · 已簽章", "high", "builtIn", "na", "2026-10-16T09:00:00.000Z"],
      ["Demo App", "Contoso Ltd · 已簽章", "normal", "default", "yes", "2026-10-15T08:00:00.000Z"],
      ["檔案總管", "沒有可驗證的簽章", "high", "builtIn", "na", "2026-10-13T00:00:00.000Z"],
      ["記事本", "商店 App", "normal", "default", "no", "2026-10-12T00:00:00.000Z"],
      ["Alpha.exe", "沒有可驗證的簽章", "normal", "default", "na", "2026-10-11T00:00:00.000Z"],
      ["beta.exe", "沒有可驗證的簽章", "normal", "default", "na", "2026-10-11T00:00:00.000Z"],
      ["Notes", "Contoso Ltd · 已簽章", "high", "user", "na", "2026-10-10T08:00:00.000Z"],
      ["終端機", "商店 App", "blocked", "builtIn", "na", "2026-10-09T00:00:00.000Z"],
      ["aaa.zed", "商店 App", "high", "user", "na", null],
      ["contoso.sap", "商店 App", "blocked", "user", "na", null],
    ]);
    const byName = new Map(apps.map((app) => [app.name, app]));
    assert.equal(byName.get("檔案總管")?.why, "檔案總管：能開啟、搬移與刪除你的檔案，所以內建為高風險。");
    assert.equal(byName.get("終端機")?.why, "終端機與命令列：能執行任何程式，所以內建為封鎖。");
    assert.equal(byName.get("Notes")?.why, "你把它設為高風險（內建是一般）。");
    assert.equal(byName.get("Demo App")?.why, "一般 App。你在詢問視窗按過「永遠允許」，所以不再詢問。");
    assert.deepEqual(
      { key: byName.get("Demo App")?.key, path: byName.get("Demo App")?.path, grantCreatedAt: byName.get("Demo App")?.grantCreatedAt },
      { key: DEMO_KEY, path: "C:\\Apps\\Demo.exe", grantCreatedAt: "2026-09-01T00:00:00.000Z" },
    );
    assert.deepEqual(byName.get("終端機")?.builtIn, { tier: "blocked", label: "終端機與命令列" });

    // Coexistence with a running MCP server: this helper is only asked to sign and to name apps; it
    // never starts a session (the Esc hook) or shows the overlay.
    const methods = (await readFile(join(c.dir, "requests.log"), "utf8")).split("\n").filter(Boolean).map((line) => (JSON.parse(line) as { method: string }).method);
    assert.deepEqual([...new Set(methods)].sort(), ["grant_mac", "list_start_menu_apps", "ping"]);
  } finally {
    await c.close();
  }
});

test("a tampered grants file is reported invalid and its contents are not shown as if they counted", async () => {
  const c = await makeCase();
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    const file = grantFile();
    // The grants reordered: every entry still verifies, the file's own MAC does not.
    await writeFile(join(c.dir, "grants.json"), JSON.stringify({ ...file, grants: [...file.grants].reverse() }));
    await mkdir(join(c.dir, "logs"), { recursive: true });
    await writeFile(join(c.dir, "logs", `${logDay(1)}.jsonl`), `${toolCall("2026-10-14T00:00:00.000Z", "C:\\Apps\\Demo.exe", DEMO_KEY)}\n`);
    await c.launch();
    const voided = await appsOf(c);
    assert.equal(voided.grants.state, "void");
    assert.match(voided.grants.message ?? "", /沒有通過驗證/);
    assert.match(voided.grants.message ?? "", /簽章不符/);
    assert.deepEqual(
      voided.apps.map((app) => [app.name, app.alwaysAllow, app.tier, app.source]),
      [["Demo.exe", "no", "normal", "default"]],
      "only the log's app, with neither its grant nor any raise",
    );

    // One entry edited: it is ignored and said so; the rest of the file counts.
    const edited = grantFile();
    edited.grants[1] = { ...edited.grants[1]!, display_name: "Renamed" };
    await writeFile(join(c.dir, "grants.json"), JSON.stringify(edited));
    await utimes(join(c.dir, "grants.json"), new Date("2026-10-20T00:00:00Z"), new Date("2026-10-20T00:00:00Z"));
    const partly = await appsOf(c);
    assert.equal(partly.grants.state, "valid");
    assert.match(partly.grants.message ?? "", /1 筆/);
    assert.deepEqual(partly.apps.find((app) => app.key === DEMO_KEY)?.alwaysAllow, "yes");
    assert.equal(partly.apps.find((app) => app.name === "Renamed"), undefined);
  } finally {
    await c.close();
  }
});

test("a hold keeps an idle server running until it is released (ticket 114's confirm window)", async () => {
  const c = await makeCase();
  try {
    const result = await c.launch({ idleMs: 200 });
    assert.ok(result.started);
    // Ticket 113: an unused token keeps the server waiting for its browser, so the tab opens first.
    assert.equal((await exchange(c.opened[0] ?? "")).reply.status, 303);
    const release = result.server.hold();
    let stopped = false;
    void result.server.closed.then(() => {
      stopped = true;
    });
    await delay(700);
    assert.equal(stopped, false);
    release();
    await Promise.race([result.server.closed, delay(5_000).then(() => assert.fail("the server did not stop"))]);
    assert.equal(await lockOf(c.dir), null);
  } finally {
    await c.close();
  }
});

// Ticket 113: the page tightens. POST /api/raise ({ key, tier }) and /api/revoke ({ key }) write
// grants.json through the Node grants module (the fake helper signs), only ever stricter; each
// write is one log event of session `settings`.

interface Tab {
  port: number;
  origin: string;
  cookie: string;
}

/** The case's first tab: its port, its origin and its session cookie. */
async function tabOf(c: Case): Promise<Tab> {
  let cookie = cookies.get(c);
  if (cookie === undefined) {
    cookie = (await exchange(c.opened[0] ?? "")).cookie;
    cookies.set(c, cookie);
  }
  const port = portOf(c.opened[0] ?? "");
  return { port, origin: `http://127.0.0.1:${port}`, cookie };
}

/** A POST as the page sends it: its Origin, its cookie, a JSON body; `headers` adds to or replaces those (an empty value drops one). */
async function postJson(tab: Tab, route: string, body: unknown, headers: Record<string, string> = {}): Promise<Reply & { json: Record<string, unknown> | null }> {
  const all: Record<string, string> = { cookie: tab.cookie, origin: tab.origin, "content-type": "application/json", "sec-fetch-site": "same-origin", ...headers };
  const reply = await send(tab.port, route, {
    method: "POST",
    headers: Object.fromEntries(Object.entries(all).filter(([, value]) => value !== "")),
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
  let json: Record<string, unknown> | null = null;
  try {
    json = JSON.parse(reply.body) as Record<string, unknown>;
  } catch {
    json = null;
  }
  return { ...reply, json };
}

/** A GrantStore as the MCP server keeps one, its MACs made here with the fake helper's key. */
function mcpStore(dir: string): { store: GrantStore; mac: GrantMac } {
  const store = new GrantStore({ dir, record: async () => undefined });
  const mac: GrantMac = async ({ entries }: { entries: MacEntry[] }) => ({ macs: entries.map((entry) => hmac(canonical(entry.fields as unknown as Record<string, unknown>))), key_state: "ok" });
  return { store, mac };
}

/** The log events of the settings server (session `settings`), in order. */
async function settingsEvents(dir: string): Promise<Record<string, unknown>[]> {
  const logs = join(dir, "logs");
  let names: string[] = [];
  try {
    names = (await readdir(logs)).filter((name) => name.endsWith(".jsonl")).sort();
  } catch {
    return [];
  }
  const events: Record<string, unknown>[] = [];
  for (const name of names) {
    for (const line of (await readFile(join(logs, name), "utf8")).split("\n")) {
      if (!line) continue;
      try {
        const event = JSON.parse(line) as Record<string, unknown>;
        if (event.session === "settings") events.push(event);
      } catch {
        // Not JSON (a fixture line).
      }
    }
  }
  return events;
}

/** An event without its `time`, which must be an ISO time. */
function untimed(event: Record<string, unknown>): Record<string, unknown> {
  const { time, ...rest } = event;
  assert.ok(typeof time === "string" && !Number.isNaN(Date.parse(time)), "an ISO time");
  return rest;
}

type WrittenFile = { version: number; generation: number; grants: Entry[]; raised: Entry[]; file_mac: string };
const readGrants = async (dir: string) => JSON.parse(await readFile(join(dir, "grants.json"), "utf8")) as WrittenFile;

async function tighteningCase(): Promise<Case> {
  const c = await makeCase();
  await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
  await writeFile(join(c.dir, "grants.json"), JSON.stringify(grantFile()));
  await writeLogs(c.dir);
  await c.launch();
  return c;
}

test("設為高風險 on an always-allowed app raises it and revokes its grant in the same write; a running MCP server's store sees it at its next check", async () => {
  const c = await tighteningCase();
  try {
    const tab = await tabOf(c);
    const page = await send(tab.port, "/", { headers: { cookie: tab.cookie } });
    for (const label of ["設為高風險", "設為封鎖", "取消永遠允許"]) assert.ok(page.body.includes(label), `the page has ${label}`);

    // The MCP server's store, as it was before the page's write.
    const mcp = mcpStore(c.dir);
    assert.equal((await (await mcp.store.current(mcp.mac)).tierFor(DEMO_KEY, { tier: "normal", why: null })).tier, "normal");
    assert.equal((await mcp.store.current(mcp.mac)).granted(DEMO_KEY), true);

    const reply = await postJson(tab, "/api/raise", { key: DEMO_KEY, tier: "high" });
    assert.equal(reply.status, 200, reply.body);
    assert.equal(reply.json?.ok, true);
    assert.equal(reply.json?.grantRemoved, true);
    assert.match(String(reply.json?.message), /Demo App/);
    assert.match(String(reply.json?.message), /高風險/);
    assert.match(String(reply.json?.message), /永遠允許已一併取消/);
    assertNoCors(reply, "the raise");

    const written = await readGrants(c.dir);
    assert.equal(written.generation, 4, "the next generation");
    assert.deepEqual(written.grants.map((entry) => entry.key), [NOTES_KEY], "the grant is gone");
    const raise = written.raised.find((entry) => entry.key === DEMO_KEY);
    assert.equal(raise?.tier, "high");
    assert.ok(!Number.isNaN(Date.parse(raise?.created_at ?? "")));

    // The same store, at its next check: the change is seen, and the file verifies whole.
    const view = await mcp.store.current(mcp.mac);
    assert.deepEqual(view.verdict(), { state: "valid", ignored: 0 });
    assert.equal((await view.tierFor(DEMO_KEY, { tier: "normal", why: null })).tier, "high");
    assert.equal(view.granted(DEMO_KEY), false);
    // A fresh store (an MCP server started now) verifies it too.
    const fresh = mcpStore(c.dir);
    assert.deepEqual((await fresh.store.current(fresh.mac)).verdict(), { state: "valid", ignored: 0 });

    const row = (await appsOf(c)).apps.find((app) => app.key === DEMO_KEY);
    assert.deepEqual([row?.tier, row?.source, row?.alwaysAllow], ["high", "user", "na"]);
    assert.deepEqual((await settingsEvents(c.dir)).map(untimed), [
      { session: "settings", event: "tier_raised", key: DEMO_KEY, path: "C:\\Apps\\Demo.exe", tier: "high", grant_removed: true },
    ]);
  } finally {
    await c.close();
  }
});

test("設為封鎖 on a user-raised app and on a built-in high one; 取消永遠允許 removes only the grant", async () => {
  const c = await tighteningCase();
  try {
    const tab = await tabOf(c);
    const edgeKey = `signer:${MICROSOFT}|msedge.exe`;
    const edge = await postJson(tab, "/api/raise", { key: edgeKey, tier: "blocked" });
    assert.equal(edge.status, 200, edge.body);
    const zed = await postJson(tab, "/api/raise", { key: "package:aaa.zed_x1", tier: "blocked" });
    assert.equal(zed.status, 200, zed.body);
    assert.equal(zed.json?.grantRemoved, false);
    assert.doesNotMatch(String(zed.json?.message), /永遠允許/);
    const revoked = await postJson(tab, "/api/revoke", { key: DEMO_KEY });
    assert.equal(revoked.status, 200, revoked.body);
    assert.match(String(revoked.json?.message), /Demo App/);
    assert.match(String(revoked.json?.message), /永遠允許/);

    const written = await readGrants(c.dir);
    assert.equal(written.generation, 6, "one generation a write");
    assert.deepEqual(written.grants.map((entry) => entry.key), [NOTES_KEY]);
    assert.deepEqual(
      written.raised.map((entry) => [entry.key, entry.tier]).sort(),
      [["package:aaa.zed_x1", "blocked"], ["package:contoso.sap_abc123", "blocked"], [NOTES_KEY, "high"], [edgeKey, "blocked"]].sort(),
    );
    const fresh = mcpStore(c.dir);
    const view = await fresh.store.current(fresh.mac);
    assert.deepEqual(view.verdict(), { state: "valid", ignored: 0 });
    assert.equal((await view.tierFor(edgeKey, { tier: "high", why: "a web browser" })).tier, "blocked");
    assert.equal(view.granted(DEMO_KEY), false);
    assert.equal((await view.tierFor(DEMO_KEY, { tier: "normal", why: null })).tier, "normal", "a revoke leaves the tier alone");

    assert.deepEqual((await settingsEvents(c.dir)).map(untimed), [
      { session: "settings", event: "tier_raised", key: edgeKey, path: EDGE, tier: "blocked", grant_removed: false },
      { session: "settings", event: "tier_raised", key: "package:aaa.zed_x1", tier: "blocked", grant_removed: false },
      { session: "settings", event: "grant_revoked", key: DEMO_KEY, path: "C:\\Apps\\Demo.exe" },
    ]);
  } finally {
    await c.close();
  }
});

test("no request lowers a tier, keeps it, goes below the built-in one, adds always-allow or names an unknown app; the file and the log are untouched", async () => {
  const c = await tighteningCase();
  try {
    const tab = await tabOf(c);
    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const edgeKey = `signer:${MICROSOFT}|msedge.exe`;
    const terminal = "package:microsoft.windowsterminal_8wekyb3d8bbwe";
    const rules: [string, unknown, string][] = [
      ["/api/raise", { key: edgeKey, tier: "high" }, "built_in"],
      ["/api/raise", { key: terminal, tier: "high" }, "built_in"],
      ["/api/raise", { key: terminal, tier: "blocked" }, "built_in"],
      ["/api/raise", { key: "package:contoso.sap_abc123", tier: "high" }, "not_stricter"],
      ["/api/raise", { key: "package:aaa.zed_x1", tier: "high" }, "not_stricter"],
      ["/api/raise", { key: "signer:CN=Nobody|nobody.exe", tier: "blocked" }, "unknown_app"],
      ["/api/revoke", { key: "c:\\tools\\alpha.exe" }, "no_grant"],
      ["/api/revoke", { key: "signer:CN=Nobody|nobody.exe" }, "unknown_app"],
    ];
    for (const [route, body, reason] of rules) {
      const reply = await postJson(tab, route, body);
      assert.equal(reply.status, 409, `${route} ${JSON.stringify(body)}: ${reply.body}`);
      assert.equal(reply.json?.ok, false);
      assert.equal(reply.json?.reason, reason, `${route} ${JSON.stringify(body)}`);
      assert.ok(String(reply.json?.message).length > 0, "said why");
    }
    const malformed: [string, unknown, Record<string, string>?][] = [
      ["/api/raise", { key: DEMO_KEY, tier: "normal" }],
      ["/api/raise", { key: DEMO_KEY, tier: "always" }],
      ["/api/raise", { key: DEMO_KEY, tier: "high", alwaysAllow: true }],
      ["/api/raise", { key: DEMO_KEY }],
      ["/api/raise", { key: 7, tier: "high" }],
      ["/api/revoke", { key: DEMO_KEY, grant: true }],
      ["/api/raise", "not json"],
      ["/api/raise", { key: DEMO_KEY, tier: "high" }, { "content-type": "text/plain" }],
    ];
    for (const [route, body, headers] of malformed) {
      const reply = await postJson(tab, route, body, headers);
      assert.equal(reply.status, 400, `${route} ${JSON.stringify(body)}: ${reply.body}`);
    }
    // There is no route that adds always-allow.
    for (const route of ["/api/grant", "/api/always-allow", "/api/allow", "/api/lower"]) {
      assert.equal((await postJson(tab, route, { key: DEMO_KEY, tier: "normal" })).status, 404, route);
    }
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "grants.json is untouched");
    assert.deepEqual(await settingsEvents(c.dir), [], "nothing logged");
  } finally {
    await c.close();
  }
});

test("ticket 133: a Start Menu app the AI never used is not listed and is refused like any unknown app; the apps the AI used keep their Start Menu names", async () => {
  const SAP = "C:\\Program Files (x86)\\SAP\\FrontEnd\\SAPgui\\saplogon.exe";
  const sapKey = "signer:C=DE, O=SAP SE, CN=SAP SE|saplogon.exe";
  const settingsKey = "package:windows.immersivecontrolpanel_cw5n1h2txyewy";
  const eventViewerKey = "shell:Microsoft.AutoGenerated.{EAD50E1F}";
  const menu = {
    apps: [
      ...startMenu.apps,
      { app_id: `exe:${SAP}`, name: "SAP Logon", key: sapKey, program: SAP },
      { app_id: "shell:Microsoft.AutoGenerated.{EAD50E1F}", name: "事件檢視器", key: null },
      { app_id: "uwp:windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel", name: "設定", key: settingsKey },
    ],
  };
  const c = await makeCase({ handlers: { list_start_menu_apps: [{ result: menu }] } });
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    await writeFile(join(c.dir, "grants.json"), JSON.stringify(grantFile()));
    await writeLogs(c.dir);
    await c.launch();
    const tab = await tabOf(c);
    let listed = (await appsOf(c)) as Awaited<ReturnType<typeof appsOf>> & { menu?: string };
    for (let attempt = 0; attempt < 50 && listed.menu !== "ready"; attempt += 1) {
      await delay(100);
      listed = await appsOf(c);
    }
    assert.equal(listed.menu, "ready", "the Start Menu is listed, for the names");
    const rowOf = (apps: Record<string, unknown>[], key: string) => apps.find((app) => app.key === key);
    for (const key of [sapKey, eventViewerKey, settingsKey]) assert.equal(rowOf(listed.apps, key), undefined, `${key} is not listed`);
    assert.ok(!listed.apps.some((app) => app.name === "SAP Logon" || app.name === "事件檢視器" || app.name === "設定"));
    // An app the AI used is named by the Start Menu (the log has no name for it).
    assert.equal(rowOf(listed.apps, "package:microsoft.windowsnotepad_8wekyb3d8bbwe")?.name, "記事本");
    assert.equal(listed.apps.filter((app) => app.key === `signer:${MICROSOFT}|msedge.exe`).length, 1, "one row an app");
    assert.equal(listed.apps.length, 10, "only the apps of grants.json and the log");

    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    for (const key of [sapKey, eventViewerKey, settingsKey]) {
      for (const route of ["/api/raise", "/api/revoke", "/api/restore"]) {
        const reply = await postJson(tab, route, route === "/api/raise" ? { key, tier: "blocked" } : { key });
        assert.equal(reply.status, 409, `${route} ${key}: ${reply.body}`);
        assert.equal(reply.json?.reason, "unknown_app", `${route} ${key}`);
      }
    }
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "nothing written for a refusal");
    assert.deepEqual(await settingsEvents(c.dir), [], "nothing logged");

    // An app the AI used can still be raised.
    const reply = await postJson(tab, "/api/raise", { key: DEMO_KEY, tier: "blocked" });
    assert.equal(reply.status, 200, reply.body);
  } finally {
    await c.close();
  }
});

test("ticket 133: the page has the filters AI 操作過, 你設定過, 封鎖 and 高風險, and no 全部", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const tab = await tabOf(c);
    const html = (await send(tab.port, "/", { headers: { cookie: tab.cookie } })).body;
    const script = (await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } })).body;
    const page = html + script;
    assert.deepEqual(Object.values(SETTINGS_TEXT.page.filters), ["AI 操作過", "你設定過", "封鎖", "高風險"]);
    assert.deepEqual(Object.keys(SETTINGS_TEXT.page.filterNotes), Object.keys(SETTINGS_TEXT.page.filters));
    assert.ok(page.includes('var FILTERS = ["used", "mine", "blocked", "high"];'), "the script's filters");
    assert.ok(!page.includes('"all"'), "no filter named all");
    assert.ok(!page.includes('"全部"'), "no 全部 string (the log's 「全部 App」 is another)");
    assert.ok(!page.includes("全部」"), "no 全部 note");
  } finally {
    await c.close();
  }
});

test("while grants.json does not verify, no change is written over it (as the control panel refused): the raises a running MCP server still keeps are not dropped", async () => {
  const c = await makeCase();
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    const file = grantFile();
    const tampered = JSON.stringify({ ...file, grants: [...file.grants].reverse() });
    await writeFile(join(c.dir, "grants.json"), tampered);
    await writeLogs(c.dir);
    await c.launch();
    const tab = await tabOf(c);
    const reply = await postJson(tab, "/api/raise", { key: `signer:${MICROSOFT}|msedge.exe`, tier: "blocked" });
    assert.equal(reply.status, 409, reply.body);
    assert.equal(reply.json?.reason, "grants_void");
    assert.match(String(reply.json?.message), /grants\.json/);
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), tampered, "the void file is left for the user");
    assert.deepEqual(await settingsEvents(c.dir), []);
  } finally {
    await c.close();
  }
});

test("a write needs the page's own Origin, the cookie and no cross-site Sec-Fetch-Site; any request with a foreign Sec-Fetch-Site is refused", async () => {
  const c = await tighteningCase();
  try {
    const tab = await tabOf(c);
    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const body = { key: DEMO_KEY, tier: "blocked" };
    const refused = [
      await postJson(tab, "/api/raise", body, { origin: "" }),
      await postJson(tab, "/api/raise", body, { origin: "http://evil.example" }),
      await postJson(tab, "/api/raise", body, { origin: `http://127.0.0.1:${tab.port + 1}` }),
      await postJson(tab, "/api/raise", body, { cookie: "" }),
      await postJson(tab, "/api/raise", body, { "sec-fetch-site": "cross-site" }),
      await postJson(tab, "/api/raise", body, { "sec-fetch-site": "same-site" }),
      await postJson(tab, "/api/revoke", { key: DEMO_KEY }, { "sec-fetch-site": "same-site" }),
    ];
    for (const [index, reply] of refused.entries()) {
      assert.equal(reply.status, 403, `request ${index}`);
      assertNoCors(reply, `request ${index}`);
    }
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before);
    // Reads and the token exchange too.
    assert.equal((await send(tab.port, "/api/apps", { headers: { cookie: tab.cookie, "sec-fetch-site": "cross-site" } })).status, 403);
    assert.equal((await send(tab.port, "/", { headers: { cookie: tab.cookie, "sec-fetch-site": "same-site" } })).status, 403);
    const second = await c.launch();
    assert.equal(second.started, false);
    const token = new URL(c.opened[1] ?? "").search;
    assert.equal((await send(tab.port, `/open${token}`, { headers: { "sec-fetch-site": "cross-site" } })).status, 403);
    // A browser's own navigation (none) and the page's fetches (same-origin) are served.
    assert.equal((await send(tab.port, "/", { headers: { cookie: tab.cookie, "sec-fetch-site": "none" } })).status, 200);
    assert.equal((await send(tab.port, "/api/apps", { headers: { cookie: tab.cookie, "sec-fetch-site": "same-origin" } })).status, 200);
    assert.equal((await postJson(tab, "/api/raise", body)).status, 200);
  } finally {
    await c.close();
  }
});

test("a request with several deskpass_session cookies is served when any of them is a live session", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const tab = await tabOf(c);
    const value = tab.cookie.slice(tab.cookie.indexOf("=") + 1);
    for (const header of [`deskpass_session=tossed; ${tab.cookie}`, `${tab.cookie}; deskpass_session=tossed`, `other=1; deskpass_session=; deskpass_session=${value}`]) {
      assert.equal((await send(tab.port, "/api/status", { headers: { cookie: header } })).status, 200, header);
    }
    assert.equal((await send(tab.port, "/api/status", { headers: { cookie: "deskpass_session=tossed; deskpass_session=also" } })).status, 403);
  } finally {
    await c.close();
  }
});

test("before its first tab opens, the server waits at least the token's lifetime, as the message says, and its token still works", async () => {
  const c = await makeCase();
  try {
    const lines: string[] = [];
    const result = await c.launch({ idleMs: 200, browserFails: true, log: (line) => lines.push(line) });
    assert.ok(result.started);
    assert.ok(lines.some((line) => line.includes(c.opened[0] ?? "-") && line.includes("within 2 minutes")), lines.join("\n"));
    let stopped = false;
    void result.server.closed.then(() => {
      stopped = true;
    });
    await delay(800);
    assert.equal(stopped, false, "still waiting for the browser");
    assert.ok(await lockOf(c.dir));
    assert.equal((await exchange(c.opened[0] ?? "")).reply.status, 303, "the token still works");
    // Once the tab has opened, the usual idle time applies.
    await Promise.race([result.server.closed, delay(5_000).then(() => assert.fail("the server did not stop"))]);
  } finally {
    await c.close();
  }
});

test("ticket 116: a failed Start Menu listing (a helper without the method) leaves the list working and is asked again, of a helper of its own", async () => {
  const unknown = { error: { code: "unknown_method", message: "unknown method list_start_menu_apps" } };
  const c = await makeCase({ handlers: { list_start_menu_apps: [unknown, unknown, { result: startMenu }] } });
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    await writeFile(join(c.dir, "grants.json"), JSON.stringify(grantFile()));
    await c.launch({ menuRetryMs: 400 });
    const tab = await tabOf(c);
    const menuOf = async () => ((await appsOf(c)) as Awaited<ReturnType<typeof appsOf>> & { menu: string }).menu;
    let menu = await menuOf();
    for (let attempt = 0; attempt < 50 && menu !== "failed"; attempt += 1) {
      await delay(50);
      menu = await menuOf();
    }
    assert.equal(menu, "failed");
    assert.equal(await menuOf(), "failed", "not asked again before the gap");

    // A raise of an app the AI used does not wait on the listing.
    const reply = await postJson(tab, "/api/raise", { key: DEMO_KEY, tier: "high" });
    assert.equal(reply.status, 200, reply.body);

    // After the gap: asked again (failing once more), then again, now listed.
    for (let attempt = 0; attempt < 100 && menu !== "ready"; attempt += 1) {
      await delay(100);
      menu = await menuOf();
    }
    assert.equal(menu, "ready");
    const requests = (await readFile(join(c.dir, "requests.log"), "utf8")).split("\n").filter(Boolean).map((line) => JSON.parse(line) as { method: string });
    // At least three listings: the fake helpers share one script, and the server's helper, saving
    // its own copy of the queue, can put a failure back that a listing helper had taken.
    const listings = requests.filter((request) => request.method === "list_start_menu_apps").length;
    assert.ok(listings >= 3, JSON.stringify(requests.map((request) => request.method)));
    // Each listing is a helper of its own: one ping for the server's helper and one a listing.
    assert.equal(requests.filter((request) => request.method === "ping").length, listings + 1);
  } finally {
    await c.close();
  }
});

test("the page's revision changes when grants.json is changed elsewhere (an approval dialog's always-allow)", async () => {
  const c = await tighteningCase();
  try {
    const tab = await tabOf(c);
    const revisionOf = async () => JSON.parse((await send(tab.port, "/api/status", { headers: { cookie: tab.cookie } })).body).revision as string;
    const first = await revisionOf();
    assert.equal(await revisionOf(), first, "steady while nothing changes");
    const file = grantFile();
    const added = sign({ key: `signer:${CONTOSO}|third.exe`, display_name: "Third", path: "C:\\Apps\\Third.exe", created_at: "2026-10-20T00:00:00.000Z", last_used_at: "2026-10-20T00:00:00.000Z" });
    await writeFile(join(c.dir, "grants.json"), JSON.stringify(sealed(4, [...file.grants, added], file.raised)));
    await utimes(join(c.dir, "grants.json"), new Date("2026-10-20T00:00:00Z"), new Date("2026-10-20T00:00:00Z"));
    const second = await revisionOf();
    assert.notEqual(second, first);
    assert.ok((await appsOf(c)).apps.some((app) => app.name === "Third"));
  } finally {
    await c.close();
  }
});

// Ticket 117: the Log tab. GET /api/log/days lists today and the day files of the 7 days before it;
// GET /api/log?day=YYYY-MM-DD[&app=key] gives that day as groups of sentences. Fixture files are named
// by days relative to today (`logDay`); the events' own times only have to be ISO times.

const logEvent = (time: string, fields: Record<string, unknown>) => JSON.stringify({ time, session: "s", ...fields });
const NOTEPAD_KEY = "package:microsoft.windowsnotepad_8wekyb3d8bbwe";
const EDGE_KEY = `signer:${MICROSOFT}|msedge.exe`;
const notepadCall = (time: string, tool = "computer_click") => logEvent(time, { event: "tool_call", tool, app: { path: NOTEPAD, key: NOTEPAD_KEY, tier: null }, action_summary: null, result: "ok" });
const edgeApproval = (time: string) => logEvent(time, { event: "approval", channel: "dialog", answer: "allow_once", app: { path: EDGE, key: EDGE_KEY, tier: "high" } });

async function writeLogDays(dir: string, days: Record<string, string[]>): Promise<void> {
  const logs = join(dir, "logs");
  await mkdir(logs, { recursive: true });
  for (const [day, lines] of Object.entries(days)) await writeFile(join(logs, `${day}.jsonl`), `${lines.join("\n")}\n`);
}

async function logCase(days: Record<string, string[]>, lang: UiLanguage | null = "zh-TW"): Promise<{ c: Case; tab: Tab; get: (route: string, headers?: Record<string, string>) => Promise<Reply & { json: any }> }> {
  const c = await makeCase({ handlers: { list_start_menu_apps: [{ result: startMenu }] } });
  await writeLogDays(c.dir, days);
  await c.launch({ lang });
  const tab = await tabOf(c);
  const get = async (route: string, headers: Record<string, string> = { cookie: tab.cookie }) => {
    const reply = await send(tab.port, route, { headers });
    let json: any = null;
    try {
      json = JSON.parse(reply.body);
    } catch {
      json = null;
    }
    return { ...reply, json };
  };
  return { c, tab, get };
}

test("/api/log/days: today, then the day files of the 7 days before it, newest first; the newest day with rows; the retention sentence", async () => {
  const { c, get } = await logCase({
    [logDay(0)]: [notepadCall("2026-10-21T08:00:00.000Z")],
    [logDay(1)]: [notepadCall("2026-10-20T08:00:00.000Z")],
    [logDay(3)]: ["not json"],
    [logDay(7)]: [notepadCall("2026-10-14T08:00:00.000Z")],
    [logDay(8)]: [notepadCall("2026-10-13T08:00:00.000Z")],
  });
  try {
    const reply = await get("/api/log/days");
    assert.equal(reply.status, 200);
    assert.match(String(reply.headers["content-type"]), /application\/json/);
    assert.deepEqual(reply.json.days, [logDay(0), logDay(1), logDay(3), logDay(7)]);
    assert.equal(reply.json.today, logDay(0));
    assert.equal(reply.json.latest, logDay(0));
    assert.match(reply.json.retention, /^紀錄保留 \d+ 天/);
    assertNoCors(reply, "/api/log/days");
  } finally {
    await c.close();
  }
});

test("/api/log/days: today is listed with no file, and latest points to the newest day that has rows", async () => {
  const { c, get } = await logCase({ [logDay(1)]: ["not json"], [logDay(2)]: [notepadCall("2026-10-19T08:00:00.000Z")] });
  try {
    const reply = await get("/api/log/days");
    assert.deepEqual(reply.json.days, [logDay(0), logDay(1), logDay(2)]);
    assert.equal(reply.json.latest, logDay(2));
  } finally {
    await c.close();
  }
});

test("/api/log/days: no log at all gives today only and no latest day", async () => {
  const { c, get } = await logCase({});
  try {
    const reply = await get("/api/log/days");
    assert.equal(reply.status, 200);
    assert.deepEqual(reply.json.days, [logDay(0)]);
    assert.equal(reply.json.latest, null);
    const day = await get(`/api/log?day=${logDay(0)}`);
    assert.equal(day.status, 200);
    assert.deepEqual(day.json.groups, []);
  } finally {
    await c.close();
  }
});

test("/api/log: a day as groups of one app's consecutive events, with sentences and tags, apps named as the App page names them", async () => {
  const { c, get } = await logCase({
    [logDay(0)]: [
      notepadCall("2026-10-21T08:00:00.000Z", "computer_screenshot"),
      notepadCall("2026-10-21T08:01:00.000Z"),
      edgeApproval("2026-10-21T08:02:00.000Z"),
      notepadCall("2026-10-21T08:03:00.000Z", "computer_type_text"),
    ],
  });
  try {
    // The Start Menu's names are read in the background, as the App page's are; the log shows the same name.
    let day = await get(`/api/log?day=${logDay(0)}`);
    for (let attempt = 0; attempt < 50 && !day.json.apps.some((app: { name: string }) => app.name === "記事本"); attempt += 1) {
      await delay(100);
      day = await get(`/api/log?day=${logDay(0)}`);
    }
    assert.equal(day.status, 200);
    assert.equal(day.json.day, logDay(0));
    assert.equal(day.json.truncated, false);
    assert.deepEqual(day.json.apps, [
      { key: EDGE_KEY, name: "Microsoft Edge" },
      { key: NOTEPAD_KEY, name: "記事本" },
    ]);
    assert.deepEqual(
      day.json.groups.map((g: { app: string; count: number; rows: { sentence: string; tag?: string }[] }) => [g.app, g.count, g.rows.map((row) => [row.sentence, row.tag])]),
      [
        ["記事本", 2, [["截圖", "成功"], ["點擊", "成功"]]],
        ["Microsoft Edge", 1, [["詢問核准", "你允許一次"]]],
        ["記事本", 1, [["輸入文字", "成功"]]],
      ],
      "chronological, a run of one app is one group",
    );
    for (const group of day.json.groups) {
      assert.match(group.first, /^\d\d:\d\d$/);
      assert.match(group.last, /^\d\d:\d\d$/);
    }
  } finally {
    await c.close();
  }
});

test("/api/log: the app filter keeps one app's groups and still lists every app of the day", async () => {
  const { c, get } = await logCase({
    [logDay(0)]: [notepadCall("2026-10-21T08:00:00.000Z"), edgeApproval("2026-10-21T08:02:00.000Z"), notepadCall("2026-10-21T08:03:00.000Z")],
  });
  try {
    const day = await get(`/api/log?day=${logDay(0)}&app=${encodeURIComponent(EDGE_KEY)}`);
    assert.equal(day.status, 200);
    assert.deepEqual(day.json.groups.map((g: { key: string; count: number }) => [g.key, g.count]), [[EDGE_KEY, 1]]);
    assert.deepEqual(day.json.apps.map((app: { key: string }) => app.key).sort(), [EDGE_KEY, NOTEPAD_KEY].sort());
    // An app that did not appear that day is not a filter of it.
    const other = await get(`/api/log?day=${logDay(0)}&app=${encodeURIComponent("package:not.there_x")}`);
    assert.equal(other.status, 404);
    assert.equal((await get(`/api/log?day=${logDay(0)}&app=`)).status, 400);
  } finally {
    await c.close();
  }
});

test("/api/log: a day that is not written exactly YYYY-MM-DD is 400, a day that is not on offer is 404", async () => {
  const { c, get } = await logCase({ [logDay(0)]: [notepadCall("2026-10-21T08:00:00.000Z")], [logDay(8)]: [notepadCall("2026-10-13T08:00:00.000Z")] });
  try {
    for (const route of ["/api/log", "/api/log?day=", "/api/log?day=2026-1-1", `/api/log?day=${logDay(0)}x`, "/api/log?day=..%2F..%2Fgrants", `/api/log?day=${logDay(0)}&day=${logDay(0)}`, "/api/log?day=2026-13-45", `/api/log?day=${logDay(0)}.jsonl`]) {
      const reply = await get(route);
      assert.equal(reply.status, 400, route);
      assert.equal(reply.json.ok, false, route);
    }
    for (const day of [logDay(8), logDay(30), "2099-01-01"]) assert.equal((await get(`/api/log?day=${day}`)).status, 404, day);
    assert.equal((await get(`/api/log?day=${logDay(0)}`)).status, 200);
  } finally {
    await c.close();
  }
});

test("the log routes need the session cookie and refuse other origins and POSTs like every other route", async () => {
  const { c, tab, get } = await logCase({ [logDay(0)]: [notepadCall("2026-10-21T08:00:00.000Z")] });
  try {
    for (const route of ["/api/log/days", `/api/log?day=${logDay(0)}`]) {
      assert.equal((await get(route, {})).status, 403, `${route} without a cookie`);
      assert.equal((await get(route, { cookie: "deskpass_session=made-up" })).status, 403, `${route} with a made-up cookie`);
      assert.equal((await get(route, { cookie: tab.cookie, "sec-fetch-site": "cross-site" })).status, 403, `${route} from another site`);
      assert.equal((await send(tab.port, route, { headers: { cookie: tab.cookie, host: "localhost" } })).status, 403, `${route} with another Host`);
      assert.equal((await postJson(tab, route, {})).status, 404, `${route} is not a POST route`);
    }
  } finally {
    await c.close();
  }
});

test("the page has a 日誌 tab next to App, and its script reads the log routes", async () => {
  const { c, tab } = await logCase({});
  try {
    const page = await send(tab.port, "/", { headers: { cookie: tab.cookie } });
    assert.match(page.body, /日誌/);
    assert.match(page.body, /AI 做了什麼/);
    const script = await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } });
    assert.ok(script.body.includes("/api/log/days") && script.body.includes("/api/log?day="));
  } finally {
    await c.close();
  }
});

// Ticket 115: Restore default loosens, so the page only asks for it: POST /api/restore ({ key }) writes
// nothing, starts the native confirm window (`confirm_change`, on a helper of its own) and answers
// 202 with the confirm's id; GET /api/restore?id= is pending until the window is answered, then
// allowed, cancelled, timeout, changed or failed. Only `allow` removes the raise.

type ConfirmStep = { delayMs?: number; result?: unknown; error?: { code: string; message: string }; exit?: number };
const answer = (value: string, delayMs?: number): ConfirmStep => ({ ...(delayMs === undefined ? {} : { delayMs }), result: { answer: value } });

async function restoreCase(steps: ConfirmStep[], { idleMs, menu, lang }: { idleMs?: number; menu?: unknown; lang?: UiLanguage | null } = {}): Promise<{ c: Case; tab: Tab }> {
  const c = await makeCase({ handlers: { confirm_change: steps, ...(menu === undefined ? {} : { list_start_menu_apps: [{ result: menu }] }) } });
  await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
  await writeFile(join(c.dir, "grants.json"), JSON.stringify(grantFile()));
  await writeLogs(c.dir);
  await c.launch({ ...(idleMs === undefined ? {} : { idleMs }), ...(lang === undefined ? {} : { lang }) });
  return { c, tab: await tabOf(c) };
}

async function confirmOf(tab: Tab, id: string): Promise<Reply & { json: Record<string, unknown> | null }> {
  const reply = await send(tab.port, `/api/restore?id=${encodeURIComponent(id)}`, { headers: { cookie: tab.cookie } });
  let json: Record<string, unknown> | null = null;
  try {
    json = JSON.parse(reply.body) as Record<string, unknown>;
  } catch {
    json = null;
  }
  return { ...reply, json };
}

/** Asks GET /api/restore?id= until the confirm is no longer pending. */
async function settled(tab: Tab, id: string, limitMs = 10_000): Promise<Record<string, unknown>> {
  const until = Date.now() + limitMs;
  for (;;) {
    const reply = await confirmOf(tab, id);
    assert.equal(reply.status, 200, reply.body);
    if (reply.json?.state !== "pending") return reply.json ?? {};
    if (Date.now() > until) assert.fail("the confirm stayed pending");
    await delay(50);
  }
}

/** The params of every `confirm_change` the fake helpers were sent. */
async function confirmRequests(dir: string): Promise<Record<string, unknown>[]> {
  return (await readFile(join(dir, "requests.log"), "utf8"))
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as { method: string; params: Record<string, unknown> })
    .filter((request) => request.method === "confirm_change")
    .map((request) => request.params);
}

test("ticket 115: 恢復預設 opens the confirm window and writes nothing until it answers allow; the write verifies in a fresh store and is logged", async () => {
  const { c, tab } = await restoreCase([answer("allow", 800)]);
  try {
    const page = await send(tab.port, "/", { headers: { cookie: tab.cookie } });
    assert.ok(page.body.includes("恢復預設…"), "the page has the button");
    assert.ok(page.body.includes("請在確認視窗按允許"));
    assert.ok((await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } })).body.includes("/api/restore"));

    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    assert.equal(reply.json?.ok, true);
    assert.equal(reply.json?.pending, true);
    assert.match(String(reply.json?.message), /請在確認視窗按允許/);
    const id = String(reply.json?.id);
    assert.match(id, /^[A-Za-z0-9_-]{8,}$/);
    assertNoCors(reply, "the restore");

    // Nothing is written while the window is open.
    assert.equal((await confirmOf(tab, id)).json?.state, "pending");
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "nothing written before the answer");
    assert.deepEqual(await settingsEvents(c.dir), []);
    // A second loosening while one is open is refused, whichever app it names.
    for (const key of [NOTES_KEY, "package:aaa.zed_x1"]) {
      const second = await postJson(tab, "/api/restore", { key });
      assert.equal(second.status, 409, second.body);
      assert.equal(second.json?.reason, "confirm_busy");
      assert.ok(String(second.json?.message).length > 0);
    }

    const result = await settled(tab, id);
    assert.equal(result.state, "allowed");
    assert.match(String(result.message), /已恢復預設/);
    assert.match(String(result.message), /Notes/);

    const written = await readGrants(c.dir);
    assert.equal(written.generation, 4);
    assert.deepEqual(written.raised.map((entry) => entry.key).sort(), ["package:aaa.zed_x1", "package:contoso.sap_abc123"]);
    assert.deepEqual(written.grants.map((entry) => entry.key), [DEMO_KEY, NOTES_KEY], "grants are left alone");
    const fresh = mcpStore(c.dir);
    const view = await fresh.store.current(fresh.mac);
    assert.deepEqual(view.verdict(), { state: "valid", ignored: 0 });
    assert.equal((await view.tierFor(NOTES_KEY, { tier: "normal", why: null })).tier, "normal");
    assert.deepEqual((await settingsEvents(c.dir)).map(untimed), [{ session: "settings", event: "tier_raise_removed", key: NOTES_KEY, path: "C:\\Apps\\Notes.exe" }]);

    // One confirm request, with the window's texts and its timeout.
    assert.deepEqual(await confirmRequests(c.dir), [{ app_name: "Notes", current_state: "高風險（你設定）", restored_state: "一般：每次詢問", timeout_ms: 60_000 }]);
    const row = (await appsOf(c)).apps.find((app) => app.key === NOTES_KEY);
    assert.deepEqual([row?.tier, row?.source], ["normal", "default"]);
    // Now there is no raise left to restore.
    const again = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(again.status, 409, again.body);
    assert.equal(again.json?.reason, "no_raise");
  } finally {
    await c.close();
  }
});

test("ticket 115: cancel, timeout, a stop (interrupted), a helper error or any other answer writes nothing and says so", async () => {
  const { c, tab } = await restoreCase([
    answer("cancel"),
    answer("timeout"),
    { error: { code: "interrupted", message: "The user pressed Esc and stopped Deskpass." } },
    { error: { code: "prompt_unavailable", message: "The screen is locked, so the user cannot see a dialog." } },
    answer("allow_always"),
  ]);
  try {
    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const outcomes: [unknown, unknown][] = [];
    for (let index = 0; index < 5; index += 1) {
      const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
      assert.equal(reply.status, 202, reply.body);
      const result = await settled(tab, String(reply.json?.id));
      outcomes.push([result.state, result.message]);
    }
    assert.deepEqual(outcomes.map(([state]) => state), ["cancelled", "timeout", "cancelled", "failed", "failed"]);
    assert.match(String(outcomes[0]![1]), /已取消，沒有任何改變/);
    assert.match(String(outcomes[1]![1]), /逾時，沒有任何改變/);
    assert.match(String(outcomes[3]![1]), /沒有任何改變/);
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "grants.json is untouched");
    assert.deepEqual(await settingsEvents(c.dir), [], "nothing logged");
    assert.equal((await confirmRequests(c.dir)).length, 5);
  } finally {
    await c.close();
  }
});

test("ticket 115: an app without a raise of the user's, an unknown app or a malformed request is refused without a confirm window", async () => {
  const { c, tab } = await restoreCase([answer("allow")]);
  try {
    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const refused: [unknown, string][] = [
      [{ key: DEMO_KEY }, "no_raise"],
      [{ key: `signer:${MICROSOFT}|msedge.exe` }, "no_raise"],
      [{ key: "package:microsoft.windowsterminal_8wekyb3d8bbwe" }, "no_raise"],
      [{ key: "signer:CN=Nobody|nobody.exe" }, "unknown_app"],
    ];
    for (const [body, reason] of refused) {
      const reply = await postJson(tab, "/api/restore", body);
      assert.equal(reply.status, 409, `${JSON.stringify(body)}: ${reply.body}`);
      assert.equal(reply.json?.reason, reason, JSON.stringify(body));
      assert.ok(String(reply.json?.message).length > 0, "said why");
    }
    for (const body of [{ key: NOTES_KEY, tier: "normal" }, {}, { key: 7 }, "not json"]) {
      assert.equal((await postJson(tab, "/api/restore", body)).status, 400, JSON.stringify(body));
    }
    assert.equal((await postJson(tab, "/api/restore", { key: NOTES_KEY }, { origin: "http://evil.example" })).status, 403);
    assert.equal((await confirmOf(tab, "made-up-id-0000")).status, 404);
    assert.deepEqual(await confirmRequests(c.dir), [], "no window was asked for");
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before);
  } finally {
    await c.close();
  }
});

test("ticket 115: while the confirm window is open the idle server does not stop; it writes the allowed change, then stops", async () => {
  const { c, tab } = await restoreCase([answer("allow", 1_500)], { idleMs: 200 });
  try {
    // The server's helper is started first, under a hold of the test's own: a request in flight is not
    // activity, and that start can take longer than 200 ms.
    const server = servers[0]!;
    const warm = server.hold();
    await appsOf(c);
    warm();
    let stopped = false;
    void server.closed.then(() => {
      stopped = true;
    });
    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    await delay(1_000);
    assert.equal(stopped, false, "held while the window is open");
    await Promise.race([server.closed, delay(10_000).then(() => assert.fail("the server did not stop"))]);
    assert.ok(!(await readGrants(c.dir)).raised.some((entry) => entry.key === NOTES_KEY), "the allowed change was written");
    assert.deepEqual((await settingsEvents(c.dir)).map((event) => event.event), ["tier_raise_removed"]);
  } finally {
    await c.close();
  }
});

test("ticket 115: a raise changed while the window was open is not removed on allow, and the page is told", async () => {
  const { c, tab } = await restoreCase([answer("allow", 800)]);
  try {
    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    // Tightening is not a loosening: it still goes through, and the user did not confirm removing it.
    const raised = await postJson(tab, "/api/raise", { key: NOTES_KEY, tier: "blocked" });
    assert.equal(raised.status, 200, raised.body);
    const result = await settled(tab, String(reply.json?.id));
    assert.equal(result.state, "changed");
    assert.match(String(result.message), /Notes/);
    assert.match(String(result.message), /沒有任何改變/);
    assert.equal((await readGrants(c.dir)).raised.find((entry) => entry.key === NOTES_KEY)?.tier, "blocked");
    assert.deepEqual((await settingsEvents(c.dir)).map((event) => event.event), ["tier_raised"]);
  } finally {
    await c.close();
  }
});

test("ticket 115: the confirm window's texts are sent without control or format characters, and capped", async () => {
  // Not U+2028 here: the fake helper's JSON line would carry it raw, and the client's line reader splits there.
  const name = `Zed\nApp\r\t\u0007\u202e\u0085X」從『Chrome${"長".repeat(300)}`;
  const menu = { apps: [{ app_id: "uwp:aaa.zed_x1!App", name, key: "package:aaa.zed_x1" }] };
  const { c, tab } = await restoreCase([answer("cancel")], { menu });
  try {
    let listed = (await appsOf(c)) as Awaited<ReturnType<typeof appsOf>> & { menu?: string };
    for (let attempt = 0; attempt < 50 && listed.menu !== "ready"; attempt += 1) {
      await delay(100);
      listed = await appsOf(c);
    }
    assert.equal(listed.menu, "ready");
    const reply = await postJson(tab, "/api/restore", { key: "package:aaa.zed_x1" });
    assert.equal(reply.status, 202, reply.body);
    assert.equal((await settled(tab, String(reply.json?.id))).state, "cancelled");
    const [params] = await confirmRequests(c.dir);
    const sent = String(params?.app_name);
    assert.doesNotMatch(sent, /[\p{Cc}\p{Cf}\u2028\u2029]/u);
    // A corner bracket would close the window's corner-bracket quote around the name and let the name pose as another sentence.
    assert.doesNotMatch(sent, /[「」『』]/u);
    assert.ok(sent.startsWith("Zed App X)從(Chrome"), sent);
    assert.ok([...sent].length <= 80, `${[...sent].length} characters`);
    assert.equal(params?.current_state, "高風險（你設定）");
    assert.equal(params?.restored_state, "一般：每次詢問");
  } finally {
    await c.close();
  }
});

test("ticket 115: a confirm the user takes longer than a helper request's usual 10 seconds to answer still counts", async () => {
  const { c, tab } = await restoreCase([answer("allow", 10_500)]);
  try {
    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    assert.equal((await settled(tab, String(reply.json?.id), 20_000)).state, "allowed");
    // Meanwhile the page's own helper kept answering: the list still verifies.
    assert.equal((await appsOf(c)).grants.state, "valid");
  } finally {
    await c.close();
  }
});

test("ticket 122: against a protocol-8 helper (before confirm_change) the grants cannot be verified, 恢復預設 answers 503 unavailable and no confirm window is asked for", async () => {
  const c = await makeCase({ version: "8", handlers: { confirm_change: [answer("allow")] } });
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    const before = JSON.stringify(grantFile());
    await writeFile(join(c.dir, "grants.json"), before);
    await writeLogs(c.dir);
    await c.launch();
    const tab = await tabOf(c);

    const listed = await appsOf(c);
    assert.equal(listed.grants.state, "unverified", "no MAC could be made, so nothing counts as verified");
    // The app of a raise only grants.json knows is not listed then; one the log names still is.
    const rowOf = (apps: Record<string, unknown>[], key: string) => apps.find((app) => app.key === key);
    assert.equal(rowOf(listed.apps, NOTES_KEY), undefined);
    assert.notEqual(rowOf(listed.apps, DEMO_KEY), undefined);
    assert.equal((await postJson(tab, "/api/restore", { key: NOTES_KEY })).status, 409, "an app not listed is unknown");

    const reply = await postJson(tab, "/api/restore", { key: DEMO_KEY });
    assert.equal(reply.status, 503, reply.body);
    assert.deepEqual([reply.json?.ok, reply.json?.reason], [false, "unavailable"]);
    assert.match(String(reply.json?.message), /現在無法寫入授權檔/);

    const asked = (await readFile(join(c.dir, "requests.log"), "utf8"))
      .split("\n")
      .filter(Boolean)
      .map((line) => (JSON.parse(line) as { method: string }).method);
    assert.ok(asked.includes("ping"), "the helper was reached, and refused at its version");
    assert.equal(asked.includes("confirm_change"), false, asked.join(", "));
    assert.equal(asked.includes("grant_mac"), false, "nothing is signed by an old helper");
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "grants.json is not touched");
  } finally {
    await c.close();
  }
});

// Ticket 123: the confirm's lifecycle (ticket 115's design): a server closed while the window is open
// ends the confirm's helper and writes nothing; a confirm helper that ends before the answer settles
// the confirm as failed, and Restore default can be asked again.

async function untilConfirmAsked(dir: string, count = 1): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      if ((await confirmRequests(dir)).length >= count) return;
    } catch {
      // No request logged yet.
    }
    await delay(50);
  }
  assert.fail("the confirm window was not asked for");
}

test("ticket 123: closing the settings server while the confirm window is open ends the confirm's helper and leaves grants.json as it was", async () => {
  const { c, tab } = await restoreCase([answer("allow", 2_000)]);
  try {
    const server = servers[0]!;
    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    await untilConfirmAsked(c.dir);
    await server.close();
    assert.ok(c.children.length >= 2, "the server's helper and the confirm's were started");
    for (const child of c.children) assert.ok(child.exitCode !== null || child.signalCode !== null, `helper ${child.pid} ended with the server`);
    // Past the time the window would have answered allow: nothing came of it.
    await delay(2_500);
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "grants.json is untouched");
    assert.deepEqual(await settingsEvents(c.dir), [], "nothing logged");
  } finally {
    await c.close();
  }
});

test("ticket 123: a confirm helper that ends before the user answers settles the confirm as failed, and 恢復預設 can be confirmed again", async () => {
  const { c, tab } = await restoreCase([{ delayMs: 300, exit: 1 }, answer("allow")]);
  try {
    const before = await readFile(join(c.dir, "grants.json"), "utf8");
    const first = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(first.status, 202, first.body);
    const failed = await settled(tab, String(first.json?.id));
    assert.equal(failed.state, "failed");
    assert.match(String(failed.message), /沒有任何改變/);
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before, "nothing written");

    const again = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(again.status, 202, again.body);
    assert.equal((await settled(tab, String(again.json?.id))).state, "allowed");
    assert.ok(!(await readGrants(c.dir)).raised.some((entry) => entry.key === NOTES_KEY), "the second confirm's allow was written");
    assert.equal((await confirmRequests(c.dir)).length, 2);
  } finally {
    await c.close();
  }
});

test("ticket 124: a confirm id another client's newer confirm replaced answers superseded, not a failure; an id never given stays 404", async () => {
  const { c, tab } = await restoreCase([answer("cancel"), answer("cancel", 1_500)]);
  try {
    const first = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(first.status, 202, first.body);
    const oldId = String(first.json?.id);
    assert.equal((await settled(tab, oldId)).state, "cancelled");
    // Another client opens a new confirm before this page asked about its own again.
    const second = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(second.status, 202, second.body);
    for (const phase of ["while the newer one is pending", "after it settled"]) {
      const stale = await confirmOf(tab, oldId);
      assert.equal(stale.status, 200, `${phase}: ${stale.body}`);
      assert.deepEqual([stale.json?.id, stale.json?.state], [oldId, "superseded"], phase);
      if (phase.startsWith("while")) assert.equal((await settled(tab, String(second.json?.id))).state, "cancelled");
    }
    assert.equal((await confirmOf(tab, "made-up-id-0000")).status, 404);
    // The page reloads the App list for a superseded confirm instead of showing a failure.
    const script = (await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } })).body;
    assert.ok(script.includes('"superseded"'), "the script knows the superseded state");
  } finally {
    await c.close();
  }
});

test("ticket 126: a page reloaded while the confirm window is open finds the same confirm in /api/apps and waits for it; no second window is asked for", async () => {
  const { c, tab } = await restoreCase([answer("allow", 1_500)]);
  try {
    assert.equal(((await appsOf(c)) as { confirm?: unknown }).confirm, null, "no confirm before one is asked for");
    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    const id = String(reply.json?.id);
    // The reloaded page has only its cookie: the list tells it which confirm is open.
    const reloaded = (await appsOf(c)) as { confirm?: Record<string, unknown> | null };
    assert.deepEqual(reloaded.confirm, { id, key: NOTES_KEY, state: "pending", message: SETTINGS_TEXT.change.restore.pending });
    assert.equal((await settled(tab, id)).state, "allowed");
    assert.equal(((await appsOf(c)) as { confirm?: unknown }).confirm, null, "a settled confirm is not offered to wait for");
    assert.equal((await confirmRequests(c.dir)).length, 1, "one confirm window");
    const script = (await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } })).body;
    assert.ok(script.includes("data.confirm"), "the script picks up the open confirm from the list");
  } finally {
    await c.close();
  }
});

test("ticket 125: a raise equal to the built-in tier is never written, and one already in grants.json is shown and cleared by 恢復預設…", async () => {
  const c = await makeCase({ handlers: { confirm_change: [answer("allow")] } });
  try {
    await writeFile(join(c.dir, "grants.key"), FAKE_GRANT_KEY);
    const file = grantFile();
    const withEqual = sealed(file.generation, file.grants, [...file.raised, sign({ key: EDGE_KEY, tier: "high", created_at: "2026-09-06T00:00:00.000Z" })]);
    await writeFile(join(c.dir, "grants.json"), JSON.stringify(withEqual));
    await writeLogs(c.dir);
    await c.launch();
    const tab = await tabOf(c);
    const before = await readFile(join(c.dir, "grants.json"), "utf8");

    // The page never writes one: "set to High risk" on a built-in high app is refused.
    const raised = await postJson(tab, "/api/raise", { key: EDGE_KEY, tier: "high" });
    assert.equal(raised.status, 409, raised.body);
    assert.equal(raised.json?.reason, "built_in");
    assert.equal(await readFile(join(c.dir, "grants.json"), "utf8"), before);

    // The one in the file: still built-in high, said to have no effect, and restorable.
    const rowOf = async () => (await appsOf(c)).apps.find((app) => app.key === EDGE_KEY);
    const row = await rowOf();
    assert.deepEqual([row?.tier, row?.source, row?.raised], ["high", "builtIn", "high"]);
    assert.match(String(row?.why), /恢復預設/);
    const others = (await appsOf(c)).apps;
    assert.equal(others.find((app) => app.key === DEMO_KEY)?.raised, null, "an app without a raise");
    assert.equal(others.find((app) => app.key === NOTES_KEY)?.raised, "high", "a user raise above the built-in tier is one too");

    const reply = await postJson(tab, "/api/restore", { key: EDGE_KEY });
    assert.equal(reply.status, 202, reply.body);
    assert.equal((await settled(tab, String(reply.json?.id))).state, "allowed");
    assert.deepEqual(await confirmRequests(c.dir), [{ app_name: "msedge.exe", current_state: "高風險（你設定）", restored_state: "高風險（內建）", timeout_ms: 60_000 }]);
    assert.ok(!(await readGrants(c.dir)).raised.some((entry) => entry.key === EDGE_KEY), "the equal raise is gone");
    assert.deepEqual((await settingsEvents(c.dir)).map((event) => [event.event, event.key]), [["tier_raise_removed", EDGE_KEY]]);
    const after = await rowOf();
    assert.deepEqual([after?.tier, after?.source, after?.raised], ["high", "builtIn", null]);
    assert.doesNotMatch(String(after?.why), /恢復預設/);
    assert.equal((await postJson(tab, "/api/restore", { key: EDGE_KEY })).json?.reason, "no_raise");
  } finally {
    await c.close();
  }
});

// Ticket 146: the "Allow background operation" switch. GET /api/settings gives it; POST /api/settings
// ({ allow_background: boolean }) sets it in settings.json, behind the same checks as every change.
// It is on by default. The MCP server reads the file at each tool call (test/background-switch.test.ts).

const settingsOf = async (tab: Tab) => {
  const reply = await send(tab.port, "/api/settings", { headers: { cookie: tab.cookie } });
  return { ...reply, json: JSON.parse(reply.body) as Record<string, unknown> };
};

test("ticket 146: the page has the 允許背景操作 switch, on by default, with no settings file", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const tab = await tabOf(c);
    const page = await send(tab.port, "/", { headers: { cookie: tab.cookie } });
    assert.ok(page.body.includes("允許背景操作"), "the label");
    assert.match(page.body, /id="allow-background"/);
    const script = await send(tab.port, "/app.js", { headers: { cookie: tab.cookie } });
    assert.ok(script.body.includes("/api/settings"));

    const settings = await settingsOf(tab);
    assert.equal(settings.status, 200);
    assert.deepEqual(settings.json, { allowBackground: true });
    assertNoCors(settings, "the settings");
    await assert.rejects(readFile(join(c.dir, "settings.json")), { code: "ENOENT" }, "reading writes nothing");
  } finally {
    await c.close();
  }
});

test("ticket 146: the switch is on for a settings file without it; the page turns it off and on, and the file is what the MCP server reads", async () => {
  const c = await makeCase();
  try {
    await writeFile(join(c.dir, "settings.json"), JSON.stringify({ other: 1 }));
    await c.launch();
    const tab = await tabOf(c);
    assert.deepEqual((await settingsOf(tab)).json, { allowBackground: true });
    const revisionOf = async () => JSON.parse((await send(tab.port, "/api/status", { headers: { cookie: tab.cookie } })).body).revision as string;
    const first = await revisionOf();

    const off = await postJson(tab, "/api/settings", { allow_background: false });
    assert.equal(off.status, 200, off.body);
    assert.equal(off.json?.ok, true);
    assert.equal(off.json?.allowBackground, false);
    assert.match(String(off.json?.message), /關閉/);
    assertNoCors(off, "turning it off");
    assert.deepEqual(JSON.parse(await readFile(join(c.dir, "settings.json"), "utf8")), { other: 1, allow_background: false });
    assert.equal(await readAllowBackground(c.dir), false, "the MCP server's reading");
    assert.deepEqual((await settingsOf(tab)).json, { allowBackground: false });
    assert.notEqual(await revisionOf(), first, "the page's poll sees the change");

    const on = await postJson(tab, "/api/settings", { allow_background: true });
    assert.equal(on.status, 200, on.body);
    assert.equal(on.json?.allowBackground, true);
    assert.match(String(on.json?.message), /開啟/);
    assert.equal(await readAllowBackground(c.dir), true);
    assert.deepEqual((await settingsOf(tab)).json, { allowBackground: true });
  } finally {
    await c.close();
  }
});

test("ticket 146: a request that is not exactly { allow_background: boolean } is 400, and a write needs the page's own Origin and cookie", async () => {
  const c = await makeCase();
  try {
    await c.launch();
    const tab = await tabOf(c);
    for (const body of [{}, { allow_background: "false" }, { allow_background: 0 }, { allow_background: null }, { allow_background: false, extra: 1 }, { other: false }, [false], "{ not json"]) {
      const reply = await postJson(tab, "/api/settings", body);
      assert.equal(reply.status, 400, JSON.stringify(body));
      assert.equal(reply.json?.reason, "bad_request");
    }
    assert.equal((await postJson(tab, "/api/settings", { allow_background: false }, { "content-type": "text/plain" })).status, 400);
    const refused = [
      await postJson(tab, "/api/settings", { allow_background: false }, { origin: "" }),
      await postJson(tab, "/api/settings", { allow_background: false }, { origin: "http://evil.example" }),
      await postJson(tab, "/api/settings", { allow_background: false }, { cookie: "" }),
      await postJson(tab, "/api/settings", { allow_background: false }, { "sec-fetch-site": "cross-site" }),
    ];
    for (const [index, reply] of refused.entries()) assert.equal(reply.status, 403, `request ${index}`);
    assert.equal((await send(tab.port, "/api/settings", { headers: {} })).status, 403, "reading needs the cookie");
    await assert.rejects(readFile(join(c.dir, "settings.json")), { code: "ENOENT" }, "nothing was written");
  } finally {
    await c.close();
  }
});

// Ticket 153 item 2: the MCP server keeps the last switch it read while settings.json cannot be read,
// which the settings page cannot know, so the page says the file cannot be read instead of showing a state.
test("ticket 153: a settings.json that cannot be read is not shown as on: the settings answer says so, and a write repairs it", async () => {
  const c = await makeCase();
  try {
    await writeFile(join(c.dir, "settings.json"), "{ not json");
    await c.launch();
    const tab = await tabOf(c);
    const unreadable = await settingsOf(tab);
    assert.equal(unreadable.status, 200);
    assert.deepEqual(unreadable.json, { allowBackground: null, unreadable: true });

    const on = await postJson(tab, "/api/settings", { allow_background: true });
    assert.equal(on.status, 200, on.body);
    assert.deepEqual((await settingsOf(tab)).json, { allowBackground: true });
  } finally {
    await c.close();
  }
});

// Open-source ticket 01: with DESKPASS_LANG=en the App page is English (the settings server decides
// its language once, at start). App names and paths are not translated. Ticket 02 made the log tab English too.

/** `body` split into its HTML and the page's text block (the JSON the script reads). */
function appPageParts(body: string): { html: string; text: Record<string, unknown> } {
  const block = /<script type="application\/json" id="deskpass-text">([\s\S]*?)<\/script>/.exec(body);
  assert.ok(block, "the page has its text block");
  const text = JSON.parse(block[1] ?? "{}") as Record<string, unknown>;
  return { html: body.replace(block[0], ""), text };
}

test("open-source 01: with DESKPASS_LANG=en the App page, its App list and its loosening confirm are in English", async () => {
  const saved = process.env.DESKPASS_LANG;
  process.env.DESKPASS_LANG = "en";
  let opened: { c: Case; tab: Tab } | null = null;
  try {
    opened = await restoreCase([answer("allow", 300)], { lang: null });
    const { c, tab } = opened;
    const E = SETTINGS_TEXT_EN;
    const page = await send(tab.port, "/", { headers: { cookie: tab.cookie } });
    assert.equal(page.status, 200);
    const { html, text } = appPageParts(page.body);
    assert.match(html, /<html lang="en">/);
    assert.ok(html.includes("<title>Deskpass Settings · Apps</title>"));
    for (const words of [E.product, E.page.heading, E.page.nav.apps, E.page.nav.log, E.page.background.label, E.page.search.placeholder, ...E.page.states.map((state) => state.name)]) {
      assert.ok(html.includes(words), `the page has ${words}`);
    }
    assert.deepEqual(Object.values((text.filters ?? {}) as Record<string, string>), ["Used by the AI", "Set by you", "Blocked", "High risk"]);
    assert.equal((text.actions as Record<string, string>).restoreHint, E.page.actions.restoreHint);
    assert.doesNotMatch(html, CJK, "no CJK in the App page's HTML");
    assert.doesNotMatch(JSON.stringify(text), CJK, "no CJK in the App page's texts");

    const { grants, apps } = await appsOf(c);
    assert.deepEqual(grants, { state: "valid", message: null });
    const byKey = new Map(apps.map((app) => [app.key, app]));
    assert.equal(byKey.get(EDGE_KEY)?.why, "Web browsers: they can sign in to your accounts and submit forms, so it is built in as High risk.");
    assert.deepEqual(byKey.get(EDGE_KEY)?.builtIn, { tier: "high", label: "Web browsers" });
    assert.equal(byKey.get("package:microsoft.windowsterminal_8wekyb3d8bbwe")?.why, "Terminals and command lines: they can run any program, so it is built in as Blocked.");
    assert.equal(byKey.get(NOTES_KEY)?.why, "You set it to High risk (built in as Normal).");
    assert.equal(byKey.get(DEMO_KEY)?.publisher, "Contoso Ltd · signed");
    // App names, paths and keys are the apps' own; everything else the list says is English.
    const said = apps.map(({ name: _name, path: _path, key: _key, ...rest }) => rest);
    assert.doesNotMatch(JSON.stringify(said), CJK, "no CJK in the App list's sentences");

    const refused = await send(tab.port, "/");
    assert.equal(refused.status, 403);
    assert.equal(refused.body, E.forbidden);

    const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
    assert.equal(reply.status, 202, reply.body);
    assert.equal(reply.json?.message, E.change.restore.pending);
    const result = await settled(tab, String(reply.json?.id));
    assert.equal(result.message, 'Default restored: Notes is now "Normal: ask every time".');
    assert.deepEqual(await confirmRequests(c.dir), [{ app_name: "Notes", current_state: "High risk (set by you)", restored_state: "Normal: ask every time", timeout_ms: 60_000 }]);
  } finally {
    if (saved === undefined) delete process.env.DESKPASS_LANG;
    else process.env.DESKPASS_LANG = saved;
    await opened?.c.close();
  }
});

// Open-source ticket 03: the helpers of the settings server (the confirm window's, the Start Menu's) are
// started in the page's language, so the confirm window and the page are always in the same one.
test("open-source 03: every helper the settings server starts is given the page's language", async () => {
  for (const lang of ["en", "zh-TW"] as UiLanguage[]) {
    const { c, tab } = await restoreCase([answer("allow")], { lang });
    try {
      const reply = await postJson(tab, "/api/restore", { key: NOTES_KEY });
      assert.equal(reply.status, 202, reply.body);
      await settled(tab, String(reply.json?.id));
      assert.ok(c.children.length > 0, "the server started a helper");
      for (const child of c.children) {
        const at = child.spawnargs.indexOf("--lang");
        assert.equal(child.spawnargs[at + 1], lang, child.spawnargs.join(" "));
      }
    } finally {
      await c.close();
    }
  }
});

// Open-source ticket 02: the log tab in English. Every kind of event the log tab knows is written
// to a day file; with DESKPASS_LANG=en what the page's log routes say has no CJK (app names and
// paths are the apps' own), and the zh-TW sentences stay as they were (the tests above).

test("open-source 02: with DESKPASS_LANG=en the log tab's page text, tones and every kind of event are in English", async () => {
  const saved = process.env.DESKPASS_LANG;
  process.env.DESKPASS_LANG = "en";
  const edge = { path: EDGE, key: EDGE_KEY, tier: "high" };
  const at = (n: number) => `2026-10-21T08:${String(n % 60).padStart(2, "0")}:00.000Z`;
  const L = SETTINGS_TEXT_EN.logRows;
  const lines = [
    ...Object.keys(L.tools).map((tool, i) => logEvent(at(i), { event: "tool_call", tool, app: edge, action_summary: "open the file", result: "ok", delivery: i % 2 === 0 ? "background" : "foreground" })),
    logEvent(at(1), { event: "tool_call", app: edge, result: "ok" }),
    logEvent(at(2), { event: "tool_call", tool: "computer_made_up", app: edge, result: "made_up_result" }),
    ...Object.keys(L.results).map((result, i) => logEvent(at(i), { event: "tool_call", tool: "computer_click", app: edge, action_summary: null, result })),
    ...["dialog", "elicitation", "grant", "session", "other"].flatMap((channel) => Object.keys(L.approval.answers).map((answer, i) => logEvent(at(i), { event: "approval", channel, answer, app: edge }))),
    ...["grant", "raised", "file", "other"].flatMap((entry) => Object.keys(L.grantInvalid.reasons).map((reason, i) => logEvent(at(i), { event: "grant_invalid", entry, reason, app: edge }))),
    logEvent(at(3), { event: "grant_key_lost", entries: 3 }),
    logEvent(at(3), { event: "grant_key_lost", entries: 1 }),
    logEvent(at(4), { event: "grant_mismatch", app: edge }),
    logEvent(at(5), { event: "grant_revoked", app: edge }),
    ...["normal", "high", "blocked"].map((tier) => logEvent(at(6), { event: "tier_raised", tier, grant_removed: tier === "blocked", app: edge })),
    logEvent(at(7), { event: "tier_raise_removed", app: edge }),
    logEvent(at(8), { event: "clipboard_read", subject: "clipboard", chars: 12, truncated: true }),
    logEvent(at(9), { event: "clipboard_read", subject: "clipboard", chars: 5 }),
    logEvent(at(9), { event: "clipboard_read", subject: "clipboard", chars: 1 }),
    ...["settings", "panel"].flatMap((session) => [
      JSON.stringify({ time: at(10), session, event: "tier_raised", tier: "high", grant_removed: true, app: edge }),
      JSON.stringify({ time: at(11), session, event: "tier_raise_removed", app: edge }),
      JSON.stringify({ time: at(12), session, event: "grant_revoked", app: edge }),
    ]),
    logEvent("not a time", { event: "tool_call", tool: "computer_click", app: edge, result: "ok" }),
    logEvent(at(13), { event: "approval", channel: "dialog", answer: "allow_once" }),
  ];
  let opened: Awaited<ReturnType<typeof logCase>> | null = null;
  try {
    opened = await logCase({ [logDay(0)]: lines }, null);
    const { tab, get } = opened;
    const E = SETTINGS_TEXT_EN;

    // The page: the log view's HTML and the script's text block (with the tags' tones) have no CJK.
    const page = await send(tab.port, "/", { headers: { cookie: tab.cookie } });
    const { html, text } = appPageParts(page.body);
    assert.ok(html.includes(E.log.heading) && html.includes(E.page.nav.log), "the log view is English");
    assert.doesNotMatch(html, CJK);
    assert.deepEqual(text.log, E.log);
    assert.doesNotMatch(JSON.stringify(text), CJK);
    const tones = text.tones as { ok: string[]; ask: string[]; bad: string[] };
    assert.deepEqual(tones.ok, [L.results.ok]);
    assert.ok(tones.ask.includes(L.approval.answers.allow_once!) && tones.ask.includes(L.approval.grantAllowAlways) && tones.ask.includes(L.approval.sessionAllowOnce));
    assert.ok(tones.bad.includes(L.results.blocked_app!) && tones.bad.includes(L.approval.answers.refuse!) && tones.bad.includes(L.mismatch.result) && tones.bad.includes(L.grantInvalid.reasons.bad_mac!));
    for (const tag of tones.ok) assert.ok(!tones.ask.includes(tag) && !tones.bad.includes(tag), `${tag} has one tone`);
    for (const tag of tones.ask) assert.ok(!tones.bad.includes(tag), `${tag} has one tone`);

    // The retention line.
    const days = await get("/api/log/days");
    assert.match(days.json.retention, /^Records are kept for \d+ days \(/);
    assert.doesNotMatch(days.body, CJK);

    // The rows: no CJK anywhere in what the day says.
    const day = await get(`/api/log?day=${logDay(0)}`);
    assert.equal(day.status, 200);
    assert.doesNotMatch(day.body, CJK, "no CJK in the log day");
    const rows: { time: string; sentence: string; tag?: string; mark?: string }[] = day.json.groups.flatMap((g: { rows: unknown[] }) => g.rows);
    const sentences = new Set(rows.map((row) => row.sentence));
    for (const name of Object.values(L.tools)) assert.ok([...sentences].some((s) => s.startsWith(`${name} · ${L.summary}open the file`)), name);
    for (const sentence of [L.toolUnnamed, "computer_made_up", L.approval.dialog, L.approval.elicitation, L.approval.other, ...Object.values(L.grantInvalid.what), L.keyLost.what, L.mismatch.what, L.grantRevoked, L.tierRaised.what, L.tierRaiseRemoved.what, L.clipboardRead.what]) {
      assert.ok(sentences.has(sentence), sentence);
    }
    const tags = new Set(rows.map((row) => row.tag));
    for (const tag of [...Object.values(L.results), ...Object.values(L.approval.answers), L.approval.grantAllowAlways, L.approval.sessionAllowOnce, ...Object.values(L.grantInvalid.reasons), "made_up_result"]) assert.ok(tags.has(tag), tag);
    for (const tag of ["3 entries no longer valid", L.mismatch.result, L.tierRaiseRemoved.result, "Set to Normal", "Set to High risk", "Set to Blocked, and its always allow was removed", "12 chars (truncated)", "5 chars", "1 char", "1 entry no longer valid"]) assert.ok(tags.has(tag), tag);
    assert.deepEqual(new Set(rows.filter((row) => row.mark).map((row) => row.mark)), new Set([L.delivery.background, L.delivery.foreground]));
    // The app is named as the App page names it: by the Start Menu once that is read, else by its file.
    for (const sentence of [
      /^You on the settings page set (Microsoft Edge|msedge.exe) to High risk, and its always allow was removed$/,
      /^You in the control panel set (Microsoft Edge|msedge.exe) to High risk, and its always allow was removed$/,
      /^You on the settings page restored (Microsoft Edge|msedge.exe) to default$/,
      /^You in the control panel removed the always allow of (Microsoft Edge|msedge.exe)$/,
    ]) {
      assert.ok([...sentences].some((s) => sentence.test(s)), String(sentence));
    }
    assert.ok(rows.some((row) => row.time === L.noTime), "a time that is not a time");
    assert.ok(day.json.groups.some((g: { app: string }) => g.app === L.clipboard), "the clipboard's group");
    assert.ok(day.json.groups.some((g: { app: string }) => g.app === L.noApp), "an event about no app");
  } finally {
    if (saved === undefined) delete process.env.DESKPASS_LANG;
    else process.env.DESKPASS_LANG = saved;
    await opened?.c.close();
  }
});
