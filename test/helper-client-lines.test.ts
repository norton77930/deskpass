import assert from "node:assert/strict";
import { test } from "node:test";
import { PROTOCOL_VERSION } from "../src/config.ts";
import { HelperClient } from "../src/helper-client.ts";
import { launchScriptedHelper } from "./harness.ts";

// Ticket 127: the helper's lines end at "\n" only. node:readline would also end one at U+2028 or U+2029,
// which JSON leaves unescaped inside a string (a Start Menu shortcut can be named with them).
const ODD_NAME = "Odd App Name";

test("a response whose string holds raw U+2028 and U+2029 is one line", async () => {
  const response = { apps: [{ name: ODD_NAME }] };
  assert.ok(JSON.stringify(response).includes(" "), "JSON.stringify leaves U+2028 raw");
  const { helper, close } = await launchScriptedHelper({ handlers: { list_start_menu_apps: [{ result: response }] } });
  try {
    assert.deepEqual(await helper.request("list_start_menu_apps", {}), response);
  } finally {
    await close();
  }
});

// A helper that answers each request in two writes: the response is cut in the middle of a multibyte
// character (the euro sign) and ended with CRLF. The answer to the ping is sent whole.
const CHUNKED_HELPER = `
const name = "Euro \\u20ac \\u2028 \\ud83d\\ude00";
process.stdout.write('{"event":"ready"}\\n');
let pending = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  pending += chunk;
  let end;
  while ((end = pending.indexOf("\\n")) >= 0) {
    const request = JSON.parse(pending.slice(0, end));
    pending = pending.slice(end + 1);
    if (request.method === "ping") {
      process.stdout.write(JSON.stringify({ id: request.id, ok: true, result: { version: ${JSON.stringify(PROTOCOL_VERSION)} } }) + "\\n");
      continue;
    }
    const bytes = Buffer.from(JSON.stringify({ id: request.id, ok: true, result: { name } }) + "\\r\\n", "utf8");
    const cut = bytes.indexOf(Buffer.from("\\u20ac", "utf8")) + 1;
    process.stdout.write(bytes.subarray(0, cut));
    setTimeout(() => process.stdout.write(bytes.subarray(cut)), 50);
  }
});
setInterval(() => {}, 1000);
`;

test("a multibyte character split across two chunks, and a CRLF line end, still parse", async () => {
  const helper = new HelperClient({
    helperPath: process.execPath,
    helperArgs: ["-e", CHUNKED_HELPER, "--"],
    serverPid: process.pid,
    expectedBuild: null,
  });
  try {
    assert.deepEqual(await helper.request("list_start_menu_apps", {}), { name: "Euro €   \u{1F600}" });
  } finally {
    await helper.dispose();
  }
});
