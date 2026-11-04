import assert from "node:assert/strict";
import { rm } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { connectServer, repoRoot, textContent } from "./harness.ts";

test("the test client can connect with or without an elicitation capability", async () => {
  const declared = await connectServer({ elicitation: "form" });
  const omitted = await connectServer();
  try {
    const declaredTools = await declared.client.listTools();
    const omittedTools = await omitted.client.listTools();
    assert.equal(declaredTools.tools.some((tool) => tool.name === "computer_list_windows"), true);
    assert.equal(omittedTools.tools.some((tool) => tool.name === "computer_list_windows"), true);
  } finally {
    await declared.close();
    await omitted.close();
  }
});

test("computer_list_windows returns each window field from the helper", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [
          {
            result: {
              windows: [
                {
                  window_id: "101",
                  app: "C:\\Windows\\notepad.exe",
                  app_name: "Notepad",
                  title: "Untitled",
                  minimized: false,
                  elevated: false,
                },
                {
                  window_id: "202",
                  app: "C:\\Apps\\paint.exe",
                  app_name: "Paint",
                  title: "Image",
                  minimized: true,
                  elevated: true,
                },
              ],
            },
          },
        ],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.notEqual(result.isError, true);
    const text = textContent(result);
    assert.match(text, /window_id: 101/);
    assert.match(text, /app: C:\\Windows\\notepad.exe/);
    assert.match(text, /app_name: Notepad/);
    assert.match(text, /title: Untitled/);
    assert.match(text, /minimized: false/);
    assert.match(text, /elevated: false/);
    assert.match(text, /window_id: 202/);
    assert.match(text, /app_name: Paint/);
    assert.match(text, /title: Image/);
    assert.match(text, /minimized: true/);
    assert.match(text, /elevated: true/);
  } finally {
    await harness.close();
  }
});

test("helper errors are tool errors whose first line is the stable code", async () => {
  const harness = await connectServer({
    script: {
      handlers: {
        list_windows: [{ error: { code: "internal", message: "enumeration failed" } }],
      },
    },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(result.isError, true);
    const lines = textContent(result).split(/\r?\n/);
    assert.equal(lines[0], "internal");
    assert.match(lines[1] ?? "", /enumeration failed/);
  } finally {
    await harness.close();
  }
});

test("non-windows platforms return windows_only without starting the helper", async () => {
  const marker = join(repoRoot(), "test-tmp", "linux-marker");
  await rm(marker, { force: true });
  const harness = await connectServer({
    platform: "linux",
    script: { markerPath: marker },
  });
  try {
    const result = await harness.client.callTool({ name: "computer_list_windows", arguments: {} });
    assert.equal(result.isError, true);
    const lines = textContent(result).split(/\r?\n/);
    assert.equal(lines[0], "windows_only");
    assert.match(lines.slice(1).join("\n"), /Windows only/);
    const requests = await harness.requests();
    assert.equal(requests.length, 0);
    await assert.rejects(rm(marker), /ENOENT/);
  } finally {
    await harness.close();
    await rm(marker, { force: true });
  }
});
