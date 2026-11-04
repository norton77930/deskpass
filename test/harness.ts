import { spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ElicitRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { createServer, type ServerOptions } from "../src/server.ts";
import type { TreeStore } from "../src/trees.ts";
import { HelperClient, type HelperClientOptions } from "../src/helper-client.ts";

/** Open-source ticket 01: any Han, kana, Hangul, CJK punctuation or full-width form (text an English page must not have). */
export const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}　-〿＀-￯]/u;

export interface FakeHandler {
  delayMs?: number;
  result?: unknown;
  error?: { code: string; message: string; data?: Record<string, unknown> };
  events?: { event: string; data?: unknown }[];
  /** Events written right after the response (an error one too). */
  eventsAfter?: { event: string; data?: unknown }[];
  raw?: string;
  stderr?: string;
  exit?: number | true;
}

export interface FakeScript {
  version?: string;
  /** The build id the fake reports from ping; omitted from the reply when unset. */
  build?: string;
  handlers?: Record<string, FakeHandler[]>;
  markerPath?: string;
  logPath?: string;
  /** Where the fake appends the arguments of each start, one JSON array per line. */
  argsPath?: string;
}

export type ElicitationMode = false | "form" | "legacy" | "url";

export interface HarnessOptions extends Omit<ServerOptions, "helperPath" | "helperArgs"> {
  script?: FakeScript;
  /** When omitted, the test client does not declare elicitation. */
  elicitation?: ElicitationMode;
  onElicit?: (message: string) => { action: "accept" | "decline" | "cancel"; content?: Record<string, string> } | Promise<{ action: "accept" | "decline" | "cancel"; content?: Record<string, string> }>;
}

export interface Harness {
  client: Client;
  dir: string;
  elicitations: string[];
  /** The requestedSchema of each elicitation, in order. */
  elicitationSchemas: unknown[];
  /** The server's record of the UI trees it read. */
  trees: TreeStore;
  requests(): Promise<unknown[]>;
  /** Open-source ticket 03: the arguments of each start of the fake helper, in order. */
  spawnArgs(): Promise<string[][]>;
  finish(): Promise<void>;
  close(): Promise<void>;
}

export function textContent(result: { content?: unknown }): string {
  const content = Array.isArray(result.content) ? result.content : [];
  return content
    .filter((block): block is { type: string; text: string } => {
      return typeof block === "object" && block !== null && "type" in block && (block as { type: string }).type === "text" && "text" in block;
    })
    .map((block) => block.text)
    .join("\n");
}

async function prepareScript(script: FakeScript): Promise<{ dir: string; scriptPath: string; logPath: string; argsPath: string }> {
  const root = join(repoRoot(), "test-tmp");
  await mkdir(root, { recursive: true });
  const dir = await mkdtemp(join(root, "case-"));
  const scriptPath = join(dir, "script.json");
  const logPath = join(dir, "requests.log");
  const argsPath = join(dir, "spawn-args.log");
  await writeFile(scriptPath, JSON.stringify({ ...script, logPath, argsPath }));
  return { dir, scriptPath, logPath, argsPath };
}

function fakeHelperPath(): string {
  return fileURLToPath(new URL("./fake-helper.mjs", import.meta.url));
}

function elicitationCapabilities(mode: ElicitationMode | undefined): { elicitation?: { form?: Record<string, never>; url?: Record<string, never> } } {
  if (mode === "form") return { elicitation: { form: {} } };
  if (mode === "url") return { elicitation: { url: {} } };
  if (mode === "legacy") return { elicitation: {} };
  return {};
}

const SPAWN_ATTEMPTS = 5;

/**
 * Ticket 45 (F30): with many test files running at once, Windows now and then refuses to start
 * the fake helper's node process (CreateProcess access denied). Node throws that synchronously as
 * `spawn EPERM`, before any request, so a test fails with `internal` without the server having
 * done anything. This retries that one refusal a few times, briefly pausing, and says so on stderr.
 */
export function retryingSpawn(inner: typeof spawn = spawn, log: (line: string) => void = (line) => process.stderr.write(`${line}\n`)): typeof spawn {
  return ((...args: Parameters<typeof spawn>) => {
    for (let attempt = 1; ; attempt += 1) {
      try {
        return inner(...args);
      } catch (error) {
        if ((error as { code?: unknown }).code !== "EPERM" || attempt >= SPAWN_ATTEMPTS) throw error;
        log(`harness: spawn EPERM starting the fake helper, retry ${attempt} of ${SPAWN_ATTEMPTS - 1}`);
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50 * attempt);
      }
    }
  }) as typeof spawn;
}

export function repoRoot(): string {
  return join(dirname(fileURLToPath(import.meta.url)), "..");
}

export async function launchScriptedHelper(
  script: FakeScript = {},
  options: Partial<HelperClientOptions> = {},
  extraArgs: string[] = [],
): Promise<{ helper: HelperClient; close: () => Promise<void> }> {
  const prepared = await prepareScript(script);
  const helper = new HelperClient({
    helperPath: process.execPath,
    helperArgs: [fakeHelperPath(), "--script", prepared.scriptPath, ...extraArgs],
    serverPid: process.pid,
    spawn: retryingSpawn(),
    ...options,
  });
  return {
    helper,
    async close() {
      await helper.dispose();
      await rm(prepared.dir, { recursive: true, force: true });
    },
  };
}

export async function connectServer(options: HarnessOptions = {}): Promise<Harness> {
  const prepared = await prepareScript(options.script ?? {});
  const { dir, scriptPath, logPath, argsPath } = prepared;
  const fakeHelper = fakeHelperPath();
  const app = createServer({
    ...options,
    lockDir: options.lockDir ?? join(dir, "lock"),
    // Ticket 89: the tool log goes to the case's folder unless a test names one (or passes undefined for the default).
    ...("logDir" in options ? {} : { logDir: join(dir, "logs") }),
    // Ticket 94: grants.json and grants.key are the case's too, never the user's.
    ...("grantsDir" in options ? {} : { grantsDir: dir }),
    // Open-source ticket 01: the case's language is fixed (zh-TW unless given), never the machine's.
    lang: options.lang ?? "zh-TW",
    spawn: options.spawn ?? retryingSpawn(),
    helperPath: process.execPath,
    helperArgs: [fakeHelper, "--script", scriptPath],
  });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const elicitations: string[] = [];
  const elicitationSchemas: unknown[] = [];
  const client = new Client({ name: "deskpass-test", version: "0.0.0" }, { capabilities: elicitationCapabilities(options.elicitation) });
  if (options.elicitation) {
    client.setRequestHandler(ElicitRequestSchema, async (request) => {
      const message = String(request.params.message ?? "");
      elicitations.push(message);
      elicitationSchemas.push((request.params as { requestedSchema?: unknown }).requestedSchema);
      if (options.onElicit) return options.onElicit(message);
      return { action: "accept" as const, content: { decision: "allow" } };
    });
  }
  await Promise.all([app.server.connect(serverTransport), client.connect(clientTransport)]);
  let finished = false;
  async function shutdown() {
    if (finished) return;
    finished = true;
    try {
      await client.close();
    } finally {
      await app.dispose();
    }
  }
  return {
    client,
    dir,
    elicitations,
    elicitationSchemas,
    trees: app.trees,
    async requests() {
      try {
        const raw = await readFile(logPath, "utf8");
        return raw
          .split(/\r?\n/)
          .filter((line) => line.length > 0)
          .map((line) => JSON.parse(line) as unknown);
      } catch {
        return [];
      }
    },
    async spawnArgs() {
      try {
        const raw = await readFile(argsPath, "utf8");
        return raw
          .split(/\r?\n/)
          .filter((line) => line.length > 0)
          .map((line) => JSON.parse(line) as string[]);
      } catch {
        return [];
      }
    },
    async finish() {
      await shutdown();
    },
    async close() {
      await shutdown();
      await rm(dir, { recursive: true, force: true });
    },
  };
}

/** The local calendar day of `time`, as the log's file names have it. */
function logDay(time: number): string {
  const date = new Date(time);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/** Ticket 90: today's `approval` events in the harness's log (its default temp `logs` folder), in order. */
export async function approvalEvents(harness: Harness): Promise<Record<string, unknown>[]> {
  let raw: string;
  try {
    raw = await readFile(join(harness.dir, "logs", `${logDay(Date.now())}.jsonl`), "utf8");
  } catch {
    return [];
  }
  return raw
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => event.event === "approval");
}

/** Ticket 90: each `approval` event's channel and answer, in order. */
export async function approvalAnswers(harness: Harness): Promise<{ channel: unknown; answer: unknown }[]> {
  return (await approvalEvents(harness)).map(({ channel, answer }) => ({ channel, answer }));
}
