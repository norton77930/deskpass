import { execFile } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import path from "node:path";

export async function resolveAppPath(app: string): Promise<string | null> {
  if (existsSync(app)) return realPath(app);
  const file = app.toLowerCase().endsWith(".exe") ? app : `${app}.exe`;
  for (const root of ["HKCU", "HKLM"]) {
    const found = await appPath(root, file);
    if (found && existsSync(found)) return realPath(found);
  }
  return null;
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
    execFile("reg.exe", ["query", key, "/ve"], { windowsHide: true, timeout: 5_000 }, (error, stdout) => {
      if (error) {
        resolve(null);
        return;
      }
      const match = /REG_SZ\s+(.+)\s*$/m.exec(stdout);
      resolve(match?.[1]?.trim() || null);
    });
  });
}
