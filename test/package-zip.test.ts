import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { checkPackageFiles, listFiles, REQUIRED_FILES, stagePackage } from "../scripts/package.mjs";
import { repoRoot } from "./harness.ts";

// Ticket 119, 134. Producing the real zip needs the release helper, so these tests check the file
// list on a staged fixture; `npm run package` runs the same check on the real stage before it zips.
// The bundle itself is tested in package-bundle.test.ts.

async function workDir(): Promise<string> {
  await mkdir(join(repoRoot(), "test-tmp"), { recursive: true });
  return mkdtemp(join(repoRoot(), "test-tmp", "package-zip-"));
}

async function put(root: string, files: string[]): Promise<void> {
  for (const file of files) {
    await mkdir(dirname(join(root, file)), { recursive: true });
    await writeFile(join(root, file), file);
  }
}

test("a complete package passes the check", () => {
  assert.deepEqual(checkPackageFiles(REQUIRED_FILES), []);
  assert.ok(!REQUIRED_FILES.some((file) => file.startsWith("node_modules/")));
});

test("every required file that is missing is reported", () => {
  const problems = checkPackageFiles(REQUIRED_FILES.filter((file) => file !== "dist/deskpass-helper.exe" && file !== "VERSION"));
  assert.equal(problems.length, 2, problems.join("\n"));
  assert.match(problems.join("\n"), /dist\/deskpass-helper\.exe/);
  assert.match(problems.join("\n"), /VERSION/);
});

test("development files are reported", () => {
  const dev = [
    "node_modules/zod/package.json",
    "node_modules/@modelcontextprotocol/sdk/package.json",
    "node_modules/typescript/package.json",
    "node_modules/@types/node/package.json",
    "dist/index.js.map",
    "src/index.ts",
    "test/package.test.ts",
    "helper/src/main.rs",
    ".scratch/computer-use-mcp/spec.md",
    "package-lock.json",
    "dist/computer-use-helper.exe",
    "dist/deskpass-helper.old-1.exe",
  ];
  const problems = checkPackageFiles([...REQUIRED_FILES, ...dev]);
  for (const file of dev) assert.ok(problems.some((problem) => problem.includes(file)), `${file} not reported:\n${problems.join("\n")}`);
});

test("stagePackage puts exactly the package's files in the stage", async () => {
  const dir = await workDir();
  try {
    const helperDir = join(dir, "dist");
    const assetsDir = join(dir, "packaging");
    const stageDir = join(dir, "stage");
    // The repo's dist/ also holds compiled modules, old builds and source maps: only the helper comes from there.
    await put(helperDir, ["deskpass-helper.exe", "helper-build.json", "index.js", "index.js.map", "panel.js", "computer-use-helper.exe"]);
    await put(assetsDir, ["install.ps1", "uninstall.ps1", "deskpass-settings.vbs"]);
    await put(dir, ["README.md"]);
    const packageJson = {
      name: "deskpass",
      version: "9.8.7",
      type: "module",
      engines: { node: ">=22" },
      scripts: { test: "x" },
      dependencies: { zod: "^3" },
      devDependencies: { typescript: "^5" },
    };

    const files = await stagePackage({
      helperDir,
      assetsDir,
      readme: join(dir, "README.md"),
      packageJson,
      stageDir,
      bundleServer: async (outfile: string) => {
        await mkdir(dirname(outfile), { recursive: true });
        await writeFile(outfile, "bundled");
      },
    });

    assert.deepEqual(files, [...REQUIRED_FILES].sort());
    assert.deepEqual(checkPackageFiles(files), []);
    assert.deepEqual(files, (await listFiles(stageDir)).sort());
    assert.equal(await readFile(join(stageDir, "dist", "index.js"), "utf8"), "bundled");
    assert.equal(await readFile(join(stageDir, "VERSION"), "utf8"), "9.8.7\n");
    const runtime = JSON.parse(await readFile(join(stageDir, "package.json"), "utf8"));
    assert.deepEqual(runtime, { name: "deskpass", version: "9.8.7", type: "module", engines: { node: ">=22" } });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("the install scripts start with a UTF-8 BOM, which Windows PowerShell 5.1 needs for their Chinese text", async () => {
  for (const name of ["install.ps1", "uninstall.ps1"]) {
    const bytes = await readFile(join(repoRoot(), "packaging", name));
    assert.deepEqual([...bytes.subarray(0, 3)], [0xef, 0xbb, 0xbf], name);
  }
});
