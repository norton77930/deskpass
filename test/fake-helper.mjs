// Scripted stand-in for the Rust helper. Speaks the JSON-lines protocol.
import { createHmac } from "node:crypto";
import { appendFileSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import process, { argv, exit, stderr, stdin, stdout } from "node:process";

const args = parseArgs(argv.slice(2));
const script = JSON.parse(readFileSync(args.script, "utf8"));

if (script.markerPath) writeFileSync(script.markerPath, "started");
// Open-source ticket 03: the arguments each start of the helper was given, one JSON line per start.
if (script.argsPath) appendFileSync(script.argsPath, `${JSON.stringify(argv.slice(2))}\n`);

writeLine({ event: "ready" });

if (args.serverPid) {
  const timer = setInterval(() => {
    try {
      process.kill(Number(args.serverPid), 0);
    } catch {
      exit(0);
    }
  }, 500);
  timer.unref();
}

// Lines end at "\n" only, as the real helper reads them: node:readline would also end one at U+2028 or U+2029,
// which JSON leaves unescaped inside a string (ticket 94's odd display name).
let pending = "";
stdin.setEncoding("utf8");
stdin.on("data", (chunk) => {
  pending += chunk;
  let end;
  while ((end = pending.indexOf("\n")) >= 0) {
    const line = pending.slice(0, end).replace(/\r$/, "");
    pending = pending.slice(end + 1);
    void handleLine(line);
  }
});

async function handleLine(line) {
  if (!line.trim()) return;
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    stderr.write("fake-helper: request is not json\n");
    return;
  }
  // `at` is when the request arrived, so tests can tell what was sent while another was answered.
  if (script.logPath) appendFileSync(script.logPath, `${JSON.stringify({ ...request, at: Date.now() })}\n`);
  const queue = script.handlers?.[request.method];
  const scripted = queue && queue.length > 0;
  const step = scripted ? queue.shift() : defaultStep(request.method, request.params ?? {});
  // Saved only when a step was taken: several fakes can share one script (the settings server starts
  // helpers of its own for the Start Menu and for each confirm), and one that took nothing must not put
  // back, from its older copy, a step another one has taken.
  if (scripted) saveScript();
  if (step.delayMs) await delay(step.delayMs);
  if (step.stderr) stderr.write(step.stderr.endsWith("\n") ? step.stderr : `${step.stderr}\n`);
  for (const event of step.events ?? []) writeLine(event);
  if (step.exit !== undefined) {
    await delay(50);
    exit(step.exit === true ? 1 : step.exit);
  }
  if (typeof step.raw === "string") {
    stdout.write(step.raw.endsWith("\n") ? step.raw : `${step.raw}\n`);
    return;
  }
  const response = step.error ? { id: request.id, ok: false, error: step.error } : { id: request.id, ok: true, result: step.result ?? {} };
  // Events the helper sends just after this response, while no request is in flight (as its stop
  // notifier writes `stopped` right after answering the request it interrupted). One write, so the
  // server reads them together: the real helper refuses input after a stop (`main.rs` dispatch),
  // which this fake does not, so a next request sent between two reads would wrongly succeed.
  stdout.write([response, ...(step.eventsAfter ?? [])].map((value) => `${JSON.stringify(value)}\n`).join(""));
}

function defaultStep(method, params) {
  if (method === "grant_mac") return grantMac(params);
  if (method === "ping") return { result: { version: script.version ?? "9", ...(script.build === undefined ? {} : { build: script.build }) } };
  return { result: {} };
}

// A helper the server kills mid-write must not leave a half-written script for its replacement to read,
// so the script is written beside it and renamed over it. A rename refused while a reader has the file open
// is retried, and falls back to the plain write.
function saveScript() {
  const temp = `${args.script}.${process.pid}.tmp`;
  writeFileSync(temp, JSON.stringify(script));
  for (let attempt = 0; attempt < 20; attempt += 1) {
    try {
      renameSync(temp, args.script);
      return;
    } catch {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5);
    }
  }
  writeFileSync(args.script, JSON.stringify(script));
  rmSync(temp, { force: true });
}

// Ticket 94: the helper's `grant_mac` with fixed test keys. The real helper keeps a random key under DPAPI in
// grants.key; this one keeps one of two fixed keys there as hex, so a test can sign entries itself
// (`FAKE_GRANT_KEY` in test/grants.test.ts). A missing file gets the first key (`created`); a file that holds no
// key gets the second (`lost_recreated`), as the real helper makes a new key when the old one cannot be unlocked.
// It checks and canonicalizes each entry as `helper/src/grants.rs` does: exactly the kind's fields, of their types,
// as JSON with the names sorted and no whitespace.
// Test-only HMAC keys, not real keys: fixed values so tests can sign grants.json entries themselves.
const GRANT_KEY = "0f1e2d3c4b5a69788796a5b4c3d2e1f000112233445566778899aabbccddeeff";
const RECREATED_GRANT_KEY = "ffeeddccbbaa99887766554433221100f0e1d2c3b4a5968778695a4b3c2d1e0f";
const isString = (value) => typeof value === "string";
const isCount = (value) => Number.isSafeInteger(value) && value >= 0;
const isMacList = (value) => Array.isArray(value) && value.every(isString);
const MAC_FIELDS = {
  grant: { key: isString, display_name: isString, path: isString, created_at: isString, last_used_at: isString },
  raised: { key: isString, tier: isString, created_at: isString },
  file: { version: isCount, generation: isCount, grant_macs: isMacList, raised_macs: isMacList },
};
// A test case's folders: the OS temp folder and this repository's test-tmp. The fake never touches a grants.key
// elsewhere, such as the user's own under %LOCALAPPDATA%\deskpass, whatever folder a server under test starts it with.
const GRANT_KEY_ROOTS = [tmpdir(), join(dirname(fileURLToPath(import.meta.url)), "..", "test-tmp")].map((root) => resolve(root).toLowerCase() + sep);

function canonicalMacEntry(entry) {
  if (typeof entry !== "object" || entry === null || Object.keys(entry).length !== 2) return null;
  const types = Object.hasOwn(MAC_FIELDS, entry.kind) ? MAC_FIELDS[entry.kind] : undefined;
  const fields = entry.fields;
  if (!types || typeof fields !== "object" || fields === null || Array.isArray(fields)) return null;
  const names = Object.keys(fields);
  if (names.length !== Object.keys(types).length || !names.every((name) => Object.hasOwn(types, name) && types[name](fields[name]))) return null;
  return JSON.stringify(Object.fromEntries(names.sort().map((name) => [name, fields[name]])));
}

// Ticket 95: like the real helper, the key is in the folder the fake was started with (`--grants-dir`); a
// request naming a key file is refused. Without the flag both refuse (ticket 96: Node always passes it), and
// the fake also refuses a folder outside the test folders, so a test can never reach the user's folder.
function grantMac(params) {
  const payloads = Array.isArray(params.entries) ? params.entries.map(canonicalMacEntry) : null;
  if ("key_path" in params || payloads === null || payloads.some((payload) => payload === null)) {
    return { error: { code: "invalid_params", message: "entries are required, and key_path is not a parameter." } };
  }
  if (typeof args.grantsDir !== "string") {
    stderr.write("fake-helper: grant_mac without --grants-dir\n");
    return { error: { code: "internal", message: "The fake helper was started without --grants-dir." } };
  }
  const keyPath = join(args.grantsDir, "grants.key");
  if (!GRANT_KEY_ROOTS.some((root) => resolve(keyPath).toLowerCase().startsWith(root))) {
    stderr.write(`fake-helper: refusing a grants.key outside the test folders: ${keyPath}\n`);
    return { error: { code: "internal", message: "The fake helper only keeps a grants.key under a temp folder." } };
  }
  let stored;
  try {
    stored = readFileSync(keyPath, "utf8");
  } catch {
    stored = null;
  }
  let key = stored;
  let keyState = "ok";
  if (stored === null || !/^[0-9a-f]{64}$/.test(stored)) {
    key = stored === null ? GRANT_KEY : RECREATED_GRANT_KEY;
    keyState = stored === null ? "created" : "lost_recreated";
    mkdirSync(dirname(keyPath), { recursive: true });
    writeFileSync(keyPath, key);
  }
  const macs = payloads.map((payload) => createHmac("sha256", Buffer.from(key, "hex")).update(payload, "utf8").digest("hex"));
  return { result: { macs, key_state: keyState } };
}

function writeLine(value) {
  stdout.write(`${JSON.stringify(value)}\n`);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function parseArgs(parts) {
  const parsed = {};
  for (let i = 0; i < parts.length; i += 1) {
    const part = parts[i];
    if (part === "--script") parsed.script = parts[++i];
    else if (part === "--server-pid") parsed.serverPid = parts[++i];
    else if (part === "--grants-dir") parsed.grantsDir = parts[++i];
    else if (part === "--lang") parsed.lang = parts[++i];
  }
  if (!parsed.script) {
    stderr.write("fake-helper: --script is required\n");
    exit(2);
  }
  return parsed;
}
