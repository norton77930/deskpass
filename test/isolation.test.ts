import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { test } from "node:test";
import { defaultLockDir } from "../src/lock.ts";
import { defaultLogDir } from "../src/log.ts";

// Ticket 100: `npm test` loads test/isolate-localappdata.mjs in every test process, so a test that
// forgets to pass its own log/lock/grants folder lands in a temporary LOCALAPPDATA, never the user's.

const inside = (parent: string, child: string) => {
  const rel = relative(resolve(parent), resolve(child));
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
};

test("the default log, lock and grants folders are under a temporary LOCALAPPDATA, not the real one", () => {
  const real = process.env.COMPUTER_USE_TEST_REAL_LOCALAPPDATA;
  assert.notEqual(real, undefined, "run through `npm test` so test/isolate-localappdata.mjs is loaded");
  const temp = tmpdir();
  // The user's own folder. The temporary folder itself may sit under the real LOCALAPPDATA
  // (Windows keeps TEMP in LOCALAPPDATA\Temp), so only deskpass there is off limits, and the old
  // computer-use-mcp folder (ticket 110) as well.
  const userFolders = real ? [join(real, "deskpass"), join(real, "computer-use-mcp")] : [];
  // The server keeps grants.json and grants.key in the lock folder by default (src/server.ts).
  const folders = { log: defaultLogDir(), lock: defaultLockDir(), grants: defaultLockDir() };
  for (const [name, folder] of Object.entries(folders)) {
    assert.ok(inside(temp, folder), `${name} folder ${folder} is under ${temp}`);
    for (const userFolder of userFolders) {
      assert.ok(!inside(userFolder, folder) && resolve(userFolder) !== resolve(folder), `${name} folder ${folder} is not the user's ${userFolder}`);
    }
  }
  assert.notEqual(process.env.LOCALAPPDATA, real);
});
