import assert from "node:assert/strict";
import { test } from "node:test";
import { identityKey, isOriginalName, readAppIdentity } from "../src/identity.ts";
import { classifyApp } from "../src/policy.ts";

const MICROSOFT = "C=US, S=Washington, L=Redmond, O=Microsoft Corporation, CN=Microsoft Windows";

test("a signed file is known by its signer only when its file name is its original name", () => {
  const key = (fileName: string, original: unknown) => identityKey(`C:\\Somewhere\\${fileName}`, readAppIdentity({ signed: true, signer: MICROSOFT, file_name: fileName, original_name: original }));
  assert.equal(key("notepad.exe", "notepad.exe"), `signer:${MICROSOFT}|notepad.exe`);
  assert.equal(key("Code.exe", "code"), `signer:${MICROSOFT}|code.exe`, "compared without .exe and case");
  // cmd.exe copied as notepad.exe is not Notepad: its path, so Notepad's always-allow does not apply.
  assert.equal(key("notepad.exe", "cmd.exe"), "c:\\somewhere\\notepad.exe");
  assert.equal(key("demo.exe", null), "c:\\somewhere\\demo.exe", "no original name: the path");
  assert.equal(key("demo.exe", undefined), "c:\\somewhere\\demo.exe", "an older helper's answer: the path");
  assert.equal(readAppIdentity({ signed: true, signer: MICROSOFT, file_name: "demo.exe", original_name: "" }), null, "an empty original name is no answer");
  assert.equal(readAppIdentity({ signed: true, signer: MICROSOFT, file_name: "demo.exe", original_name: 5 }), null);
  assert.equal(readAppIdentity({ signed: true, signer: MICROSOFT, file_name: "demo.exe", original_name: "Demo.EXE" })?.original_name, "demo.exe");
  assert.equal(isOriginalName("tool.exe", "cmd.exe"), false);
});

test("a signed Electron app is known by its signer, though its version resource says electron.exe", () => {
  const key = (fileName: string, original: unknown) => identityKey(`C:\\Somewhere\\${fileName}`, readAppIdentity({ signed: true, signer: MICROSOFT, file_name: fileName, original_name: original }));
  // Every Electron app keeps the OriginalFilename of the Electron binary it was built from.
  assert.equal(key("Code.exe", "electron.exe"), `signer:${MICROSOFT}|code.exe`);
  assert.equal(key("Slack.exe", "Electron"), `signer:${MICROSOFT}|slack.exe`, "compared without .exe and case");
  assert.equal(isOriginalName("code.exe", "electron.exe"), true);
  // Only that one name: a name that merely contains it is an ordinary other program.
  assert.equal(key("Code.exe", "electron-helper.exe"), "c:\\somewhere\\code.exe");
  assert.equal(key("Code.exe", "notelectron.exe"), "c:\\somewhere\\code.exe");
  // The exception is about the key only: a blocked name stays blocked whatever its version resource says.
  assert.equal(classifyApp("C:\\Users\\me\\cmd.exe", [], undefined, undefined, "electron.exe").tier, "blocked");
  assert.equal(classifyApp("C:\\Users\\me\\Code.exe", [], undefined, undefined, "electron.exe").tier, "normal");
});

test("an app is in the strictest tier of its file name and its original name", () => {
  assert.equal(classifyApp("C:\\Users\\me\\tool.exe", [], undefined, undefined, "cmd.exe").tier, "blocked");
  assert.equal(classifyApp("C:\\Users\\me\\notepad.exe", [], undefined, undefined, "powershell.exe").tier, "blocked");
  assert.equal(classifyApp("C:\\Users\\me\\viewer.exe", [], undefined, undefined, "msedge.exe").tier, "high");
  // The original name only ever makes an app stricter.
  assert.equal(classifyApp("C:\\Windows\\System32\\cmd.exe", [], undefined, undefined, "notepad.exe").tier, "blocked");
  assert.equal(classifyApp("C:\\Users\\me\\tool.exe", [], undefined, undefined, null).tier, "normal");
});
