#!/usr/bin/env node
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createServer } from "./server.ts";
import { runSettings } from "./settings-server.ts";
import { uiLanguage } from "./ui-language.ts";

// Ticket 112: `deskpass settings` opens the settings web page; with no subcommand this is the MCP
// server. Ticket 118: the native control panel is gone; `deskpass panel` only points to the
// settings page (on stderr, as every line the command prints) and ends without error.
if (process.argv[2] === "panel") {
  const message = (await uiLanguage()) === "zh-TW" ? "控制台已改成設定網頁，請用 deskpass settings" : "The control panel is now the settings page: run deskpass settings";
  console.error(message);
} else if (process.argv[2] === "settings") {
  process.exitCode = await runSettings();
} else {
  const app = createServer();
  await app.server.connect(new StdioServerTransport());
}
