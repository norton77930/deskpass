import { toolError } from "./errors.ts";
import { INTERRUPTED_MESSAGE } from "./helper-client.ts";

/**
 * Ticket 75: what one step (a tool call, later a batch step or a script call) produced, with no
 * MCP types in it. `text` holds the result's text blocks in order: one for most steps, a second
 * for the UI tree a screenshot read with it. `data` holds structured facts for later callers
 * (a batch summary, a script) that the MCP response need not show.
 */
export type StepResult = StepSuccess | StepFailure;

export interface StepSuccess {
  ok: true;
  text: string[];
  images?: StepImage[];
  data?: Record<string, unknown>;
  /** Ticket 138: rendered as the tool result's `structuredContent` (`data` is not rendered). */
  structuredContent?: Record<string, unknown>;
}

export interface StepFailure {
  ok: false;
  code: string;
  message: string;
  /** Whether the step sent input before it failed (`sentInput`). */
  sent: boolean;
  /**
   * Ticket 138: structured facts the model reads beside the message, returned as the result's
   * `structuredContent` (background_unavailable's `reason` and `recommended`). Absent for most failures.
   */
  data?: Record<string, unknown>;
}

export interface StepImage {
  /** Base64 of the image. */
  data: string;
  mimeType: "image/jpeg";
}

/** Ticket 76: the steps of a computer_batch call that started, in order; a step without a result was not run. */
export interface BatchRun {
  steps: BatchStep[];
}

export interface BatchStep {
  action: string;
  result?: StepResult;
}

/** A failure the step's own code produced; the step runner settles `sent` (`sentInput`). */
export type Refusal = Omit<StepFailure, "sent">;

export function refuse(code: string, message: string): Refusal {
  return { ok: false, code, message };
}

export function done(text: string, more: Omit<StepSuccess, "ok" | "text"> = {}): StepSuccess {
  return { ok: true, text: [text], ...more };
}

/**
 * Codes the helper only answers before it sends any input: a refusal of the target (covered, off
 * screen, a dialog open, gone, not operable or unidentified), of the element (an action it does
 * not have, a tree the helper does not keep), or of the request itself.
 */
const REFUSED_BEFORE_INPUT: ReadonlySet<string> = new Set([
  "invalid_params",
  "target_obscured",
  "offscreen",
  "dialog_open",
  "window_not_found",
  "window_not_operable",
  "app_unresolved",
  "not_supported",
  "no_ui_tree",
  // Ticket 138: a background action the helper could not deliver sends nothing.
  "background_unavailable",
]);

/**
 * The rule for `sent`. Until the step's input request reaches the helper nothing was sent, so
 * every failure before it (invalid params, an early rejection, approval denied or unanswered, a
 * stop the gate saw first) is `false`. Once it has reached the helper, only the helper's refusals
 * above are known to precede any input; any other failure (input_failed, interrupted, a timeout or
 * crash, a result that cannot be read) may follow input already sent, so it is `true`, except a
 * stop the helper says came before the input of a method in `STOP_SAYS_IF_SENT`. Ticket 87: a
 * helper error that says whether its input was sent (`inputSent`: the error's `data.input_sent`)
 * decides it instead; an older helper's error without it takes the rule above, which never
 * answers `false` for input that may have been sent. A caller that sees `sent: false` may repeat
 * the step without acting twice.
 */
export function sentInput(code: string, reachedHelper: boolean, inputMethod?: string, message?: string, inputSent?: boolean): boolean {
  if (!reachedHelper) return false;
  if (inputSent !== undefined) return inputSent;
  if (REFUSED_BEFORE_INPUT.has(code)) return false;
  return !(code === "interrupted" && inputMethod !== undefined && STOP_SAYS_IF_SENT.has(inputMethod) && message === INTERRUPTED_MESSAGE);
}

/**
 * Ticket 77: input methods whose one input is marked as an action (`esc::act` in the helper), so a
 * stop after it is answered with the action-taken text and the general stop text
 * (`INTERRUPTED_MESSAGE`, which the client also gives the helper's shorter stop texts) means the
 * input was not sent. paste_text's input is its Ctrl+V; a stop before it leaves the user's
 * clipboard put back and nothing pasted.
 */
const STOP_SAYS_IF_SENT: ReadonlySet<string> = new Set(["paste_text"]);

/**
 * Ticket 76: McpRenderer for a batch. The first text block is the summary (`completed N of M
 * steps`, or `stopped at step K of M: <code>`), then one entry per step: `[i] action: ok <its text>`,
 * `[i] action: <code> (sent: true|false) <its message>`, or `[i] action: not run`. A step's text
 * may run over several lines (a screenshot's metadata, a tree); every entry starts a line with
 * `[i]`. The images of screenshot and zoom steps follow in step order, each after a text block
 * `image of step i (action)`. A batch that stopped is an error whose text begins with the code on
 * its own line, as `toolError` words every tool failure (`code\nmessage`), the summary being the
 * message's first line. Ticket 153: when the failed step carries structured facts (`data`), the
 * result's `structuredContent` is `{ steps: [{ step, action, code, ...data }] }`.
 */
export function renderBatch(run: BatchRun) {
  const total = run.steps.length;
  const failedAt = run.steps.findIndex((step) => step.result?.ok === false);
  const failure = failedAt < 0 ? undefined : (run.steps[failedAt]?.result as StepFailure);
  const completed = run.steps.filter((step) => step.result?.ok === true).length;
  const summary = failure ? `stopped at step ${failedAt + 1} of ${total}: ${failure.code}` : `completed ${completed} of ${total} steps`;
  const entries = run.steps.map((step, index) => `[${index + 1}] ${step.action}: ${batchEntry(step.result)}`);
  const text = [summary, ...entries].join("\n");
  const images = run.steps.flatMap((step, index) =>
    step.result?.ok
      ? (step.result.images ?? []).flatMap((image) => [
          { type: "text" as const, text: `image of step ${index + 1} (${step.action})` },
          { type: "image" as const, data: image.data, mimeType: image.mimeType },
        ])
      : [],
  );
  // Ticket 153: a failed step's structured facts (background_unavailable's `reason` and `recommended`)
  // are the batch's `structuredContent` too, as the same tool alone returns them.
  const failed = failure?.data === undefined ? [] : [{ step: failedAt + 1, action: run.steps[failedAt].action, code: failure.code, ...failure.data }];
  if (failure) {
    return {
      isError: true as const,
      content: [{ type: "text" as const, text: `${failure.code}\n${text}` }, ...images],
      ...(failed.length === 0 ? {} : { structuredContent: { steps: failed } }),
    };
  }
  return { content: [{ type: "text" as const, text }, ...images] };
}

function batchEntry(result: StepResult | undefined): string {
  if (!result) return "not run";
  if (result.ok) return `ok ${result.text.join("\n")}`;
  return `${result.code} (sent: ${result.sent}) ${result.message}`;
}

/**
 * McpRenderer for one tool: the content a tool call returns. A success is its images, then its
 * text blocks; a failure is the code and message, as `toolError` words every tool failure.
 */
export function renderStep(result: StepResult) {
  if (!result.ok) return result.data === undefined ? toolError(result.code, result.message) : { ...toolError(result.code, result.message), structuredContent: result.data };
  return {
    content: [
      ...(result.images ?? []).map((image) => ({ type: "image" as const, data: image.data, mimeType: image.mimeType })),
      ...result.text.map((text) => ({ type: "text" as const, text })),
    ],
    ...(result.structuredContent === undefined ? {} : { structuredContent: result.structuredContent }),
  };
}
