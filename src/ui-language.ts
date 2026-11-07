import { execFile } from "node:child_process";
import { systemTool } from "./system-tools.ts";

/**
 * Open-source ticket 01: the user interface language of Deskpass, decided once when a server (the
 * MCP server, or the separately launched settings server) starts, by one rule: Traditional Chinese
 * when the Windows UI language is Traditional Chinese (zh-TW, zh-HK, zh-MO or any zh-Hant), English
 * otherwise; `DESKPASS_LANG` (`en` or `zh-TW`) overrides it, any other value is ignored.
 */
export type UiLanguage = "en" | "zh-TW";

export const UI_LANGUAGE_ENV = "DESKPASS_LANG";

/** Whether the BCP 47 tag `tag` names Traditional Chinese. */
function isTraditionalChinese(tag: string): boolean {
  const parts = tag.trim().replaceAll("_", "-").toLowerCase().split("-");
  if (parts[0] !== "zh") return false;
  const script = parts.find((part, index) => index > 0 && part.length === 4);
  if (script !== undefined) return script === "hant";
  return parts.slice(1).some((part) => part === "tw" || part === "hk" || part === "mo");
}

/**
 * The rule: `override` (the value of `DESKPASS_LANG`) when it is exactly `en` or `zh-TW`; else the
 * first (preferred) of the Windows UI languages `lookup` gives; `en` when it gives none or fails.
 */
export async function decideUiLanguage(override: string | undefined, lookup: () => Promise<readonly string[]>): Promise<UiLanguage> {
  if (override === "en" || override === "zh-TW") return override;
  try {
    const [first] = await lookup();
    return first !== undefined && isTraditionalChinese(first) ? "zh-TW" : "en";
  } catch {
    return "en";
  }
}

/** The strings of the REG_MULTI_SZ value `name` in `reg.exe query` output (it prints them joined by a literal `\0`); [] when absent. */
export function parseRegMultiSz(output: string, name: string): string[] {
  for (const line of output.split(/\r?\n/)) {
    const match = /^\s+(\S+)\s+REG_MULTI_SZ\s+(.*)$/.exec(line);
    if (!match || match[1]?.toLowerCase() !== name.toLowerCase()) continue;
    return (match[2] ?? "").split("\\0").map((value) => value.trim()).filter((value) => value.length > 0);
  }
  return [];
}

function queryRegistry(key: string, name: string): Promise<string[]> {
  return new Promise((resolve) => {
    execFile(systemTool("reg.exe"), ["query", key, "/v", name], { windowsHide: true, timeout: 5_000, encoding: "utf8" }, (error, stdout) => {
      resolve(error ? [] : parseRegMultiSz(stdout, name));
    });
  });
}

/**
 * The Windows UI (display) languages, preferred first, as Windows itself resolves them: the user's
 * choice (`PreferredUILanguages`, present once the user picked a display language), else the
 * system's (`MuiCached\MachinePreferredUILanguages`). Not the regional format, which is what Intl
 * gives. Both are read at once, in the background, by `reg.exe`; [] off Windows or on any failure.
 */
export async function windowsUiLanguages(): Promise<string[]> {
  if (process.platform !== "win32") return [];
  const [user, machine] = await Promise.all([
    queryRegistry("HKCU\\Control Panel\\Desktop", "PreferredUILanguages"),
    queryRegistry("HKCU\\Control Panel\\Desktop\\MuiCached", "MachinePreferredUILanguages"),
  ]);
  return user.length > 0 ? user : machine;
}

/** The interface language by the rule, from this process's `DESKPASS_LANG` and Windows; each server asks once, when it starts, and keeps the answer. */
export function uiLanguage(): Promise<UiLanguage> {
  return decideUiLanguage(process.env[UI_LANGUAGE_ENV], windowsUiLanguages);
}
