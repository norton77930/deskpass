import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import { defaultHelperPath } from "../src/config.ts";

test("the server finds the built helper executable by default", () => {
  const helper = defaultHelperPath();
  assert.equal(existsSync(helper), true, helper);
  assert.match(helper, /deskpass-helper\.exe$/i);
});
