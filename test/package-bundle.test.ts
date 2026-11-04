import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { bundleServer } from "../scripts/package.mjs";
import { repoRoot } from "./harness.ts";

// Ticket 134. The release holds one bundled dist/index.js and no node_modules, so the bundle must
// run where the repo's node_modules cannot be reached. The folder is under the system temp folder,
// not under the repo (a worktree resolves packages from a parent folder).

test("the bundled server answers initialize and tools/list with no node_modules around it", async () => {
  const dir = await mkdtemp(join(tmpdir(), "deskpass-bundle-"));
  try {
    const outfile = join(dir, "dist", "index.js");
    await bundleServer({ root: repoRoot(), outfile });

    const text = await readFile(outfile, "utf8");
    assert.ok(text.startsWith("#!/usr/bin/env node"), "the shebang is kept");
    assert.ok(!text.includes("sourceMappingURL"), "no source map");
    // Only Node's own modules are imported; the SDK and zod are inlined. (Not assert.doesNotMatch: it prints the whole bundle.)
    const outside = text.split("\n").filter((line) => /^import\b/.test(line) && !/["']node:[a-z/_]+["']/.test(line));
    assert.deepEqual(outside, [], "imports a package from outside the bundle");

    const transport = new StdioClientTransport({ command: process.execPath, args: [outfile], cwd: dir, stderr: "pipe" });
    const client = new Client({ name: "deskpass-bundle-test", version: "0.0.0" }, { capabilities: {} });
    await client.connect(transport);
    try {
      const names = (await client.listTools()).tools.map((tool) => tool.name);
      assert.ok(names.includes("computer_click"), names.join(", "));
      assert.ok(names.includes("computer_screenshot"), names.join(", "));
    } finally {
      await client.close();
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
