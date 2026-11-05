import assert from "node:assert/strict";
import { test } from "node:test";
import { connectServer, textContent } from "./harness.ts";

const launched = {
  pid: 50,
  window: {
    window_id: "9",
    app: "C:\\Apps\\demo.exe",
    app_name: "Demo",
    title: "Demo",
    minimized: false,
    elevated: false,
  },
};

test("launching an app asks for approval and returns the new window", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async () => "C:\\Apps\\demo.exe",
    script: { handlers: { launch_app: [{ result: launched }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } });
    assert.notEqual(result.isError, true);
    assert.match(textContent(result), /window_id: 9/);
    assert.equal(harness.elicitations.length, 1);
    assert.match(harness.elicitations[0] ?? "", /demo\.exe/i);
    const requests = await harness.requests();
    const launch = requests.find((entry) => (entry as { method?: string }).method === "launch_app") as { params?: { app?: string } };
    assert.equal(launch.params?.app, "C:\\Apps\\demo.exe");
  } finally {
    await harness.close();
  }
});

test("a high-risk app is not launched", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async () => "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
    script: { handlers: { launch_app: [{ result: launched }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "powershell" } });
    assert.equal(textContent(result).split(/\r?\n/)[0], "blocked_app");
    assert.equal(harness.elicitations.length, 0);
    const requests = await harness.requests();
    assert.equal(requests.some((entry) => (entry as { method?: string }).method === "launch_app"), false);
  } finally {
    await harness.close();
  }
});

test("a high-risk short name is blocked before resolveApp is called", async () => {
  for (const name of ["cmd", "PowerShell.EXE", "wt"]) {
    let resolveCalled = false;
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async () => {
        resolveCalled = true;
        return null;
      },
      script: { handlers: { launch_app: [{ result: launched }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: name } });
      assert.equal(textContent(result).split(/\r?\n/)[0], "blocked_app", name);
      assert.equal(resolveCalled, false, `resolveApp should not be called for ${name}`);
      assert.equal(harness.elicitations.length, 0);
      const requests = await harness.requests();
      assert.equal(requests.some((entry) => (entry as { method?: string }).method === "launch_app"), false);
    } finally {
      await harness.close();
    }
  }
});

// Ticket 34: ApplicationFrameHost.exe only hosts UWP apps' windows; approving it would cover none
// of them, so launching it is blocked without asking, by name, by path, or once resolved.
test("ApplicationFrameHost.exe is blocked without asking, by name, path or resolved path", async () => {
  const framehost = String.raw`C:\Windows\System32\ApplicationFrameHost.exe`;
  for (const name of ["ApplicationFrameHost", "applicationframehost.EXE", framehost, "C:/WINDOWS/system32/ApplicationFrameHost.exe", "frame host"]) {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async () => framehost,
      script: { handlers: { launch_app: [{ result: launched }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: name } });
      const [code, message] = textContent(result).split(/\r?\n/);
      assert.equal(code, "blocked_app", name);
      assert.match(message, /hosts the windows of UWP apps and is not an app itself/, name);
      assert.equal(harness.elicitations.length, 0, name);
      const requests = await harness.requests();
      assert.equal(requests.some((entry) => (entry as { method?: string }).method === "launch_app"), false, name);
    } finally {
      await harness.close();
    }
  }
});

test("a non-high-risk short name that cannot be resolved is still launch_failed", async () => {
  let resolveCalled = false;
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async () => {
      resolveCalled = true;
      return null;
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "notepad" } });
    assert.equal(textContent(result).split(/\r?\n/)[0], "launch_failed");
    assert.equal(resolveCalled, true);
  } finally {
    await harness.close();
  }
});

test("a missing executable or helper failure is launch_failed", async () => {
  const missing = await connectServer({ elicitation: "form", resolveApp: async () => null });
  const failed = await connectServer({
    elicitation: "form",
    resolveApp: async () => "C:\\Apps\\demo.exe",
    script: { handlers: { launch_app: [{ error: { code: "launch_failed", message: "The process exited." } }] } },
  });
  try {
    const notFound = await missing.client.callTool({ name: "computer_launch_app", arguments: { app: "missing" } });
    assert.equal(textContent(notFound).split(/\r?\n/)[0], "launch_failed");
    assert.equal(missing.elicitations.length, 0);
    const helperFailed = await failed.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } });
    assert.equal(textContent(helperFailed).split(/\r?\n/)[0], "launch_failed");
  } finally {
    await missing.close();
    await failed.close();
  }
});

type Logged = { method?: string; params?: Record<string, unknown> };

function sent(requests: unknown[], method: string): Logged[] {
  return (requests as Logged[]).filter((entry) => entry.method === method);
}

// Ticket 79: computer_launch_app takes an app_id from computer_list_apps instead of app.
test("app and app_id together, or neither, are invalid_params without asking or launching", async () => {
  for (const args of [{ app: "demo", app_id: String.raw`exe:C:\Apps\demo.exe` }, {}]) {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async () => "C:\\Apps\\demo.exe",
      script: { handlers: { launch_app: [{ result: launched }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: args });
      const [code, message] = textContent(result).split(/\r?\n/);
      assert.equal(code, "invalid_params", JSON.stringify(args));
      assert.match(message, /\bapp\b.*\bapp_id\b/, JSON.stringify(args));
      assert.equal(harness.elicitations.length, 0);
      const requests = await harness.requests();
      assert.equal(sent(requests, "launch_app").length + sent(requests, "resolve_app_id").length, 0);
    } finally {
      await harness.close();
    }
  }
});

test("an app_id that computer_list_apps does not give is refused clearly, without quoting it", async () => {
  for (const appId of ["notepad", "foo:bar-secret", "exe:notepad.exe", String.raw`exe:C:\Apps\readme.txt`, "uwp:", "uwp:NoBang", String.raw`shell:..\x-secret`, "shell:", 42, ""]) {
    const harness = await connectServer({ elicitation: "form", resolveApp: async () => "C:\\Apps\\demo.exe" });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: appId } });
      const [code, message] = textContent(result).split(/\r?\n/);
      assert.equal(code, "invalid_params", String(appId));
      assert.match(message, /computer_list_apps/, String(appId));
      assert.doesNotMatch(message, /secret|readme|NoBang/, String(appId));
      assert.equal(harness.elicitations.length, 0);
      const requests = await harness.requests();
      assert.equal(sent(requests, "launch_app").length + sent(requests, "resolve_app_id").length, 0, String(appId));
    } finally {
      await harness.close();
    }
  }
});

test("an exe: app_id is approved and launched by its executable path", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async (app) => (app === "C:\\Apps\\demo.exe" ? app : null),
    script: { handlers: { launch_app: [{ result: launched }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: String.raw`exe:C:\Apps\demo.exe` } });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /window_id: 9/);
    assert.equal(harness.elicitations.length, 1);
    assert.match(harness.elicitations[0] ?? "", /C:\\Apps\\demo\.exe/);
    const [launch] = sent(await harness.requests(), "launch_app");
    assert.deepEqual(launch?.params, { app: "C:\\Apps\\demo.exe" });
  } finally {
    await harness.close();
  }
});

const CALCULATOR_AUMID = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
const CALCULATOR_EXE = String.raw`C:\Program Files\WindowsApps\Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe\CalculatorApp.exe`;

test("a uwp: app_id is approved by the package's executable and launched by its AUMID", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async (app) => (app === CALCULATOR_EXE ? app : null),
    script: {
      handlers: {
        resolve_app_id: [{ result: { app: CALCULATOR_EXE, aumid: CALCULATOR_AUMID } }],
        launch_app: [{ result: { ...launched, kind: "main" } }, { result: { ...launched, kind: "main" } }],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: `uwp:${CALCULATOR_AUMID}` } });
    assert.notEqual(result.isError, true, textContent(result));
    assert.match(textContent(result), /window_id: 9/);
    assert.doesNotMatch(textContent(result), /kind:/, "a main window reads as before");
    assert.equal(harness.elicitations.length, 1);
    assert.match(harness.elicitations[0] ?? "", /CalculatorApp\.exe/);
    // The approval is the executable's: launching it by path asks nothing more.
    const again = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: CALCULATOR_EXE } });
    assert.notEqual(again.isError, true, textContent(again));
    assert.equal(harness.elicitations.length, 1);
    const requests = await harness.requests();
    assert.deepEqual(sent(requests, "resolve_app_id").map((entry) => entry.params), [{ app_id: `uwp:${CALCULATOR_AUMID}` }]);
    assert.deepEqual(sent(requests, "launch_app").map((entry) => entry.params), [{ app: CALCULATOR_EXE, aumid: CALCULATOR_AUMID }, { app: CALCULATOR_EXE }]);
  } finally {
    await harness.close();
  }
});

test("a high-risk app cannot be launched through any app_id form", async () => {
  const terminal = String.raw`C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21.0_x64__8wekyb3d8bbwe\WindowsTerminal.exe`;
  const powershell = String.raw`C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe`;
  const cases: { appId: string; resolved?: string }[] = [
    // Blocked by the package name before the helper is asked.
    { appId: "uwp:Microsoft.WindowsTerminal_8wekyb3d8bbwe!App" },
    { appId: String.raw`exe:C:\Windows\System32\cmd.exe` },
    // Blocked once the helper says which executable it is.
    { appId: "uwp:Contoso.Innocent_abcdefgh!App", resolved: terminal },
    { appId: "shell:Contoso.Shortcut", resolved: powershell },
    { appId: "shell:Contoso.Frame", resolved: String.raw`C:\Windows\System32\ApplicationFrameHost.exe` },
  ];
  for (const { appId, resolved } of cases) {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async (app) => app,
      script: { handlers: { resolve_app_id: [{ result: { app: resolved } }], launch_app: [{ result: launched }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: appId } });
      assert.equal(textContent(result).split(/\r?\n/)[0], "blocked_app", appId);
      assert.equal(harness.elicitations.length, 0, appId);
      const requests = await harness.requests();
      assert.equal(sent(requests, "launch_app").length, 0, appId);
      if (!resolved) assert.equal(sent(requests, "resolve_app_id").length, 0, appId);
    } finally {
      await harness.close();
    }
  }
});

test("an app_id the helper cannot resolve is refused without asking", async () => {
  const failing = [
    { error: { code: "launch_failed", message: "That app is not installed for this user." } },
    // A result without an executable is never approved.
    { result: {} },
  ];
  for (const step of failing) {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async (app) => app,
      script: { handlers: { resolve_app_id: [step], launch_app: [{ result: launched }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: "uwp:Contoso.Gone_abcdefgh!App" } });
      assert.equal(textContent(result).split(/\r?\n/)[0], "launch_failed", JSON.stringify(step));
      assert.equal(harness.elicitations.length, 0);
      assert.equal(sent(await harness.requests(), "launch_app").length, 0);
    } finally {
      await harness.close();
    }
  }
});

test("a launch that only showed a dialog returns it marked kind: dialog with an explanation", async () => {
  const dialog = { ...launched.window, window_id: "12", title: "Sign in" };
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async () => "C:\\Apps\\demo.exe",
    script: { handlers: { launch_app: [{ result: { pid: 50, window: dialog, kind: "dialog" } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } });
    assert.notEqual(result.isError, true);
    const text = textContent(result);
    assert.match(text, /window_id: 12/);
    assert.match(text, /^kind: dialog$/m);
    assert.match(text, /dialog, not its main window.*main window may appear later/s);
  } finally {
    await harness.close();
  }
});

test("launch still succeeds when no window appears within 10 seconds", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async () => "C:\\Apps\\demo.exe",
    script: { handlers: { launch_app: [{ result: { pid: 50, window: null } }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: "demo" } });
    assert.notEqual(result.isError, true);
    assert.match(textContent(result), /computer_list_windows/);
  } finally {
    await harness.close();
  }
});

test("a file that is not an .exe is not launched: a script or shortcut would run another program", async () => {
  for (const file of [String.raw`C:\Users\me\run.bat`, String.raw`C:\Users\me\run.CMD`, String.raw`C:\Users\me\Demo.lnk`, String.raw`C:\Users\me\x.ps1`, String.raw`C:\Users\me\x.exe.vbs`]) {
    const harness = await connectServer({
      elicitation: "form",
      resolveApp: async () => file,
      script: { handlers: { launch_app: [{ result: launched }] } },
    });
    try {
      const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app: file } });
      assert.equal(result.isError, true, file);
      assert.equal(textContent(result).split(/\r?\n/)[0], "invalid_params", file);
      assert.match(textContent(result), /\.exe files only/, file);
      assert.equal(harness.elicitations.length, 0, file);
      assert.equal(sent(await harness.requests(), "launch_app").length, 0, file);
    } finally {
      await harness.close();
    }
  }
});

test("an app_id whose shortcut resolves to a script is not launched", async () => {
  const harness = await connectServer({
    elicitation: "form",
    resolveApp: async (app) => app,
    script: { handlers: { resolve_app_id: [{ result: { app: String.raw`C:\Tools\start.cmd` } }], launch_app: [{ result: launched }] } },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_launch_app", arguments: { app_id: "shell:Contoso.Starter" } });
    assert.equal(textContent(result).split(/\r?\n/)[0], "invalid_params");
    assert.equal(harness.elicitations.length, 0);
    assert.equal(sent(await harness.requests(), "launch_app").length, 0);
  } finally {
    await harness.close();
  }
});
