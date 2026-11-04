import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { ASK_USER_TIMEOUT_MS, CONFIRM_CHANGE_MARGIN_MS, DEFAULT_REQUEST_TIMEOUT_MS, LAUNCH_APP_TIMEOUT_MS, PASTE_TEXT_TIMEOUT_MS, PROTOCOL_VERSION, TYPE_TEXT_TIMEOUT_MS } from "./config.ts";
import { ToolFailure } from "./errors.ts";
import { buildMismatchMessage, readExpectedBuild } from "./helper-build.ts";
import type { UiLanguage } from "./ui-language.ts";

/** Methods that wait on the user: sent alone, and nothing else is sent until they are answered. */
const USER_METHODS = new Set(["ask_user", "confirm_change"]);

/** Ticket 114: the longest `timeout_ms` the helper takes for `confirm_change`; a request asking for more is refused by it. */
const CONFIRM_CHANGE_MAX_MS = 120_000;

/**
 * Ticket 116: `list_start_menu_apps` lists every Start Menu app and verifies each program's signer;
 * on the development machine that took about 8 seconds (209 apps), so it gets more than the default.
 */
const LIST_START_MENU_APPS_TIMEOUT_MS = 60_000;

export const INTERRUPTED_MESSAGE =
  "The user pressed Esc and stopped Deskpass. Do not call these tools again. End this turn and tell the user.";

/**
 * Ticket 70: what an `interrupted` text from the helper must say for it to reach the model as it
 * is. The helper's texts that tell the model what to do after a stop (its copy of
 * `INTERRUPTED_MESSAGE`, and the one for a stop after an element action was taken) all say to end
 * the turn ("End this turn" / "end this turn"); its shorter ones ("Input stopped before it
 * finished.", "Input is stopped until the server resumes it.") do not, and become
 * `INTERRUPTED_MESSAGE`.
 */
const ENDS_THE_TURN = /end this turn/i;

function interruptedMessage(message: unknown): string {
  return typeof message === "string" && ENDS_THE_TURN.test(message) ? message : INTERRUPTED_MESSAGE;
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

export interface HelperClientOptions {
  helperPath: string;
  helperArgs: string[];
  serverPid: number;
  /**
   * Open-source ticket 03: the interface language the helper shows its dialogs and notices in, passed at every start as
   * `--lang <en|zh-TW>` (a promise is awaited first: the server decides it once, when it is created). The helper never
   * detects a language itself and shows English when it is given none.
   */
  lang?: UiLanguage | Promise<UiLanguage>;
  /**
   * The build id the helper must report from `ping`. Left out, it is read from
   * `helper-build.json` next to the helper exe; null (or no such file) checks only the protocol
   * version.
   */
  expectedBuild?: string | null;
  /** How the expected build is read when `expectedBuild` is left out; for tests. */
  readExpectedBuild?: (helperPath: string) => string | undefined;
  /** Where the server's own diagnostic lines go; stderr by default. */
  log?: (line: string) => void;
  /**
   * `data` carries the event's payload, e.g. the stop `generation` of a `stopped` event;
   * `instance` is the helper process that sent it (see `HelperClient.instance`).
   */
  onEvent?: (event: string, data: Record<string, unknown> | undefined, instance: number) => void;
  /** How the helper process is started; `child_process.spawn` by default. For tests. */
  spawn?: typeof spawn;
}

export class HelperClient {
  private readonly options: HelperClientOptions;
  /** Read once, here: a rebuild after this server started must not change what its helper is held to. */
  private readonly expectedBuild: string | null | undefined;
  private child: ChildProcessWithoutNullStreams | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private starting: Promise<void> | null = null;
  private stderrLines: string[] = [];
  private disposed = false;
  private versionError: ToolFailure | null = null;
  private launches = 0;
  /** Requests on their way to or at the helper, other than a question to the user. */
  private active = new Set<Promise<unknown>>();
  /** Settles when the open question to the user is answered; null while none is. */
  private userTurn: Promise<void> | null = null;

  constructor(options: HelperClientOptions) {
    this.options = options;
    this.expectedBuild =
      options.expectedBuild === undefined
        ? (options.readExpectedBuild ?? ((path) => readExpectedBuild(path, options.log)))(options.helperPath)
        : options.expectedBuild;
  }

  pid(): number | undefined {
    return this.child?.pid;
  }

  /**
   * Which helper process is running: 1 for the first, one more for each restart after a timeout
   * or crash; undefined while none is. State the helper keeps (the Esc hook, the overlay, stop
   * generations) belongs to one instance and is gone once the instance changes.
   */
  instance(): number | undefined {
    const child = this.child;
    return child && child.exitCode === null && child.signalCode === null ? this.launches : undefined;
  }

  async request(method: string, params: Record<string, unknown>): Promise<unknown> {
    if (this.versionError) throw this.versionError;
    if (this.disposed) throw new ToolFailure("helper_crashed", "Helper client is closed.");
    // Nothing is sent while the user is being asked: the helper would refuse it anyway, and a
    // request queued behind the question could time out and kill the helper.
    while (this.userTurn) await this.userTurn;
    if (this.disposed) throw new ToolFailure("helper_crashed", "Helper client is closed.");
    if (USER_METHODS.has(method)) return this.alone(method, params);
    return this.track(this.startAndSend(method, params));
  }

  /** Whether a question to the user is open (or waiting for the requests before it to finish). */
  prompting(): boolean {
    return this.userTurn !== null;
  }

  private async startAndSend(method: string, params: Record<string, unknown>): Promise<unknown> {
    await this.ensureStarted();
    return this.send(method, params);
  }

  private track(work: Promise<unknown>): Promise<unknown> {
    this.active.add(work);
    const forget = () => {
      this.active.delete(work);
    };
    work.then(forget, forget);
    return work;
  }

  /** Sends a question to the user once every earlier request is finished, and nothing else until it is answered. */
  private async alone(method: string, params: Record<string, unknown>): Promise<unknown> {
    let release: () => void = () => {};
    this.userTurn = new Promise<void>((resolve) => {
      release = resolve;
    });
    try {
      while (this.active.size > 0) await Promise.allSettled([...this.active]);
      if (this.disposed) throw new ToolFailure("helper_crashed", "Helper client is closed.");
      return await this.startAndSend(method, params);
    } finally {
      this.userTurn = null;
      release();
    }
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    const child = this.child;
    this.child = null;
    this.starting = null;
    if (!child) return;
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await new Promise<void>((resolve) => {
      if (child.exitCode !== null || child.signalCode !== null) resolve();
      else child.once("exit", () => resolve());
    });
  }

  private timeoutFor(method: string, params: Record<string, unknown>): number {
    if (method === "launch_app") return LAUNCH_APP_TIMEOUT_MS;
    // Ticket 115: the confirm window waits up to its `timeout_ms` for the user, then closes itself.
    if (method === "confirm_change") {
      const asked = typeof params.timeout_ms === "number" && Number.isFinite(params.timeout_ms) ? params.timeout_ms : CONFIRM_CHANGE_MAX_MS;
      return Math.min(Math.max(asked, 0), CONFIRM_CHANGE_MAX_MS) + CONFIRM_CHANGE_MARGIN_MS;
    }
    if (method === "type_text") return TYPE_TEXT_TIMEOUT_MS;
    if (method === "paste_text") return PASTE_TEXT_TIMEOUT_MS;
    if (method === "ask_user") return ASK_USER_TIMEOUT_MS;
    if (method === "list_start_menu_apps") return LIST_START_MENU_APPS_TIMEOUT_MS;
    return DEFAULT_REQUEST_TIMEOUT_MS;
  }

  private async ensureStarted(): Promise<void> {
    if (this.child && this.child.exitCode === null && !this.starting) return;
    if (this.child && this.child.exitCode === null && this.starting) {
      await this.starting;
      return;
    }
    if (!this.starting) this.starting = this.start();
    try {
      await this.starting;
    } catch (error) {
      this.starting = null;
      throw error;
    }
  }

  private async start(): Promise<void> {
    const lang = await this.options.lang;
    const child = (this.options.spawn ?? spawn)(
      this.options.helperPath,
      [...this.options.helperArgs, ...(lang === undefined ? [] : ["--lang", lang]), "--server-pid", String(this.options.serverPid)],
      { stdio: ["pipe", "pipe", "pipe"], windowsHide: true },
    );
    this.child = child;
    this.launches += 1;
    const instance = this.launches;
    this.stderrLines = [];

    let resolveReady: () => void = () => {};
    let rejectReady: (error: Error) => void = () => {};
    let readySettled = false;
    const ready = new Promise<void>((resolve, reject) => {
      resolveReady = resolve;
      rejectReady = reject;
    });
    const settleOk = () => {
      if (readySettled) return;
      readySettled = true;
      resolveReady();
    };
    const settleErr = (error: Error) => {
      if (readySettled) return;
      readySettled = true;
      rejectReady(error);
    };

    child.stderr.setEncoding("utf8");
    let stderrBuffer = "";
    child.stderr.on("data", (chunk: string) => {
      stderrBuffer += chunk;
      const parts = stderrBuffer.split(/\r?\n/);
      stderrBuffer = parts.pop() ?? "";
      for (const line of parts) this.rememberStderr(line);
    });

    // Ticket 127: lines end at "\n" only (a trailing "\r" is dropped). node:readline would also end one at
    // U+2028 or U+2029, which JSON leaves unescaped inside a string. The stream decodes UTF-8 itself, so a
    // character cut between two chunks stays whole.
    child.stdout.setEncoding("utf8");
    let stdoutBuffer = "";
    child.stdout.on("data", (chunk: string) => {
      stdoutBuffer += chunk;
      let end: number;
      while ((end = stdoutBuffer.indexOf("\n")) >= 0) {
        const line = stdoutBuffer.slice(0, end).replace(/\r$/, "");
        stdoutBuffer = stdoutBuffer.slice(end + 1);
        this.onLine(line, settleOk, instance);
      }
    });
    // As readline did, a last line the helper left without its "\n" is still read when the stream ends.
    child.stdout.on("end", () => {
      const line = stdoutBuffer.replace(/\r$/, "");
      stdoutBuffer = "";
      if (line) this.onLine(line, settleOk, instance);
    });

    child.on("error", (error) => {
      settleErr(new ToolFailure("helper_crashed", error.message));
      this.failPending("helper_crashed", error.message);
      this.child = null;
      this.starting = null;
    });
    let exitFinished = false;
    const finishExit = (code: number | null) => {
      if (exitFinished) return;
      exitFinished = true;
      if (stderrBuffer.trim()) this.rememberStderr(stderrBuffer.trim());
      stderrBuffer = "";
      const message = this.crashMessage(code);
      settleErr(new ToolFailure("helper_crashed", message));
      this.failPending("helper_crashed", message);
      if (this.child === child) {
        this.child = null;
        this.starting = null;
      }
    };
    child.on("exit", (code) => {
      const report = () => finishExit(code);
      if (child.stderr.readableEnded) report();
      else {
        child.stderr.once("end", report);
        setTimeout(report, 150);
      }
    });

    const startupTimer = setTimeout(() => {
      settleErr(new ToolFailure("helper_timeout", "Helper did not become ready."));
      this.stopChild(child);
    }, DEFAULT_REQUEST_TIMEOUT_MS);

    try {
      await ready;
    } finally {
      clearTimeout(startupTimer);
    }
    const ping = (await this.send("ping", {})) as { version?: unknown; build?: unknown };
    const reported = typeof ping?.version === "string" ? ping.version : "";
    if (reported !== PROTOCOL_VERSION) {
      this.rejectHelper(
        child,
        `Helper version "${reported}" is not compatible with server protocol version "${PROTOCOL_VERSION}".`,
      );
    }
    const expectedBuild = this.expectedBuild;
    if (expectedBuild) {
      const build = typeof ping?.build === "string" ? ping.build : "";
      if (build !== expectedBuild) {
        this.rejectHelper(child, buildMismatchMessage(build, expectedBuild));
      }
    }
  }

  private rejectHelper(child: ChildProcessWithoutNullStreams, message: string): never {
    const failure = new ToolFailure("helper_version_mismatch", message);
    (this.options.log ?? ((line) => console.error(line)))(`deskpass: ${message}`);
    this.versionError = failure;
    this.stopChild(child);
    throw failure;
  }

  private crashMessage(code: number | null): string {
    const head = `Helper exited (${code ?? "signal"}).`;
    if (this.stderrLines.length === 0) return head;
    return `${head}\n${this.stderrLines.join("\n")}`;
  }

  private stopChild(child: ChildProcessWithoutNullStreams): void {
    if (this.child === child) {
      this.child = null;
      this.starting = null;
    }
    if (child.exitCode === null && child.signalCode === null) child.kill();
  }

  private rememberStderr(line: string): void {
    if (!line) return;
    this.stderrLines.push(line);
    if (this.stderrLines.length > 20) this.stderrLines.shift();
  }

  private failOldest(code: string, message: string): void {
    const oldest = this.pending.keys().next();
    if (oldest.done) return;
    const pending = this.pending.get(oldest.value);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(oldest.value);
    pending.reject(new ToolFailure(code, message));
  }

  private failPending(code: string, message: string): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new ToolFailure(code, message));
    }
    this.pending.clear();
  }

  private onLine(line: string, onReady: () => void, instance: number): void {
    const trimmed = line.trim();
    if (!trimmed) return;
    let message: Record<string, unknown>;
    try {
      message = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      console.error(`helper non-json: ${trimmed}`);
      this.failOldest("helper_protocol", "Helper sent a line that is not JSON.");
      return;
    }
    if (typeof message.event === "string") {
      if (message.event === "ready") onReady();
      if (message.event === "stopped") {
        const data = typeof message.data === "object" && message.data !== null ? (message.data as Record<string, unknown>) : undefined;
        this.options.onEvent?.("stopped", data, instance);
        this.failPending("interrupted", INTERRUPTED_MESSAGE);
      }
      return;
    }
    if (typeof message.id !== "number") return;
    const pending = this.pending.get(message.id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(message.id);
    if (message.ok === true) {
      pending.resolve(message.result);
      return;
    }
    const error = message.error as { code?: unknown; message?: unknown; data?: unknown } | undefined;
    const code = typeof error?.code === "string" ? error.code : "internal";
    if (code === "interrupted") {
      this.options.onEvent?.("stopped", undefined, instance);
      pending.reject(new ToolFailure("interrupted", interruptedMessage(error?.message)));
      return;
    }
    const text = typeof error?.message === "string" ? error.message : "Helper request failed.";
    const data = typeof error?.data === "object" && error.data !== null ? (error.data as Record<string, unknown>) : undefined;
    pending.reject(new ToolFailure(code, text, data));
  }

  private send(method: string, params: Record<string, unknown>): Promise<unknown> {
    const child = this.child;
    if (!child || child.exitCode !== null || !child.stdin.writable) {
      return Promise.reject(new ToolFailure("helper_crashed", "Helper is not running."));
    }
    const id = this.nextId++;
    const timeoutMs = this.timeoutFor(method, params);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.has(id)) return;
        this.pending.delete(id);
        reject(new ToolFailure("helper_timeout", `Helper did not respond within ${timeoutMs}ms.`));
        this.stopChild(child);
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      child.stdin.write(`${JSON.stringify({ id, method, params })}\n`, (error) => {
        if (!error) return;
        if (!this.pending.has(id)) return;
        clearTimeout(timer);
        this.pending.delete(id);
        reject(new ToolFailure("helper_crashed", error.message));
      });
    });
  }
}
