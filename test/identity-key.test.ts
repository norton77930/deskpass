import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { approvalEvents, connectServer, repoRoot, textContent, type Harness } from "./harness.ts";

// Ticket 93: every approval check asks the helper who the app is (`app_identity`), cached by the
// executable's path and modification time, and the log names the app by that identity's key: the
// signer and file name of a signed app, the package family of a packaged one, the normalized path
// otherwise. An identity that cannot be had is treated as unsigned; the tool goes on. Seam one:
// the MCP client, the server and the scripted helper, the log in the case's temp folder.

const demo = {
  window_id: "5",
  app: "C:\\Apps\\Demo.exe",
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

const SIGNER = "C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd";

const windows = (count: number, window: Record<string, unknown> = demo) => Array.from({ length: count }, () => ({ result: window }));

function typeText(harness: Harness) {
  return harness.client.callTool({ name: "computer_type_text", arguments: { window_id: "5", text: "x" } });
}

async function toolCallApps(harness: Harness): Promise<unknown[]> {
  const day = new Date();
  const name = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}.jsonl`;
  const raw = await readFile(join(harness.dir, "logs", name), "utf8");
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "tool_call")
    .map((event) => event.app);
}

async function identityRequests(harness: Harness): Promise<Record<string, unknown>[]> {
  return ((await harness.requests()) as { method?: string; params?: Record<string, unknown> }[])
    .filter((entry) => entry.method === "app_identity")
    .map((entry) => entry.params ?? {});
}

test("a signed app is logged by its signer and file name, a packaged one by its package family", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: windows(2),
        // Ticket 95: both can be always allowed, so the dialog asks (Allow once).
        ask_user: [{ result: { answer: "allow" } }, { result: { answer: "allow" } }],
        app_identity: [
          { result: { signed: true, signer: SIGNER, file_name: "demo.exe", original_name: "demo.exe" } },
          { result: { signed: false, signer: null, file_name: "demo.exe", package_family_name: "Contoso.Demo_8wekyb3d8bbwe" } },
        ],
      },
    },
  });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    const [signed, packaged] = await toolCallApps(harness);
    assert.deepEqual(signed, { path: demo.app, key: `signer:${SIGNER}|demo.exe`, tier: "normal" });
    assert.deepEqual(packaged, { path: demo.app, key: "package:contoso.demo_8wekyb3d8bbwe", tier: "normal" });
    const [approval] = await approvalEvents(harness);
    assert.deepEqual(approval?.app, signed, "the approval event names the app the same way");
    // The window's path and process go to the helper, once per approval check.
    assert.deepEqual(await identityRequests(harness), [
      { path: demo.app, pid: demo.pid },
      { path: demo.app, pid: demo.pid },
    ]);
  } finally {
    await harness.close();
  }
});

test("a path written with forward slashes goes to the helper with backslashes", async () => {
  const slashed = { ...demo, app: "C:/Apps/Demo.exe" };
  const harness = await connectServer({ elicitation: "form", script: { handlers: { get_window: windows(1, slashed) } } });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await identityRequests(harness), [{ path: "C:\\Apps\\Demo.exe", pid: demo.pid }]);
  } finally {
    await harness.close();
  }
});

test("an app whose identity cannot be had is unsigned: logged by its normalized path, and the tool goes on", async () => {
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: windows(3),
        app_identity: [
          { error: { code: "internal", message: "WinVerifyTrust failed" } },
          { result: { signed: true, signer: "", file_name: "demo.exe" } },
          { result: { signed: false, signer: null, file_name: "demo.exe" } },
        ],
      },
    },
  });
  try {
    for (let call = 0; call < 3; call += 1) assert.notEqual((await typeText(harness)).isError, true, `call ${call + 1}`);
    const unsigned = { path: demo.app, key: "c:\\apps\\demo.exe", tier: "normal" };
    assert.deepEqual(await toolCallApps(harness), [unsigned, unsigned, unsigned]);
  } finally {
    await harness.close();
  }
});

test("the package family the helper reports decides a packaged app's tier", async () => {
  const mail = { ...demo, app: "C:\\Apps\\HxOutlook.exe", app_name: "Mail" };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: windows(1, mail),
        app_identity: [{ result: { signed: false, signer: null, file_name: "hxoutlook.exe", package_family_name: "microsoft.windowscommunicationsapps_8wekyb3d8bbwe" } }],
      },
    },
  });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.deepEqual(await toolCallApps(harness), [{ path: mail.app, key: "package:microsoft.windowscommunicationsapps_8wekyb3d8bbwe", tier: "high" }]);
  } finally {
    await harness.close();
  }
});

test("ticket 111: the Store Windows Terminal is blocked by its package family, whatever its executable is named", async () => {
  for (const family of ["Microsoft.WindowsTerminal_8wekyb3d8bbwe", "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe"]) {
    // A file name on no list, so only the family can block it.
    const terminal = { ...demo, app: "C:\\Apps\\TerminalHost.exe", app_name: "Terminal" };
    const harness = await connectServer({
      elicitation: "form",
      script: { handlers: { get_window: windows(1, terminal), app_identity: [{ result: { signed: false, signer: null, file_name: "terminalhost.exe", package_family_name: family } }] } },
    });
    try {
      const result = await typeText(harness);
      assert.equal(result.isError, true, family);
      assert.equal(textContent(result).split(/\r?\n/)[0], "blocked_app", family);
      assert.equal(harness.elicitations.length, 0, family);
      assert.deepEqual(await toolCallApps(harness), [{ path: terminal.app, key: `package:${family.toLowerCase()}`, tier: "blocked" }], family);
    } finally {
      await harness.close();
    }
  }
});

test("the identity is cached by path and modification time, and asked again when the executable changes", async () => {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const folder = await mkdtemp(join(root, "identity-"));
  const exe = join(folder, "Cached.exe");
  await writeFile(exe, "not really an executable");
  const before = new Date("2026-01-01T00:00:00Z");
  await utimes(exe, before, before);
  const window = { ...demo, app: exe, app_name: "Cached" };
  const harness = await connectServer({
    elicitation: "form",
    script: {
      handlers: {
        get_window: windows(5, window),
        ask_user: [{ result: { answer: "allow" } }],
        app_identity: [
          { result: { signed: true, signer: SIGNER, file_name: "cached.exe", original_name: "cached.exe" } },
          { result: { signed: false, signer: null, file_name: "cached.exe" } },
        ],
      },
    },
  });
  try {
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal((await identityRequests(harness)).length, 1, "the second check used the cached identity");
    // The executable is replaced (an update, or another file under the same name).
    const after = new Date("2026-02-01T00:00:00Z");
    await utimes(exe, after, after);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal((await identityRequests(harness)).length, 2, "asked again once the modification time changed, then cached");
    // Rewritten to another size with the modification time put back: asked again as well.
    await writeFile(exe, "a different, longer executable");
    await utimes(exe, after, after);
    assert.notEqual((await typeText(harness)).isError, true);
    assert.equal((await identityRequests(harness)).length, 3, "asked again once the size changed");
    const signedKey = `signer:${SIGNER}|cached.exe`;
    const pathKey = exe.replaceAll("/", "\\").toLowerCase();
    assert.deepEqual((await toolCallApps(harness)).map((app) => (app as { key?: unknown }).key), [signedKey, signedKey, pathKey, pathKey, pathKey]);
  } finally {
    await harness.close();
    await rm(folder, { recursive: true, force: true });
  }
});
