/**
 * Ticket 95: the always-allow answer of the helper's approval dialog, as the one value
 * `GrantStore.addGrant` takes. Only `readDialogAnswer` makes one, from the helper's `ask_user`
 * answer `allow_always` to a question that offered the button (`allow_always: true`); the
 * elicitation path, the tools and every other module have no way to construct it (its constructor
 * wants a symbol this module keeps), and `addGrant` takes each one once (`takeAlwaysAllow`). So no
 * code path but the user pressing Always allow in the dialog can add a grant (spec-phase4b, grants file and
 * tamper protection: a new always-allow is written only by the helper's dialog). It does not stop code in this process that sets out
 * to sign entries through `grant_mac` itself; that boundary is the README's.
 */

const DIALOG = Symbol("approval dialog");
/** The answers made and not yet taken. */
const unspent = new WeakSet<AlwaysAllowAnswer>();

/** What the grant is for: the app's identity key, its name and path as the dialog showed them. */
export interface AlwaysAllowFor {
  key: string;
  displayName: string;
  path: string;
}

/** The user's Always allow for one app, from the dialog. */
export class AlwaysAllowAnswer {
  readonly key: string;
  readonly displayName: string;
  readonly path: string;

  constructor(seal: typeof DIALOG, app: AlwaysAllowFor) {
    if (seal !== DIALOG) throw new Error("An always-allow answer comes from the approval dialog only.");
    this.key = app.key;
    this.displayName = app.displayName;
    this.path = app.path;
    unspent.add(this);
  }
}

/** The dialog's answers the server reads; `allow` is Allow once. */
export type DialogAnswer = "allow" | "deny" | "timeout";

/**
 * The helper's `ask_user` answer to an `approve_app` question: one of the plain answers, the
 * always-allow (only when `offered` was the app the button was shown for), or null when the answer
 * is none of these (an `allow_always` the question did not offer included), which the server
 * treats as no usable answer.
 */
export function readDialogAnswer(answer: unknown, offered: AlwaysAllowFor | null): DialogAnswer | AlwaysAllowAnswer | null {
  if (answer === "allow" || answer === "deny" || answer === "timeout") return answer;
  if (answer === "allow_always" && offered !== null) return new AlwaysAllowAnswer(DIALOG, offered);
  return null;
}

/** The app of an answer not yet taken, and the answer spent; null for anything else. */
export function takeAlwaysAllow(answer: unknown): AlwaysAllowFor | null {
  if (!(answer instanceof AlwaysAllowAnswer) || !unspent.has(answer)) return null;
  unspent.delete(answer);
  return { key: answer.key, displayName: answer.displayName, path: answer.path };
}
