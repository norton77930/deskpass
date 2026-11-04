import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readdir, readFile, realpath, rename, rm, symlink, unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { installHelper, replaceHelper } from "../scripts/copy-helper.mjs";
import { repoRoot } from "./harness.ts";

async function workDir(): Promise<string> {
  await mkdir(join(repoRoot(), "test-tmp"), { recursive: true });
  return mkdtemp(join(repoRoot(), "test-tmp", "copy-helper-"));
}

type HelperFs = Record<"copyFile" | "mkdir" | "readdir" | "rename" | "unlink", (...args: any[]) => Promise<any>>;

function failing(code: string): never {
  throw Object.assign(new Error(`${code}: simulated`), { code });
}

/** A file system where the named operations fail; everything else is the real one. */
function brokenFs(broken: Partial<Record<keyof HelperFs, () => never>>): HelperFs {
  const real: HelperFs = { copyFile, mkdir, readdir, rename, unlink };
  return Object.fromEntries(Object.entries(real).map(([name, fn]) => [name, broken[name as keyof HelperFs] ?? fn])) as unknown as HelperFs;
}

test("copies the helper when nothing holds the destination", async () => {
  const dir = await workDir();
  try {
    const source = join(dir, "source.exe");
    const destination = join(dir, "dist", "deskpass-helper.exe");
    await writeFile(source, "new");
    const result = await replaceHelper({ source, destination });
    assert.equal(result.movedAside, null);
    assert.equal(await readFile(destination, "utf8"), "new");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

// Windows refuses to overwrite the image of a running executable; other systems do not, so the lock this relies on does not exist there.
test("a destination locked by a running process is renamed aside, replaced, and the leftover is removed by the next run", { timeout: 30_000, skip: process.platform !== "win32" }, async () => {
  const dir = await workDir();
  const destination = join(dir, "dist", "deskpass-helper.exe");
  await mkdir(join(dir, "dist"));
  // A real, small, long-running Windows executable stands in for the helper that holds the lock.
  await copyFile(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "ping.exe"), destination);
  const running = spawn(destination, ["-n", "60", "127.0.0.1"], { stdio: "ignore", windowsHide: true });
  const exited = new Promise((resolve) => {
    running.once("exit", resolve);
    running.once("error", resolve);
  });
  try {
    await new Promise((resolve) => setTimeout(resolve, 500));
    const source = join(dir, "source.exe");
    await writeFile(source, "new helper");
    await assert.rejects(copyFile(source, destination), "the destination must really be locked for this test to mean anything");

    const result = await replaceHelper({ source, destination, now: () => 1234 });
    assert.match(result.movedAside ?? "", /deskpass-helper\.old-1234\.exe$/);
    assert.equal(await readFile(destination, "utf8"), "new helper");
    assert.deepEqual((await readdir(join(dir, "dist"))).sort(), ["deskpass-helper.exe", "deskpass-helper.old-1234.exe"]);

    // The old image is still running, so its renamed file cannot be deleted yet: ignored, not fatal.
    const again = await replaceHelper({ source, destination, now: () => 5678 });
    assert.equal(again.movedAside, null);
    assert.ok((await readdir(join(dir, "dist"))).includes("deskpass-helper.old-1234.exe"));

    running.kill();
    await exited;
    await replaceHelper({ source, destination });
    assert.deepEqual(await readdir(join(dir, "dist")), ["deskpass-helper.exe"]);
  } finally {
    // The image stays locked until the process is gone, so removing the directory has to wait for it.
    running.kill();
    await exited;
    await rm(dir, { recursive: true, force: true });
  }
});

test("fails, naming the problem, when the destination can be neither copied over nor renamed", async () => {
  const dir = await workDir();
  try {
    const destination = join(dir, "deskpass-helper.exe");
    await writeFile(destination, "old");
    await assert.rejects(
      replaceHelper({ source: join(dir, "source.exe"), destination, fs: brokenFs({ copyFile: () => failing("EBUSY"), rename: () => failing("EPERM") }) }),
      (error: Error) =>
        /could not replace/i.test(error.message) &&
        error.message.includes(destination) &&
        /EBUSY/.test(error.message) &&
        /EPERM/.test(error.message) &&
        /locked or not writable/.test(error.message),
    );
    assert.equal(await readFile(destination, "utf8"), "old");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("puts the old helper back when the copy fails after it was renamed aside", async () => {
  const dir = await workDir();
  try {
    const destination = join(dir, "deskpass-helper.exe");
    await writeFile(destination, "old");
    const source = join(dir, "source.exe");
    await writeFile(source, "new");
    await assert.rejects(
      replaceHelper({ source, destination, fs: brokenFs({ copyFile: () => failing("EBUSY") }) }),
      /could not replace/i,
    );
    assert.deepEqual(await readdir(dir), ["deskpass-helper.exe", "source.exe"]);
    assert.equal(await readFile(destination, "utf8"), "old");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("leftovers that cannot be deleted are ignored", async () => {
  const dir = await workDir();
  try {
    const destination = join(dir, "deskpass-helper.exe");
    await writeFile(join(dir, "deskpass-helper.old-1.exe"), "stale");
    const source = join(dir, "source.exe");
    await writeFile(source, "new");
    const result = await replaceHelper({ source, destination, fs: brokenFs({ unlink: () => failing("EBUSY") }) });
    assert.equal(result.movedAside, null);
    assert.equal(await readFile(destination, "utf8"), "new");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("the build id is recorded before the helper is replaced, so a failed replacement is still caught by the server", async () => {
  const dir = await workDir();
  try {
    const destination = join(dir, "dist", "deskpass-helper.exe");
    const buildFile = join(dir, "dist", "helper-build.json");
    await mkdir(join(dir, "dist"));
    await writeFile(destination, "old");
    await assert.rejects(
      installHelper({
        source: join(dir, "source.exe"),
        destination,
        buildFile,
        readBuildId: () => "0123456789abcdef",
        fs: brokenFs({ copyFile: () => failing("EBUSY"), rename: () => failing("EPERM") }),
      }),
      /could not replace/i,
    );
    assert.deepEqual(JSON.parse(await readFile(buildFile, "utf8")), { build: "0123456789abcdef" });
    assert.equal(await readFile(destination, "utf8"), "old");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("does not touch dist when the new helper's build id cannot be read", async () => {
  const dir = await workDir();
  try {
    const destination = join(dir, "dist", "deskpass-helper.exe");
    await assert.rejects(
      installHelper({
        source: join(dir, "source.exe"),
        destination,
        buildFile: join(dir, "dist", "helper-build.json"),
        readBuildId: () => {
          throw new Error("no build id");
        },
      }),
      /no build id/,
    );
    await assert.rejects(readdir(join(dir, "dist")));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("installHelper records the build id, then installs the helper", async () => {
  const dir = await workDir();
  try {
    const source = join(dir, "source.exe");
    const destination = join(dir, "dist", "deskpass-helper.exe");
    const buildFile = join(dir, "dist", "helper-build.json");
    await writeFile(source, "new");
    const result = await installHelper({ source, destination, buildFile, readBuildId: (path) => (path === source ? "0123456789abcdef" : "wrong") });
    assert.deepEqual(result, { build: "0123456789abcdef", movedAside: null });
    assert.deepEqual(JSON.parse(await readFile(buildFile, "utf8")), { build: "0123456789abcdef" });
    assert.equal(await readFile(destination, "utf8"), "new");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

/** Runs a copy of the script from `invoked`, where the helper it should install does not exist. */
function runCopyScript(invoked: string): { status: number | null; stderr: string } {
  const run = spawnSync(process.execPath, [invoked], { encoding: "utf8", windowsHide: true });
  return { status: run.status, stderr: run.stderr };
}

test("the script still runs as a command when it is started through a junction", async () => {
  const dir = await workDir();
  try {
    await mkdir(join(dir, "real", "scripts"), { recursive: true });
    await copyFile(join(repoRoot(), "scripts", "copy-helper.mjs"), join(dir, "real", "scripts", "copy-helper.mjs"));
    await symlink(join(dir, "real"), join(dir, "link"), "junction");
    // No helper was built under real/, so running as a command must fail; exiting 0 would be a silent skip.
    const run = runCopyScript(join(dir, "link", "scripts", "copy-helper.mjs"));
    assert.notEqual(run.status, 0, "a build that copied nothing must not report success");
    assert.notEqual(run.stderr.trim(), "");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("the script still runs as a command when the drive letter is written in the other case", { skip: process.platform !== "win32" }, async () => {
  const dir = await workDir();
  try {
    await mkdir(join(dir, "scripts"), { recursive: true });
    await copyFile(join(repoRoot(), "scripts", "copy-helper.mjs"), join(dir, "scripts", "copy-helper.mjs"));
    const real = await realpath(join(dir, "scripts", "copy-helper.mjs"));
    const flipped = (real[0] === real[0].toLowerCase() ? real[0].toUpperCase() : real[0].toLowerCase()) + real.slice(1);
    assert.notEqual(flipped, real);
    const run = runCopyScript(resolve(flipped));
    assert.notEqual(run.status, 0, "a build that copied nothing must not report success");
    assert.notEqual(run.stderr.trim(), "");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
