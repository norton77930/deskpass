import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, stat, unlink, writeFile } from "node:fs/promises";
import path from "node:path";

/**
 * Ticket 146: the user's plain settings, in `%LOCALAPPDATA%\deskpass\settings.json` (the data folder
 * beside grants.json), one JSON object. It holds nothing secret and nothing that loosens an approval:
 *
 *     { "allow_background": true }
 *
 * `allow_background` is the settings page's "Allow background operation" switch (spec-background, settings and records):
 * on by default, also while there is no file or the file has no such setting. When it is off the
 * server delivers every action in the foreground. The MCP server reads the file at every tool call
 * (`readAllowBackground`), so a change from the settings page counts at the next call; the settings
 * server writes it (`writeAllowBackground`) by renaming a temporary file over it.
 */

export const SETTINGS_FILE = "settings.json";
/** A settings.json larger than this is not read. */
export const SETTINGS_MAX_BYTES = 64 * 1024;

type SettingsObject = Record<string, unknown>;

/** The file's object; null when there is no file, `undefined` when it cannot be read as one. */
async function readObject(dir: string): Promise<SettingsObject | null | undefined> {
  const file = path.join(dir, SETTINGS_FILE);
  let text: string;
  try {
    if ((await stat(file)).size > SETTINGS_MAX_BYTES) return undefined;
    text = await readFile(file, "utf8");
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "ENOENT" ? null : undefined;
  }
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as SettingsObject) : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Whether background delivery is allowed. No file, or a file without the setting (or with one that
 * is not a boolean), is on. A file that cannot be read gives `lastKnown`, so a damaged file does not
 * turn a switch the user set off back on.
 */
export async function readAllowBackground(dir: string, lastKnown = true): Promise<boolean> {
  const settings = await readObject(dir);
  if (settings === undefined) return lastKnown;
  return typeof settings?.allow_background === "boolean" ? settings.allow_background : true;
}

/**
 * Ticket 153: the switch as the settings page shows it. The MCP server keeps the last value it read
 * while the file cannot be read, a value this process cannot know, so the page is told `null` rather
 * than a state that may be the opposite of what the server applies.
 */
export async function readAllowBackgroundOrUnreadable(dir: string): Promise<boolean | null> {
  const settings = await readObject(dir);
  if (settings === undefined) return null;
  return typeof settings?.allow_background === "boolean" ? settings.allow_background : true;
}

/** Sets the switch, keeping the file's other settings; a file that cannot be read is replaced. */
export async function writeAllowBackground(dir: string, allowed: boolean): Promise<void> {
  const settings = (await readObject(dir)) ?? {};
  settings.allow_background = allowed;
  await mkdir(dir, { recursive: true });
  const file = path.join(dir, SETTINGS_FILE);
  const temporary = path.join(dir, `${SETTINGS_FILE}.${randomUUID()}.tmp`);
  try {
    await writeFile(temporary, `${JSON.stringify(settings, null, 2)}\n`, "utf8");
    await rename(temporary, file);
  } catch (error) {
    await unlink(temporary).catch(() => undefined);
    throw error;
  }
}
