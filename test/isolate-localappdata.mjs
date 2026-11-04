// Ticket 100: loaded by `npm test` (node --import) into every test process. Points LOCALAPPDATA at a
// fresh temporary folder so a test that forgets its own log/lock/grants folder cannot write into the
// user's real %LOCALAPPDATA%\deskpass. The real value stays in an env var for the check in
// test/isolation.test.ts.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

if (process.env.COMPUTER_USE_TEST_REAL_LOCALAPPDATA === undefined) {
  process.env.COMPUTER_USE_TEST_REAL_LOCALAPPDATA = process.env.LOCALAPPDATA ?? "";
}

const temp = mkdtempSync(join(tmpdir(), `deskpass-test-localappdata-${process.pid}-`));
process.env.LOCALAPPDATA = temp;

process.on("exit", () => {
  try {
    rmSync(temp, { recursive: true, force: true });
  } catch {
    // best effort
  }
});
