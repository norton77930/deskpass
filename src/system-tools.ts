import path from "node:path";

/**
 * The full path of a Windows tool in `System32` (`reg.exe`, `rundll32.exe`). A tool is never
 * started by its bare name: Windows (libuv) looks for a bare name in the current directory before
 * PATH, and this server's current directory is whatever the MCP client started it in, usually the
 * user's project, so a `reg.exe` in a cloned repository would run as the user. The Windows folder
 * comes from `SystemRoot` (then `windir`), as Windows sets it for every process.
 */
export function systemTool(name: string, env: NodeJS.ProcessEnv = process.env): string {
  const root = [env.SystemRoot, env.windir].find((value) => typeof value === "string" && path.win32.isAbsolute(value) && /^[a-z]:\\/i.test(value)) ?? "C:\\Windows";
  return path.win32.join(root, "System32", name);
}
