import { execFile } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import path from "node:path";
import { systemTool } from "./system-tools.ts";

/**
 * The executable `app` names: an absolute path that exists, or a name the App Paths registry
 * knows (`notepad`, `winword.exe`). A relative path is never looked up on disk: it would be read
 * against this server's current directory, which the MCP client chose (usually the user's
 * project), so a file of that name in a cloned repository would be the one launched.
 */
export async function resolveAppPath(app: string): Promise<string | null> {
  if (isAbsoluteWindowsPath(app)) return existsSync(app) ? realPath(app) : null;
  if (/[\\/]/.test(app)) return null;
  const file = app.toLowerCase().endsWith(".exe") ? app : `${app}.exe`;
  for (const root of ["HKCU", "HKLM"]) {
    const found = await appPath(root, file);
    if (found && isAbsoluteWindowsPath(found) && existsSync(found)) return realPath(found);
  }
  return null;
}

/** A drive path (`C:\...`) or a UNC path (`\\server\...`), written with either slash. */
function isAbsoluteWindowsPath(file: string): boolean {
  return /^([a-z]:[\\/]|[\\/]{2}[^\\/])/i.test(file);
}

/**
 * The path under its real name (8.3 aliases expanded, links followed), so the high-risk check and
 * the approval see the program that runs, as its windows will report it.
 */
function realPath(file: string): string {
  try {
    return realpathSync.native(file);
  } catch {
    return path.resolve(file);
  }
}

function appPath(root: string, file: string): Promise<string | null> {
  const key = `${root}\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\${file}`;
  return new Promise((resolve) => {
    execFile(systemTool("reg.exe"), ["query", key, "/ve"], { windowsHide: true, timeout: 5_000 }, (error, stdout) => {
      if (error) {
        resolve(null);
        return;
      }
      const match = /REG_SZ\s+(.+)\s*$/m.exec(stdout);
      resolve(match?.[1]?.trim() || null);
    });
  });
}
