import { WAIT_FOR_STOP_CHECK_MS } from "./config.ts";

/**
 * Reference-gaps G2: the polling loop of computer_wait_for, with no knowledge of what is read. A
 * condition is a `check` that reads once and says whether it holds; a read that fails for good
 * throws (and so ends the wait), one that failed for the moment is a `pending` with a note.
 */
export type Poll<T> = { met: T } | { pending: string | undefined };

export type PollOutcome<T> =
  | { kind: "met"; value: T; elapsedMs: number }
  /** `note` is what the last read found (or why it failed), when the check said. */
  | { kind: "timeout"; elapsedMs: number; note: string | undefined }
  | { kind: "interrupted" };

export interface PollOptions<T> {
  timeoutMs: number;
  /** The pause from the END of one read to the START of the next. */
  gapMs: number;
  /** Whether input is stopped; looked at before every read and during every pause. */
  interrupted(): boolean;
  check(): Promise<Poll<T>>;
}

const LATE = Symbol("late");

export async function pollUntil<T>(options: PollOptions<T>): Promise<PollOutcome<T>> {
  const started = performance.now();
  const limit = started + options.timeoutMs;
  const elapsed = () => Math.round(performance.now() - started);
  let note: string | undefined;
  for (;;) {
    if (options.interrupted()) return { kind: "interrupted" };
    const left = limit - performance.now();
    if (left <= 0) return { kind: "timeout", elapsedMs: elapsed(), note };
    // A read is not let run past the limit: a hung helper would otherwise hold the wait (and a batch) for its own timeout.
    const read = await within(options.check(), left);
    if (read === LATE) return { kind: "timeout", elapsedMs: elapsed(), note };
    if ("met" in read) return { kind: "met", value: read.met, elapsedMs: elapsed() };
    note = read.pending;
    const next = performance.now() + options.gapMs;
    // No room for another read before the limit: wait out the rest, so the time waited is the timeout asked for.
    const stopped = await sleepUntil(Math.min(next, limit), options.interrupted);
    if (stopped) return { kind: "interrupted" };
    if (next >= limit) return { kind: "timeout", elapsedMs: elapsed(), note };
  }
}

/** `promise`'s outcome, or `LATE` once `ms` have passed; a read that fails after that is no one's concern. */
function within<T>(promise: Promise<T>, ms: number): Promise<T | typeof LATE> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => resolve(LATE), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

/** Sleeps until `target` (a `performance.now()` time) in short slices; true when input was stopped meanwhile. */
async function sleepUntil(target: number, interrupted: () => boolean): Promise<boolean> {
  for (;;) {
    if (interrupted()) return true;
    const left = target - performance.now();
    if (left <= 0) return false;
    await new Promise((resolve) => setTimeout(resolve, Math.min(left, WAIT_FOR_STOP_CHECK_MS)));
  }
}
