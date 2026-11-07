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

/**
 * Reference-gaps G1: the combinations whose effect reaches beyond the approved app, besides any
 * with the Win key (`judgeSystemKeys`). Written in the normal form of `normalizedCombination`.
 */
export const SYSTEM_KEY_ENTRIES = ["alt+f4", "ctrl+alt+delete", "alt+tab", "ctrl+alt+tab", "alt+escape", "ctrl+escape", "ctrl+shift+escape", "ctrl+alt+escape", "alt+f8", "alt+f11"] as const;

/** The entry of a combination with the Win key and others, which no list could enumerate. */
const ANY_WIN_ENTRY = "win+*";

export type SystemKeyJudgement = { dangerous: true; entry: string } | { dangerous: false; entry: null };

/**
 * Reference-gaps G1: whether keys (canonical names, as `parseChord` gives them) need the user's
 * consent, and the list entry they match. Compared as sets, so order does not matter and holding
 * more keys than an entry still matches it; the longest entry held is the one named.
 */
export function judgeSystemKeys(keys: Iterable<string>): SystemKeyJudgement {
  const held = new Set(keys);
  if (held.has("win")) return { dangerous: true, entry: held.size === 1 ? "win" : ANY_WIN_ENTRY };
  let matched: string | null = null;
  for (const entry of SYSTEM_KEY_ENTRIES) {
    const parts = entry.split("+");
    if (parts.every((part) => held.has(part)) && parts.length > (matched?.split("+").length ?? 0)) matched = entry;
  }
  return matched === null ? { dangerous: false, entry: null } : { dangerous: true, entry: matched };
}

const MODIFIER_ORDER = ["ctrl", "alt", "shift", "win"];

/**
 * Reference-gaps G1: one spelling for one set of canonical key names, the modifiers first in a
 * fixed order and the other keys sorted, so a consent does not depend on how the keys were written.
 * A superset stays a combination of its own.
 */
export function normalizedCombination(keys: Iterable<string>): string {
  const held = new Set(keys);
  const others = [...held].filter((key) => !MODIFIER_ORDER.includes(key)).sort();
  return [...MODIFIER_ORDER.filter((key) => held.has(key)), ...others].join("+");
}
