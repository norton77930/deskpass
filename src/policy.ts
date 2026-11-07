import path from "node:path";

/** Ticket 91: an app's risk tier (spec-phase4b, tiers). */
export type Tier = "blocked" | "high" | "normal";

/** Open-source ticket 01: the built-in tier groups, by the stable names the UI string tables key their text by. */
export type BuiltInGroupId = "terminal" | "admin" | "uac" | "credentials" | "deskpass" | "browser" | "explorer" | "settings" | "passwords" | "mail";

/** One group of a built-in tier: why its apps are there, and how they are recognized. */
interface TierGroup {
  /** Completes "in the blocked tier (...)" in a refusal. */
  why: string;
  /**
   * Open-source ticket 01: the group's stable name; its display name and reason for the settings
   * page are `builtInTiers[id]` of each language's table (settings-strings.ts).
   */
  id: BuiltInGroupId;
  /** Executable file names without `.exe`, lower case, compared without regard to case. */
  executables: readonly string[];
  /** UWP package family names, lower case (Name_PublisherId). */
  packageFamilies?: readonly string[];
}

/**
 * Ticket 91: the built-in tiers, in one place; every app on neither list is `normal`. The names
 * are a starting point, not a complete list (spec-phase4b, tiers); the ticket 91 comment says which
 * were confirmed on the development machine. A process of this server or its helper is blocked as
 * well (`tierOf`), whatever its file name. Ticket 111: the one source of the built-in tiers, for
 * the approval check and the settings page alike; `why` is the English text of a refusal, and the
 * group's UI text (name and reason) is in each language's string table under its `id`.
 */
export const BUILT_IN_TIERS: { readonly blocked: readonly TierGroup[]; readonly high: readonly TierGroup[] } = {
  blocked: [
    {
      why: "it is a command shell or terminal, which can run any program",
      id: "terminal",
      executables: ["cmd", "powershell", "pwsh", "powershell_ise", "windowsterminal", "wt", "openconsole", "conhost", "wsl", "bash", "mintty", "alacritty", "wezterm", "wezterm-gui", "hyperkitty", "warp"],
      // Ticket 111: the Store Windows Terminal and its Preview.
      packageFamilies: ["microsoft.windowsterminal_8wekyb3d8bbwe", "microsoft.windowsterminalpreview_8wekyb3d8bbwe"],
    },
    {
      why: "it is a Windows administration tool (the registry, management consoles, running processes)",
      id: "admin",
      executables: ["regedit", "mmc", "taskmgr"],
    },
    { why: "it is the Windows UAC consent prompt, which grants administrator rights", id: "uac", executables: ["consent"] },
    { why: "it is the Windows credential prompt, which takes passwords and PINs", id: "credentials", executables: ["credentialuibroker"] },
    // Ticket 110: computer-use-helper is the helper of an older install still running on the same desktop.
    { why: "it is part of Deskpass itself", id: "deskpass", executables: ["deskpass-helper", "computer-use-helper"] },
  ],
  high: [
    { why: "a web browser", id: "browser", executables: ["msedge", "chrome", "firefox", "brave", "opera", "vivaldi", "iexplore"] },
    { why: "File Explorer", id: "explorer", executables: ["explorer"] },
    {
      why: "Windows Settings or Control Panel",
      id: "settings",
      executables: ["systemsettings", "control"],
      packageFamilies: ["windows.immersivecontrolpanel_cw5n1h2txyewy"],
    },
    { why: "a password manager", id: "passwords", executables: ["1password", "bitwarden", "keepass", "keepassxc", "dashlane", "lastpass"] },
    {
      why: "a mail app",
      id: "mail",
      executables: ["outlook", "olk", "thunderbird"],
      packageFamilies: ["microsoft.windowscommunicationsapps_8wekyb3d8bbwe", "microsoft.outlookforwindows_8wekyb3d8bbwe"],
    },
  ],
};

/** An app's tier and, for a built-in one, why it is there (null for `normal`). */
export interface TierMatch {
  tier: Tier;
  why: string | null;
}

const OWN_PROCESS_WHY = "it is a process of Deskpass itself";

/** Ticket 96: how strict each tier is; a raise counts only when it is stricter than the built-in tier. */
export const TIER_RANK: Readonly<Record<Tier, number>> = { normal: 0, high: 1, blocked: 2 };

/** Completes "in the blocked tier (...)" (or names the tier) for an app the user raised on the settings page. */
export const RAISED_WHY = "the user raised it to this tier on the settings page";

/**
 * Ticket 96: the tier an app is treated in, decided in this one place for the approval check
 * (`authorize`), the launch pre-check (`launchRefusal`) and computer_list_apps' blocked marker:
 * its built-in tier, or the user's raise for its identity key (`raised`, the strictest one there
 * is) when that is stricter. A raise that is not stricter (one asking to lower or keep the built-in
 * tier) changes nothing and is reported as `ignoredRaise`, for the `not_higher` log event.
 */
export function effectiveTier(builtIn: TierMatch, raised: Tier | null): { tier: TierMatch; ignoredRaise: boolean } {
  if (raised === null) return { tier: builtIn, ignoredRaise: false };
  if (TIER_RANK[raised] <= TIER_RANK[builtIn.tier]) return { tier: builtIn, ignoredRaise: true };
  return { tier: { tier: raised, why: RAISED_WHY }, ignoredRaise: false };
}

/**
 * Ticket 96: whether a raise of `key` could be about the executable at `appPath`, judged without
 * its identity: the key is the path's own (an app without a signer or package), the package family
 * of its install folder, or a signer's key for its file name. Only then is the identity looked up
 * to know the app's key, so a launch or a list of apps asks the helper nothing more while no raise
 * names its file. A packaged app outside `WindowsApps` (its family read from its process only) is
 * then met by the window's approval check instead, which always has the identity.
 */
export function mayBeRaised(key: string, appPath: string): boolean {
  if (key === appKey(appPath)) return true;
  const family = packageFamilyFromPath(appPath);
  if (family !== null && key === `package:${family}`) return true;
  const fileName = path.win32.basename(appPath.replaceAll("/", "\\")).toLowerCase();
  return key.startsWith("signer:") && key.slice(key.lastIndexOf("|") + 1) === fileName;
}

export type PolicyDenial = {
  code: "blocked_app" | "elevated_target" | "approval_unavailable" | "approval_declined" | "approval_refused_earlier";
  message: string;
};

export interface ApprovalTarget {
  appPath: string;
  appName: string;
  elevated: boolean;
  helperElevated: boolean;
  pid?: number;
}

/**
 * What this server session remembers about approvals: the apps the user allowed, and (ticket 90)
 * the apps the user refused, which are not asked about again until the server restarts. An
 * unanswered question or one that could not be shown is remembered neither way. Ticket 95: by the
 * app's identity key (`identityKey`), the key its grants have: a signed app's signer and file name,
 * a packaged app's family, the normalized path of any other app (spec-phase4b, asking channel and session state).
 */
export class SessionApprovals {
  private readonly approved = new Set<string>();
  private readonly refused = new Set<string>();

  has(key: string): boolean {
    return this.approved.has(key);
  }

  allow(key: string): void {
    this.approved.add(key);
  }

  /**
   * Ticket 90: whether the user refused the app of `key`, or the app at `appPath`, earlier in this
   * session (ticket 95 review: by its path as well, so a check whose identity lookup failed, which
   * keys the app by its path, still meets a refusal made under its signer's or package's key).
   */
  refusedEarlier(key: string, appPath: string): boolean {
    return this.refused.has(key) || this.refused.has(appKey(appPath));
  }

  /**
   * Ticket 90: remembers the user's explicit refusal of the app of `key` for this session, under its
   * path's key too. Matching a refusal too widely only asks less; allows are kept by `key` alone.
   */
  refuse(key: string, appPath: string): void {
    this.refused.add(key);
    this.refused.add(appKey(appPath));
  }

  /**
   * Ticket 97: reading the clipboard, asked once per server session through the helper's dialog:
   * `allowed` after Allow once, `refused` after the user's refusal (ticket 90's rule), `undefined`
   * until then (an unanswered question or one that could not be shown changes nothing). Kept
   * apart from the apps' keys, so no app key can ever stand for it.
   */
  clipboard: "allowed" | "refused" | undefined;

  /**
   * Reference-gaps G1: the key combinations the user answered about, by app and normalized
   * combination, in this server's memory only: nothing here reaches the grants, and neither an app's
   * approval nor its always-allow stands for them. As for an app, a refusal is kept under the app's
   * path too (`refusedEarlier`) and an allow by `key` alone; an unanswered question changes nothing.
   */
  private readonly systemKeysAllowed = new Set<string>();
  private readonly systemKeysRefused = new Set<string>();

  /** What the user said about `combination` in the app of `key`, at `appPath`; a refusal counts first. */
  systemKey(key: string, appPath: string, combination: string): "allowed" | "refused" | undefined {
    if (this.systemKeysRefused.has(systemKeyId(key, combination)) || this.systemKeysRefused.has(systemKeyId(appKey(appPath), combination))) return "refused";
    return this.systemKeysAllowed.has(systemKeyId(key, combination)) ? "allowed" : undefined;
  }

  allowSystemKey(key: string, combination: string): void {
    this.systemKeysAllowed.add(systemKeyId(key, combination));
  }

  refuseSystemKey(key: string, appPath: string, combination: string): void {
    this.systemKeysRefused.add(systemKeyId(key, combination));
    this.systemKeysRefused.add(systemKeyId(appKey(appPath), combination));
  }
}

function systemKeyId(key: string, combination: string): string {
  return JSON.stringify([key, combination]);
}

export function appKey(appPath: string): string {
  return appPath.replaceAll("/", "\\").toLowerCase();
}

/**
 * The UWP package family of an executable installed under `WindowsApps`: its folder is
 * `Name_Version_Arch_ResourceId_PublisherId` and the family is `Name_PublisherId`; null elsewhere.
 */
export function packageFamilyFromPath(appPath: string): string | null {
  const folder = /\\WindowsApps\\([^\\]+)\\/i.exec(appPath.replaceAll("/", "\\"))?.[1];
  const parts = folder?.split("_");
  if (!parts || parts.length !== 5 || !parts[0] || !parts[4]) return null;
  return `${parts[0]}_${parts[4]}`.toLowerCase();
}

/**
 * Ticket 91: the tier of `appPath`, by executable name (without regard to case) and, for a UWP
 * app, package family name; a process of this server or its helper (`ownPids`) is blocked.
 * `packageFamily` is the one ticket 93's app identity reports; when the identity could not be had,
 * the family is read from the install folder (`packageFamilyFromPath`). `originalName` is the
 * identity's `original_name`: the app is in the strictest tier either of its names is in, so a
 * copy of cmd.exe saved under another name is still blocked. It can only make an app stricter.
 */
export function classifyApp(appPath: string, ownPids: number[], pid?: number, packageFamily?: string, originalName?: string | null): TierMatch {
  if (pid !== undefined && ownPids.includes(pid)) return { tier: "blocked", why: OWN_PROCESS_WHY };
  const stems = [executableStem(appPath), ...(originalName ? [executableStem(originalName)] : [])];
  const match = builtInGroup(stems, [packageFamily ?? packageFamilyFromPath(appPath)]);
  return match ? { tier: match.tier, why: match.group.why } : { tier: "normal", why: null };
}

/** An executable's file name without `.exe`, lower case, as the built-in tiers list it. */
function executableStem(file: string): string {
  return path.win32.basename(file.replaceAll("/", "\\")).replace(/\.exe$/i, "").toLowerCase();
}

/**
 * Ticket 111: the one matching rule of the built-in tiers (`classifyApp`, `builtInTierOf`): the
 * first group, blocked before high, that lists one of `stems` as an executable or one of
 * `families` as a package family (compared without regard to case).
 */
function builtInGroup(stems: string[], families: (string | null | undefined)[]): { tier: "blocked" | "high"; group: TierGroup } | null {
  const known = families.filter((family): family is string => typeof family === "string").map((family) => family.toLowerCase());
  const inGroup = (group: TierGroup) => stems.some((stem) => group.executables.includes(stem)) || known.some((family) => (group.packageFamilies ?? []).includes(family));
  for (const tier of ["blocked", "high"] as const) {
    const group = BUILT_IN_TIERS[tier].find(inGroup);
    if (group) return { tier, group };
  }
  return null;
}

/** Ticket 111: an app's built-in tier and its group, for the settings page (which names the group in its language). */
export interface BuiltInTierInfo {
  tier: "blocked" | "high";
  group: BuiltInGroupId;
  /** The group's English `why`. */
  why: string;
}

/**
 * Ticket 111: the built-in tier of the app of identity key `appId` (`identityKey`: `package:` and
 * its family, `signer:` ending in its file name, or its path), `name` being its executable's path
 * or file name when known ("" otherwise), or null when it is in no built-in tier; matched as
 * `classifyApp` matches the approval check's app (its executable's name, its package family).
 */
export function builtInTierOf(appId: string, name: string): BuiltInTierInfo | null {
  const stems: string[] = [];
  const families: (string | null)[] = [];
  if (appId.startsWith("package:")) families.push(appId.slice("package:".length));
  else if (appId.startsWith("signer:")) stems.push(executableStem(appId.slice(appId.lastIndexOf("|") + 1)));
  else {
    stems.push(executableStem(appId));
    families.push(packageFamilyFromPath(appId));
  }
  if (name.length > 0) {
    stems.push(executableStem(name));
    families.push(packageFamilyFromPath(name));
  }
  const match = builtInGroup(stems, families);
  return match ? { tier: match.tier, group: match.group.id, why: match.group.why } : null;
}

export function tierOf(appPath: string, ownPids: number[], pid?: number): Tier {
  return classifyApp(appPath, ownPids, pid).tier;
}

/** Ticket 91: whether `appPath` is in the blocked tier (it can never be launched or operated). */
export function isBlockedTier(appPath: string, ownPids: number[], pid?: number): boolean {
  return tierOf(appPath, ownPids, pid) === "blocked";
}

/**
 * Ticket 34: why `appPath` cannot be launched although it is not in the blocked tier, or null.
 * ApplicationFrameHost.exe only hosts the windows of UWP apps: approving it would cover none of
 * them (their approval is the app's own path), so it is refused as `blocked_app` before any
 * question. Launch-only: a frame-host window keeps its `app_unresolved` handling.
 */
export function launchBlockedMessage(appPath: string): string | null {
  const stem = path.win32.basename(appPath.replaceAll("/", "\\")).replace(/\.exe$/i, "").toLowerCase();
  if (stem !== "applicationframehost") return null;
  return "ApplicationFrameHost.exe hosts the windows of UWP apps and is not an app itself, so it cannot be launched. Launch the app by its own name or path instead.";
}

export function denialMessage(code: PolicyDenial["code"], appName: string): string {
  switch (code) {
    case "blocked_app":
      return blockedMessage(appName, "it is on the built-in blocked list");
    case "elevated_target":
      return `${appName} is running as administrator and cannot be operated. Report this to the user instead of retrying.`;
    case "approval_unavailable":
      return "The user could not be asked for approval: this client cannot show the question and the Windows approval dialog could not be shown either. The user was not asked, and Deskpass is refused. Report this to the user instead of retrying.";
    case "approval_declined":
      return `The user declined approval for ${appName}. Report this to the user instead of retrying.`;
    case "approval_refused_earlier":
      return `The user refused approval for ${appName} earlier in this session, so this session will not ask again and the app cannot be operated. Restarting the MCP server clears the refusal. Report this to the user instead of retrying.`;
  }
}

/** Ticket 91: the refusal of an app in the blocked tier, saying why it is there. */
export function blockedMessage(appName: string, why: string): string {
  return `${appName} is in the blocked tier (${why}), so it can never be operated or approved. Report this to the user instead of retrying.`;
}

/** The refusal when the user left the approval dialog unanswered until it closed. */
export function unansweredMessage(appName: string, seconds: number): string {
  return `The user did not answer the approval question for ${appName} within ${seconds} seconds, so it was declined. Report this to the user instead of retrying.`;
}

export function supportsFormElicitation(capabilities: { elicitation?: object } | undefined): boolean {
  const elicitation = capabilities?.elicitation as { form?: unknown; url?: unknown } | undefined;
  if (!elicitation) return false;
  if ("form" in elicitation) return true;
  if ("url" in elicitation) return false;
  return true;
}
