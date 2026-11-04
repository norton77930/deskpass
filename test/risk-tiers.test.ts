import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { BUILT_IN_TIERS, builtInTierOf, classifyApp } from "../src/policy.ts";
import { SETTINGS_TEXT } from "../src/settings-strings.ts";
import { approvalEvents, connectServer, textContent, type Harness } from "./harness.ts";

// Ticket 91: every app is in one tier: blocked (never operated, refused without asking), high
// (asked, and allowed for this server session only) or normal (as before). The tier is decided by
// the authorization decision and written into the `approval` and `tool_call` log events. Seam one:
// the MCP client, the server and the scripted helper, the log in the case's temp folder.

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

function windowOf(app: string, appName: string, extra: Record<string, unknown> = {}) {
  return { ...demo, app, app_name: appName, ...extra };
}

const CONSENT = "C:\\Windows\\System32\\consent.exe";
const CREDENTIAL_BROKER = "C:\\Windows\\System32\\CredentialUIBroker.exe";
const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";

function typeText(harness: Harness) {
  return harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "x" } });
}

function code(result: { content?: unknown }): string {
  return textContent(result).split(/\r?\n/)[0] ?? "";
}

/** Today's events of `kind` in the case's log. */
async function events(harness: Harness, kind: "approval" | "tool_call"): Promise<Record<string, unknown>[]> {
  if (kind === "approval") return approvalEvents(harness);
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === kind);
}

async function decisions(harness: Harness) {
  return (await events(harness, "approval")).map(({ channel, answer, app }) => ({ channel, answer, tier: (app as { tier?: unknown }).tier }));
}

async function sentMethods(harness: Harness): Promise<string[]> {
  return (await harness.requests()).map((entry) => (entry as { method?: string }).method ?? "");
}

test("ticket 110: the helper is in the blocked tier under its name and under the old install's name", () => {
  for (const app of ["C:\\Deskpass\\dist\\deskpass-helper.exe", "C:\\Old\\dist\\computer-use-helper.exe", "C:\\Old\\dist\\Computer-Use-Helper.EXE"]) {
    assert.deepEqual(classifyApp(app, []), { tier: "blocked", why: "it is part of Deskpass itself" }, app);
  }
});

for (const [app, appName, why] of [
  [CONSENT, "Consent", /UAC/],
  [CREDENTIAL_BROKER, "Windows Security", /credential/i],
] as const) {
  test(`${appName} (${app.split("\\").pop()}) is in the blocked tier: refused without asking, the message says the tier and why`, async () => {
    const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: windowOf(app, appName) }] } } });
    try {
      const result = await typeText(harness);
      assert.equal(result.isError, true);
      assert.equal(code(result), "blocked_app");
      const text = textContent(result);
      assert.match(text, /blocked tier/i);
      assert.match(text, why);
      assert.match(text, /report/i);
      assert.equal(harness.elicitations.length, 0);
      const methods = await sentMethods(harness);
      assert.equal(methods.some((method) => ["ask_user", "type_text"].includes(method)), false, methods.join(", "));
      assert.deepEqual(await decisions(harness), [{ channel: null, answer: "blocked", tier: "blocked" }]);
      const [call] = await events(harness, "tool_call");
      assert.deepEqual(call?.app, { path: app, key: app.toLowerCase(), tier: "blocked" });
    } finally {
      await harness.close();
    }
  });
}

test("consent.exe and CredentialUIBroker.exe cannot be launched: blocked_app before the app is resolved", async () => {
  for (const app of ["consent", "CredentialUIBroker.exe", CONSENT]) {
    let resolved = 0;
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async (name) => {
        resolved += 1;
        return name;
      },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app } });
      assert.equal(code(result), "blocked_app", app);
      assert.match(textContent(result), /blocked tier/i, app);
      assert.equal(resolved, 0, app);
      assert.equal(harness.elicitations.length, 0, app);
      assert.deepEqual(await decisions(harness), [{ channel: null, answer: "blocked", tier: "blocked" }], app);
    } finally {
      await harness.close();
    }
  }
});

test("consent.exe and CredentialUIBroker.exe cannot be launched through an app_id either", async () => {
  const cases: { appId: string; resolved?: string }[] = [
    { appId: `exe:${CONSENT}` },
    { appId: `exe:${CREDENTIAL_BROKER}` },
    // Blocked once the helper says which executable the shortcut runs.
    { appId: "shell:Contoso.Prompt", resolved: CREDENTIAL_BROKER },
  ];
  for (const { appId, resolved } of cases) {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async (app) => app,
      script: { handlers: { resolve_app_id: [{ result: { app: resolved } }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: appId } });
      assert.equal(code(result), "blocked_app", appId);
      assert.match(textContent(result), /blocked tier/i, appId);
      assert.equal(harness.elicitations.length, 0, appId);
      const methods = await sentMethods(harness);
      assert.equal(methods.includes("launch_app"), false, appId);
      if (!resolved) assert.equal(methods.includes("resolve_app_id"), false, appId);
      assert.deepEqual(await decisions(harness), [{ channel: null, answer: "blocked", tier: "blocked" }], appId);
    } finally {
      await harness.close();
    }
  }
});

test("a high-risk app can be launched: the pre-check lets it through, the user is asked, and the approval logs tier high", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async () => EDGE,
    script: { handlers: { launch_app: [{ result: { pid: 50, window: { ...windowOf(EDGE, "Microsoft Edge"), window_id: "9" } } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "msedge" } });
    assert.notEqual(result.isError, true, textContent(result));
    assert.equal(harness.elicitations.length, 1);
    assert.match(harness.elicitations[0] ?? "", /msedge\.exe/i);
    const launch = (await harness.requests()).find((entry) => (entry as { method?: string }).method === "launch_app") as { params?: { app?: string } } | undefined;
    assert.equal(launch?.params?.app, EDGE);
    assert.deepEqual(await decisions(harness), [{ channel: "elicitation", answer: "allow_once", tier: "high" }]);
  } finally {
    await harness.close();
  }
});

test("the blocked tier stands whatever the session remembers: an app allowed or refused earlier is blocked as this server's own process", async () => {
  // The harness runs the server in this process, so a window of this pid is the server's own.
  const own = windowOf(demo.app, demo.app_name, { pid: process.pid });
  for (const onElicit of [() => ({ action: "accept" as const }), () => ({ action: "decline" as const })]) {
    const harness = await connectServer({ elicitation: "form", onElicit, script: { handlers: { get_window: [{ result: demo }, { result: own }] } } });
    try {
      await typeText(harness);
      const result = await typeText(harness);
      assert.equal(code(result), "blocked_app");
      assert.match(textContent(result), /blocked tier/i);
      assert.equal(harness.elicitations.length, 1);
      const [, second] = await decisions(harness);
      assert.deepEqual(second, { channel: null, answer: "blocked", tier: "blocked" });
    } finally {
      await harness.close();
    }
  }
});

test("a high-risk app is asked about; its allow is remembered for this session only, and every decision logs tier high", async () => {
  const edge = windowOf(EDGE, "Microsoft Edge");
  const script = { handlers: { get_window: [{ result: edge }, { result: edge }] } };
  const first = await connectServer({ elicitation: "form", script });
  const second = await connectServer({ elicitation: "form", script });
  try {
    assert.notEqual((await typeText(first)).isError, true);
    assert.notEqual((await typeText(first)).isError, true);
    assert.equal(first.elicitations.length, 1);
    assert.match(first.elicitations[0] ?? "", /for this session/);
    assert.deepEqual(await decisions(first), [
      { channel: "elicitation", answer: "allow_once", tier: "high" },
      { channel: "session", answer: "allow_once", tier: "high" },
    ]);
    const calls = await events(first, "tool_call");
    assert.deepEqual(calls.map((call) => (call.app as { tier?: unknown }).tier), ["high", "high"]);
    // A new server session does not carry the allow.
    assert.notEqual((await typeText(second)).isError, true);
    assert.equal(second.elicitations.length, 1);
  } finally {
    await first.close();
    await second.close();
  }
});

test("a high-risk app the user refused is refused earlier for the rest of the session, logged with tier high", async () => {
  const edge = windowOf(EDGE, "Microsoft Edge");
  const harness = await connectServer({ elicitation: "form", onElicit: () => ({ action: "decline" }), script: { handlers: { get_window: [{ result: edge }, { result: edge }] } } });
  try {
    assert.equal(code(await typeText(harness)), "approval_declined");
    assert.equal(code(await typeText(harness)), "approval_refused_earlier");
    assert.equal(harness.elicitations.length, 1);
    assert.deepEqual(await decisions(harness), [
      { channel: "elicitation", answer: "refuse", tier: "high" },
      { channel: "session", answer: "refused_earlier", tier: "high" },
    ]);
  } finally {
    await harness.close();
  }
});

test("the built-in high-risk apps are each asked about and logged as tier high", async () => {
  const apps = [
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files\\Mozilla Firefox\\firefox.exe",
    "C:\\Program Files\\Internet Explorer\\iexplore.exe",
    "C:\\Windows\\explorer.exe",
    "C:\\Windows\\ImmersiveControlPanel\\SystemSettings.exe",
    "C:\\Windows\\System32\\control.exe",
    "C:\\Program Files\\KeePassXC\\KeePassXC.exe",
    "C:\\Program Files\\Bitwarden\\Bitwarden.exe",
    "C:\\Program Files\\Microsoft Office\\root\\Office16\\OUTLOOK.EXE",
    "C:\\Program Files\\WindowsApps\\Microsoft.OutlookForWindows_1.2025.0.0_x64__8wekyb3d8bbwe\\olk.exe",
    // UWP Mail, matched by its package family name (the folder it is installed in).
    "C:\\Program Files\\WindowsApps\\microsoft.windowscommunicationsapps_16005.14326.22342.0_x64__8wekyb3d8bbwe\\HxOutlook.exe",
  ];
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: apps.map((app) => ({ result: windowOf(app, "App") })) } } });
  try {
    for (const app of apps) assert.notEqual((await typeText(harness)).isError, true, app);
    assert.equal(harness.elicitations.length, apps.length);
    const logged = await events(harness, "approval");
    assert.deepEqual(
      logged.map((event) => event.app),
      apps.map((app) => ({ path: app, key: app.toLowerCase(), tier: "high" })),
    );
  } finally {
    await harness.close();
  }
});

test("a normal app keeps today's approvals, logged as tier normal: asked, then allowed or refused earlier for the session", async () => {
  const allowing = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: demo }, { result: demo }] } } });
  const refusing = await connectServer({ elicitation: "form", onElicit: () => ({ action: "decline" }), script: { handlers: { get_window: [{ result: demo }, { result: demo }] } } });
  try {
    await typeText(allowing);
    await typeText(allowing);
    assert.deepEqual(await decisions(allowing), [
      { channel: "elicitation", answer: "allow_once", tier: "normal" },
      { channel: "session", answer: "allow_once", tier: "normal" },
    ]);
    await typeText(refusing);
    await typeText(refusing);
    assert.deepEqual(await decisions(refusing), [
      { channel: "elicitation", answer: "refuse", tier: "normal" },
      { channel: "session", answer: "refused_earlier", tier: "normal" },
    ]);
  } finally {
    await allowing.close();
    await refusing.close();
  }
});

test("the elevated-window rule comes before the tier: an elevated consent.exe is elevated_target", async () => {
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: [{ result: windowOf(CONSENT, "Consent", { elevated: true }) }] } } });
  try {
    assert.equal(code(await typeText(harness)), "elevated_target");
    assert.equal(harness.elicitations.length, 0);
    assert.deepEqual(await decisions(harness), []);
  } finally {
    await harness.close();
  }
});

// Ticket 111: the built-in tier each app has had since ticket 91, pinned before the matching was
// shared with `builtInTierOf`; only the Store Windows Terminal families were added after it.
type Characterized = [app: string, family: string | undefined, tier: "blocked" | "high" | "normal", why: string | null];
const SHELL_WHY = "it is a command shell or terminal, which can run any program";
const ADMIN_WHY = "it is a Windows administration tool (the registry, management consoles, running processes)";
const BROWSER_WHY = "a web browser";
const SETTINGS_WHY = "Windows Settings or Control Panel";
const MAIL_WHY = "a mail app";
const stems = (folder: string, names: string[], tier: Characterized[2], why: string): Characterized[] => names.map((stem) => [`${folder}\\${stem}.exe`, undefined, tier, why]);
const CHARACTERIZED: Characterized[] = [
  ...stems("C:\\Tools", ["CMD", "PowerShell", "pwsh", "powershell_ise", "WindowsTerminal", "wt", "OpenConsole", "conhost", "wsl", "bash", "mintty", "alacritty", "wezterm", "wezterm-gui", "hyperkitty", "warp"], "blocked", SHELL_WHY),
  ["C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.21.0_x64__8wekyb3d8bbwe\\WindowsTerminal.exe", undefined, "blocked", SHELL_WHY],
  ["C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.21.0_x64__8wekyb3d8bbwe\\WindowsTerminal.exe", "Microsoft.WindowsTerminal_8wekyb3d8bbwe", "blocked", SHELL_WHY],
  ...stems("C:\\Windows\\System32", ["regedit", "mmc", "Taskmgr"], "blocked", ADMIN_WHY),
  [CONSENT, undefined, "blocked", "it is the Windows UAC consent prompt, which grants administrator rights"],
  [CREDENTIAL_BROKER, undefined, "blocked", "it is the Windows credential prompt, which takes passwords and PINs"],
  ...stems("C:\\Deskpass\\dist", ["deskpass-helper", "computer-use-helper"], "blocked", "it is part of Deskpass itself"),
  ...stems("C:\\Browsers", ["msedge", "chrome", "firefox", "brave", "opera", "vivaldi", "iexplore"], "high", BROWSER_WHY),
  ["C:\\Windows\\explorer.exe", undefined, "high", "File Explorer"],
  ...stems("C:\\Windows\\System32", ["SystemSettings", "control"], "high", SETTINGS_WHY),
  ["C:\\Apps\\Settings.exe", "windows.immersivecontrolpanel_cw5n1h2txyewy", "high", SETTINGS_WHY],
  ["C:\\Program Files\\WindowsApps\\windows.immersivecontrolpanel_10.0.6.1000_neutral_neutral_cw5n1h2txyewy\\SettingsHost.exe", undefined, "high", SETTINGS_WHY],
  ...stems("C:\\Vaults", ["1Password", "Bitwarden", "KeePass", "KeePassXC", "Dashlane", "LastPass"], "high", "a password manager"),
  ...stems("C:\\Mail", ["OUTLOOK", "olk", "thunderbird"], "high", MAIL_WHY),
  ["C:\\Program Files\\WindowsApps\\microsoft.windowscommunicationsapps_16005.14326.22342.0_x64__8wekyb3d8bbwe\\HxOutlook.exe", undefined, "high", MAIL_WHY],
  ["C:\\Apps\\Host.exe", "Microsoft.OutlookForWindows_8wekyb3d8bbwe", "high", MAIL_WHY],
  // Neither list: normal, including names that only contain a listed one.
  ["C:\\Apps\\demo.exe", undefined, "normal", null],
  ["C:\\Apps\\cmd2.exe", undefined, "normal", null],
  ["C:\\Apps\\explorer.exe.bak", undefined, "normal", null],
  ["C:\\Program Files\\WindowsApps\\Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe\\CalculatorApp.exe", undefined, "normal", null],
  ["C:\\Apps\\Notes.exe", "Contoso.Notes_abcdefgh", "normal", null],
  ["C:\\Windows\\System32\\ApplicationFrameHost.exe", undefined, "normal", null],
];

test("ticket 111: every app keeps the built-in tier it had before the matching was shared", () => {
  for (const [app, family, tier, why] of CHARACTERIZED) {
    assert.deepEqual(classifyApp(app, [], undefined, family), { tier, why }, `${app} ${family ?? ""}`);
  }
});

const CJK = /[\u4e00-\u9fff]/;

test("ticket 111: every built-in group has a zh-TW name and reason (by its id) next to its English why", () => {
  const groups = [...BUILT_IN_TIERS.blocked, ...BUILT_IN_TIERS.high];
  assert.equal(new Set(groups.map((group) => group.id)).size, groups.length, "each group has its own id");
  for (const group of groups) {
    assert.match(SETTINGS_TEXT.builtInTiers[group.id].label, CJK, group.why);
    assert.match(SETTINGS_TEXT.builtInTiers[group.id].reason, CJK, group.why);
    assert.ok(group.why.length > 0);
  }
});

test("ticket 111: builtInTierOf names an app's built-in tier and group by its identity key and executable", () => {
  assert.deepEqual(builtInTierOf("package:microsoft.windowsterminal_8wekyb3d8bbwe", ""), { tier: "blocked", group: "terminal", why: "it is a command shell or terminal, which can run any program" });
  assert.deepEqual(builtInTierOf("package:microsoft.windowsterminalpreview_8wekyb3d8bbwe", "Windows Terminal Preview")?.tier, "blocked");
  assert.deepEqual(builtInTierOf("signer:CN=Google LLC|chrome.exe", ""), { tier: "high", group: "browser", why: "a web browser" });
  assert.deepEqual(builtInTierOf("c:\\deskpass\\dist\\deskpass-helper.exe", "")?.group, "deskpass");
  assert.equal(builtInTierOf("c:\\windows\\explorer.exe", "C:\\Windows\\explorer.exe")?.tier, "high");
  // A package's executable counts as it does in the approval check.
  assert.equal(builtInTierOf("package:contoso.innocent_abcdefgh", "C:\\Program Files\\WindowsApps\\Contoso.Innocent_1.0.0.0_x64__abcdefgh\\WindowsTerminal.exe")?.tier, "blocked");
  assert.equal(builtInTierOf("package:contoso.notes_abcdefgh", ""), null);
  assert.equal(builtInTierOf("c:\\apps\\demo.exe", "Demo"), null);
});

test("ticket 111: builtInTierOf agrees with the approval check for every characterized app", () => {
  const terminals: Characterized[] = ["Microsoft.WindowsTerminal_8wekyb3d8bbwe", "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe"].map((family) => ["C:\\Apps\\TerminalHost.exe", family, "blocked", SHELL_WHY]);
  for (const [app, family] of [...CHARACTERIZED, ...terminals]) {
    // The key the approval check gives it (an unsigned app): its package family, else its path.
    const key = family === undefined ? app.toLowerCase() : `package:${family.toLowerCase()}`;
    const approval = classifyApp(app, [], undefined, family).tier;
    assert.equal(builtInTierOf(key, app)?.tier ?? "normal", approval, `${key} ${app}`);
    if (family !== undefined) assert.equal(builtInTierOf(key, "")?.tier ?? "normal", classifyApp("", [], undefined, family).tier, key);
  }
});

test("ticket 116: computer_list_apps marks a uwp: app blocked by its package family, as the approval check does (the Store Terminal Preview)", async () => {
  const apps = [
    { app_id: "uwp:Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe!App", name: "終端機 Preview", kind: "uwp", running: false },
    { app_id: "uwp:Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", name: "終端機", kind: "uwp", running: false },
    { app_id: "uwp:Microsoft.WindowsNotepad_8wekyb3d8bbwe!App", name: "記事本", kind: "uwp", running: false },
  ];
  const harness = await connectServer({ script: { handlers: { list_apps: [{ result: { apps, total: apps.length } }] } } });
  try {
    const text = textContent((await harness.client.callTool({ name: "computer_list_apps", arguments: {} })) as Parameters<typeof textContent>[0]).split("\n");
    assert.match(text[0] ?? "", /; blocked: true \(in the blocked tier/, text[0]);
    assert.match(text[1] ?? "", /; blocked: true \(in the blocked tier/, text[1]);
    assert.doesNotMatch(text[2] ?? "", /blocked/, text[2]);
  } finally {
    await harness.close();
  }
});
