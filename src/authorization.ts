import type { ApprovalAnswer, ApprovalChannel } from "./log.ts";
import { isSignedKey } from "./grants.ts";
import { blockedMessage, denialMessage, launchBlockedMessage, type TierMatch } from "./policy.ts";

/**
 * Ticket 91: the authorization decision, in one place. A pure function of the app, its tier and
 * what this server session remembers about it; the server's approval flow (ask, read the window
 * again, approve again when its app changed) and the launch pre-checks call it and act on the
 * result. Tested only through the MCP tools (spec-phase4b, Testing Decisions).
 */

/** The app a decision is about. */
export interface DecisionApp {
  path: string;
  /** The name the refusal messages use. */
  name: string;
  /**
   * Ticket 93's identity key (`identityKey`): a signer's or a package's, or the normalized path of
   * an app with neither. Ticket 95: only the first two can be always allowed.
   */
  key: string;
}

/** What this server session remembers about the app (`SessionApprovals`, by identity key). */
export interface SessionState {
  allowed: boolean;
  refused: boolean;
}

/** Nothing remembered: what a launch pre-check decides on, before the approval proper. */
export const NOTHING_REMEMBERED: SessionState = { allowed: false, refused: false };

/**
 * The facts of a window that come before the tier. A window whose app could not be identified
 * (ticket 34's `app_unresolved`) never reaches the decision: the server refuses it first.
 */
export interface DecisionWindow {
  elevated: boolean;
  helperElevated: boolean;
}

export interface DecisionInput {
  app: DecisionApp;
  /** The app's tier and why (`classifyApp`). */
  tier: TierMatch;
  session: SessionState;
  /** The window being operated, when there is one. */
  window?: DecisionWindow;
  /** A launch: ApplicationFrameHost.exe cannot be launched (ticket 34). */
  launch?: boolean;
  /**
   * Ticket 94: the app's identity has an always-allow grant that counts (verified, a signed app's,
   * not raised by the user; `GrantView.granted`). Used for a normal-tier app only.
   */
  granted?: boolean;
}

/** The approval event a decision made without asking writes, or null for none. */
export type DecisionLog = { channel: ApprovalChannel | null; answer: ApprovalAnswer } | null;

export type RefusalCode = "elevated_target" | "blocked_app" | "approval_refused_earlier";

/**
 * Whether the app can be always allowed, and if not, why. Ticket 95: a normal app with a signer's
 * or a package's identity can; a high-risk one never can, nor can an app without such an identity
 * (unsigned, a signature that does not verify, or no identity to be had).
 */
export type AlwaysAllow = { available: true } | { available: false; because: "high_risk_tier" | "no_app_identity" };

export type Decision =
  | { kind: "allow"; log: DecisionLog; alwaysAllow: AlwaysAllow }
  | {
      kind: "ask";
      /**
       * `elicitation_or_dialog`: the client's form elicitation when it declares it, otherwise the
       * helper's dialog. Ticket 95: `dialog`, the helper's dialog and nothing else, for an app that
       * can be always allowed, so its third button is seen and no channel the model can reach
       * answers for it; when the dialog cannot be shown, the user is not asked.
       */
      channel: "elicitation_or_dialog" | "dialog";
      buttons: readonly ("refuse" | "allow_once" | "allow_always")[];
      alwaysAllow: AlwaysAllow;
    }
  | { kind: "refuse"; code: RefusalCode; reason: string; log: DecisionLog };

/**
 * The rules in order: the window is elevated and the helper is not, the launch is of the frame
 * host; then the tier; then the session's memory: a refusal stands; then a normal app's
 * always-allow grant (ticket 94); then an allow this session gave; otherwise the user is asked.
 */
export function decide({ app, tier, session, window, launch = false, granted = false }: DecisionInput): Decision {
  if (window && window.elevated && !window.helperElevated) {
    return { kind: "refuse", code: "elevated_target", reason: denialMessage("elevated_target", app.name), log: null };
  }
  if (launch) {
    const notLaunchable = launchBlockedMessage(app.path);
    if (notLaunchable) return { kind: "refuse", code: "blocked_app", reason: notLaunchable, log: { channel: null, answer: "blocked" } };
  }
  if (tier.tier === "blocked") {
    const reason = tier.why === null ? denialMessage("blocked_app", app.name) : blockedMessage(app.name, tier.why);
    return { kind: "refuse", code: "blocked_app", reason, log: { channel: null, answer: "blocked" } };
  }
  // Ticket 90: a refusal the user gave in this session stands; the user is not asked again.
  if (session.refused) {
    return {
      kind: "refuse",
      code: "approval_refused_earlier",
      reason: denialMessage("approval_refused_earlier", app.name),
      log: { channel: "session", answer: "refused_earlier" },
    };
  }
  // A high-risk app is allowed for this server session at most; so is an app without a signer's or a package's identity.
  const alwaysAllow: AlwaysAllow =
    tier.tier === "high" ? { available: false, because: "high_risk_tier" } : isSignedKey(app.key) ? { available: true } : { available: false, because: "no_app_identity" };
  // Ticket 94: a normal app's always-allow grant allows it without asking; a high-risk app's never does.
  if (granted && tier.tier === "normal") return { kind: "allow", log: { channel: "grant", answer: "allow_always" }, alwaysAllow };
  if (session.allowed) return { kind: "allow", log: { channel: "session", answer: "allow_once" }, alwaysAllow };
  if (alwaysAllow.available) return { kind: "ask", channel: "dialog", buttons: ["refuse", "allow_once", "allow_always"], alwaysAllow };
  return { kind: "ask", channel: "elicitation_or_dialog", buttons: ["refuse", "allow_once"], alwaysAllow };
}
