// `npm run package` (ticket 119): turns a checkout into release/deskpass-<version>.zip, the
// install package for colleagues who have Node 22 but no Rust or C++ tools.
//
// The zip holds the server as ONE file, dist/index.js, bundled from src/ by esbuild with
// @modelcontextprotocol/sdk and zod inlined (ticket 134: ~3400 node_modules files made the install
// take minutes), so there is no node_modules and no source map (they would point at src/*.ts, which
// is not shipped). It also holds the release helper and its build id from dist/, a package.json that
// keeps `"type": "module"` (dist/index.js is an ES module), install.ps1, uninstall.ps1, the
// settings launcher, README.md and VERSION. Everything the server runs (the settings server, the
// approval dialog) is inside dist/index.js; nothing else in dist/ is started by path.
//
// It does not build the helper: cargo is slow and needs the Rust and C++ tools. It requires
// dist/deskpass-helper.exe from `npm run build` and fails with a clear message when that is
// missing, does not match dist/helper-build.json, or is older than the helper's sources.
//
// The zip is written by Windows' own tar.exe (bsdtar, `-a` picks zip from the name): it ships
// with Windows 10 and 11, is fast, and writes forward-slash entry names. Compress-Archive is the
// fallback when it is missing.
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

export const PACKAGE_NAME = "deskpass";

/** Files the zip must hold (forward slashes, relative to the zip's root). */
export const REQUIRED_FILES = [
  "dist/index.js",
  "dist/deskpass-helper.exe",
  "dist/helper-build.json",
  "package.json",
  "install.ps1",
  "uninstall.ps1",
  "deskpass-settings.vbs",
  "README.md",
  "VERSION",
];

/** What a development file looks like, and why it does not belong in the zip. */
const FORBIDDEN = [
  { test: (file) => file.startsWith("node_modules/"), why: "in node_modules (the dependencies are bundled into dist/index.js)" },
  { test: (file) => file.endsWith(".map"), why: "a source map" },
  { test: (file) => /^(src|test|helper|\.scratch|scripts|packaging|release|test-tmp)\//.test(file), why: "a repository folder that is not shipped" },
  { test: (file) => /\.ts$/.test(file), why: "TypeScript source" },
  { test: (file) => file === "package-lock.json", why: "the lockfile (nothing is installed from it)" },
  { test: (file) => file.startsWith("dist/") && file.endsWith(".js") && file !== "dist/index.js", why: "a module outside the bundle" },
  { test: (file) => file.startsWith("dist/") && file.endsWith(".exe") && file !== "dist/deskpass-helper.exe", why: "an old helper" },
];

/** Problems with a package's file list (forward slashes, relative to the zip's root); empty when it is fine. */
export function checkPackageFiles(files) {
  const set = new Set(files);
  const problems = REQUIRED_FILES.filter((file) => !set.has(file)).map((file) => `missing ${file}`);
  for (const file of files) {
    const rule = FORBIDDEN.find((candidate) => candidate.test(file));
    if (rule) problems.push(`${file} is ${rule.why}`);
  }
  return problems;
}

/** Every file under `dir`, relative, with forward slashes, sorted. */
export async function listFiles(dir) {
  const entries = await readdir(dir, { recursive: true, withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile())
    .map((entry) => relative(dir, join(entry.parentPath, entry.name)).split(sep).join("/"))
    .sort();
}

/** The package.json the installed program runs with: what Node needs, no scripts and no dev dependencies. */
function runtimePackageJson(packageJson) {
  // No "dependencies": they are inside dist/index.js.
  const keep = ["name", "version", "private", "type", "description", "bin", "engines"];
  return Object.fromEntries(keep.filter((key) => packageJson[key] !== undefined).map((key) => [key, packageJson[key]]));
}

/**
 * Bundles src/index.ts into the single file `outfile`: the SDK and zod are inlined, the output is an
 * ES module (src/config.ts finds the helper through import.meta.url, which an ES module keeps),
 * not minified (stack traces stay readable) and has no source map.
 *
 * The entry is src/, not tsc's output: tsc writes to a temporary folder where the packages cannot
 * be resolved, and esbuild reads the .ts imports directly. `npm run package` type-checks with
 * `tsc --noEmit` first.
 */
export async function bundleServer({ root, outfile }) {
  await build({
    entryPoints: [join(root, "src", "index.ts")],
    outfile,
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node22",
    sourcemap: false,
    minify: false,
    legalComments: "none",
    logLevel: "warning",
  });
}

/**
 * Builds the package's folder at `stageDir` (emptied first) and returns its file list.
 * `bundleServer(outfile)` must write the bundled server to `outfile`.
 */
export async function stagePackage({ bundleServer, helperDir, assetsDir, readme, packageJson, stageDir }) {
  await rm(stageDir, { recursive: true, force: true });
  await mkdir(join(stageDir, "dist"), { recursive: true });

  await writeFile(join(stageDir, "package.json"), `${JSON.stringify(runtimePackageJson(packageJson), null, 2)}\n`);
  await bundleServer(join(stageDir, "dist", "index.js"));
  for (const name of ["deskpass-helper.exe", "helper-build.json"]) {
    await copyFile(join(helperDir, name), join(stageDir, "dist", name));
  }
  for (const name of ["install.ps1", "uninstall.ps1", "deskpass-settings.vbs"]) {
    await copyFile(join(assetsDir, name), join(stageDir, name));
  }
  await copyFile(readme, join(stageDir, "README.md"));
  await writeFile(join(stageDir, "VERSION"), `${packageJson.version}\n`);
  return listFiles(stageDir);
}

/** Windows' bsdtar writes zip with `-a`; a GNU tar earlier on PATH (Git Bash) cannot. */
function windowsTar() {
  const candidate = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
  return existsSync(candidate) ? candidate : null;
}

export function zipDirectory(sourceDir, zipPath) {
  const tar = windowsTar();
  if (tar) {
    execFileSync(tar, ["-a", "-cf", zipPath, "-C", sourceDir, "."], { stdio: "inherit" });
    return;
  }
  execFileSync("powershell", ["-NoProfile", "-Command", `Compress-Archive -Path '${join(sourceDir, "*")}' -DestinationPath '${zipPath}' -Force`], { stdio: "inherit" });
}

/** A zip's file entries, through the same tar.exe, without bsdtar's leading `./` and without folders. */
export function listZip(zipPath) {
  return execFileSync(windowsTar() ?? "tar", ["-tf", zipPath], { encoding: "utf8" })
    .split(/\r?\n/)
    .map((line) => line.replace(/^\.\//, ""))
    .filter((line) => line.length > 0 && line !== "." && !line.endsWith("/"))
    .sort();
}

function newestMtime(path) {
  const stat = statSync(path);
  if (!stat.isDirectory()) return stat.mtimeMs;
  return Math.max(0, ...readdirSync(path).map((name) => newestMtime(join(path, name))));
}

/** Fails unless dist/ holds a release helper that matches its build id and is newer than its sources. */
export function checkReleaseHelper(root) {
  const helper = join(root, "dist", "deskpass-helper.exe");
  const buildFile = join(root, "dist", "helper-build.json");
  const rebuild = "Run `npm run build` (it needs Rust and the C++ tools) and then `npm run package` again.";
  if (!existsSync(helper) || !existsSync(buildFile)) {
    throw new Error(`${helper} or ${buildFile} does not exist. ${rebuild}`);
  }
  const recorded = JSON.parse(readFileSync(buildFile, "utf8")).build;
  const actual = execFileSync(helper, ["--build-id"], { encoding: "utf8", windowsHide: true, timeout: 10_000 }).trim();
  if (recorded !== actual) {
    throw new Error(`${helper} reports build ${actual}, but ${buildFile} records ${recorded}. ${rebuild}`);
  }
  const sources = ["src", "Cargo.toml", "Cargo.lock", "build.rs"].map((name) => join(root, "helper", name)).filter((path) => existsSync(path));
  const newest = Math.max(0, ...sources.map(newestMtime));
  if (newest > statSync(helper).mtimeMs) {
    throw new Error(`${helper} is older than the helper's sources in helper/. ${rebuild}`);
  }
}

function step(title) {
  process.stdout.write(`\n== ${title}\n`);
}

export async function buildPackage(root) {
  const packageJson = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const version = packageJson.version;

  step("check the release helper in dist/");
  checkReleaseHelper(root);

  const work = await mkdtemp(join(tmpdir(), "deskpass-package-"));
  try {
    step("type-check src/");
    // The repo's own TypeScript, wherever npm put it (a worktree resolves it from a parent folder).
    const tsc = createRequire(join(root, "package.json")).resolve("typescript/bin/tsc");
    execFileSync(process.execPath, [tsc, "-p", join(root, "tsconfig.json"), "--noEmit"], { stdio: "inherit" });

    step(`bundle and stage ${PACKAGE_NAME}-${version}`);
    const stageDir = join(work, "stage");
    const files = await stagePackage({
      bundleServer: (outfile) => bundleServer({ root, outfile }),
      helperDir: join(root, "dist"),
      assetsDir: join(root, "packaging"),
      readme: join(root, "README.md"),
      packageJson,
      stageDir,
    });
    const problems = checkPackageFiles(files);
    if (problems.length > 0) throw new Error(`the package is not right:\n  ${problems.join("\n  ")}`);

    step("zip");
    const releaseDir = join(root, "release");
    await mkdir(releaseDir, { recursive: true });
    const zipPath = join(releaseDir, `${PACKAGE_NAME}-${version}.zip`);
    await rm(zipPath, { force: true });
    zipDirectory(stageDir, zipPath);
    const zipped = listZip(zipPath);
    const zipProblems = checkPackageFiles(zipped);
    if (zipProblems.length > 0) throw new Error(`${zipPath} is not right:\n  ${zipProblems.join("\n  ")}`);
    process.stdout.write(`  ${zipPath} (${zipped.length} files, ${statSync(zipPath).size} bytes)\n`);
    return zipPath;
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}

function runAsCommand() {
  if (!process.argv[1]) return false;
  const canonical = (path) => realpathSync(path).toLowerCase();
  try {
    return canonical(fileURLToPath(import.meta.url)) === canonical(resolve(process.argv[1]));
  } catch {
    return false;
  }
}

if (runAsCommand()) {
  try {
    await buildPackage(resolve(dirname(fileURLToPath(import.meta.url)), ".."));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
