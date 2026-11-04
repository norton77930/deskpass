const ALIASES: Record<string, string> = {
  control: "ctrl",
  ctrl: "ctrl",
  enter: "return",
  return: "return",
  esc: "escape",
  escape: "escape",
  win: "win",
  super: "win",
  meta: "win",
  del: "delete",
  delete: "delete",
  pgup: "pageup",
  pageup: "pageup",
  pgdn: "pagedown",
  pagedown: "pagedown",
};

const NAMED = [
  "backspace",
  "tab",
  "return",
  "escape",
  "space",
  "pageup",
  "pagedown",
  "end",
  "home",
  "left",
  "up",
  "right",
  "down",
  "insert",
  "delete",
  "shift",
  "ctrl",
  "alt",
  "win",
];

export const KEY_NAMES = [
  ...Array.from({ length: 26 }, (_, index) => String.fromCharCode(97 + index)),
  ...Array.from({ length: 10 }, (_, index) => String(index)),
  ...Array.from({ length: 24 }, (_, index) => `f${index + 1}`),
  ...NAMED,
];

const KNOWN = new Set(KEY_NAMES);

export function canonicalKey(name: string): string | null {
  const trimmed = name.trim().toLowerCase();
  const canonical = ALIASES[trimmed] ?? trimmed;
  return KNOWN.has(canonical) ? canonical : null;
}

export function parseChord(raw: string): string[] | null {
  if (typeof raw !== "string" || raw.trim().length === 0) return null;
  const parts = raw.split("+").map((part) => canonicalKey(part));
  if (parts.some((part) => part === null)) return null;
  const keys = parts as string[];
  if (new Set(keys).size !== keys.length) return null;
  return keys;
}

export function keyList(): string {
  return KEY_NAMES.join(", ");
}
