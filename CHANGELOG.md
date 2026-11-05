# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- `reg.exe` and `rundll32.exe` are started by their full `System32` path. Started
  by their bare names, Windows looked for them in the current directory first,
  which is the folder the MCP client started the server in (usually the user's
  project), so a program of that name in a cloned repository ran when the server
  started.
- `computer_launch_app` starts `.exe` files only. A `.bat` or `.cmd` ran through
  cmd.exe, a blocked-tier terminal, while the approval named only the script.
- `computer_launch_app` no longer looks a relative path up in the current
  directory: `app` is a full path or a name the App Paths registry knows.
- A signed app is known by its signer only when its file name is the original
  name in its version information. A signature covers a file's content but not
  its name, so a copy of cmd.exe saved as notepad.exe verified as Microsoft's
  Notepad and used Notepad's "Always allow"; saved as tool.exe it was a normal
  app. It is now known by its path, and the risk tiers look at both names, so
  any copy of a blocked program is blocked. A signed app whose file name differs
  from its original name, or that has no version information, can now only be
  allowed once; for other apps the identity key and existing "Always allow"
  entries are unchanged. Helper protocol version 10.
- Window titles and app names are quoted and escaped in every tool result
  (`computer_list_windows`, a launch's window, and the messages that name a
  dialog, a covering window or the new foreground), as the UI tree already
  was: a title with line breaks could add lines that read as the tool's own.
  An app name, which is its executable's file description and so chosen by
  whoever built it, is also flattened to one line of at most 100 characters,
  without bidi or zero-width characters, wherever Deskpass shows it, including
  the approval question.
- The MCP SDK is 1.32.2 and zod 4.6.7 (from 1.30 and 3.25), and the locked
  dependencies are updated: `npm audit` reported 3 high and 5 moderate
  advisories, among them zod's memory exhaustion on oversized collections
  (GHSA-mrhx-c7xc-m3w5); it now reports none. Deskpass uses zod only
  through the SDK.
- A background action whose UI Automation call did not answer within 3
  seconds keeps its foreground watch until that call returns (at most 30
  seconds). The call cannot be cancelled, so the app could take the action,
  and the foreground, after the request had been answered, when nothing gave
  the foreground back any more.
- The server waits 15 seconds for `click`, `element_action` and `set_value`
  instead of 10. A background click at a point can take about 11.5 seconds at
  worst, and timing out killed the helper in the middle of the action, with the
  watch that gives the foreground back.

### Changed

- Releases are built by a GitHub Actions workflow from the tagged source, not
  on a workstation: it runs the checks, packages the zip, publishes its SHA-256
  and a signed build provenance (`gh attestation verify`), and opens a draft
  release. CI's actions are pinned by commit SHA with read-only permissions,
  the Rust toolchain is pinned (`rust-toolchain.toml`), and Dependabot proposes
  updates.

## [0.1.0] - 2026-11-04

First public version of Deskpass, a Windows computer use MCP server for Claude
Code and other MCP clients.

The approval dialog, the corner notice, the settings page and the installer are
in English or Traditional Chinese, following `DESKPASS_LANG` (`en` or `zh-TW`)
or else the Windows display language.

### Added

- Per-app approval: the first time the AI wants to operate an app, a dialog
  asks the user. "Allow once" lasts for the session; "Always allow" is offered
  only for signed apps and is remembered.
- Risk tiers: apps are blocked (terminals, the registry editor, Task Manager,
  UAC prompts and Deskpass itself), high (browsers, mail, the file manager,
  password managers; always asked, never "always allow") or normal.
- A signed grants file, so "always allow" entries and raised tiers cannot be
  added or changed by editing the file.
- A settings web page, opened from the Start menu, showing which apps the AI
  can operate and its action log, with controls to block an app or revoke
  "always allow".
- UI-tree-first tools: the AI reads an app's UI Automation tree and acts on
  elements, with screenshot and zoom for what the tree does not cover.
- Background delivery for `click`, `element_action` and `set_value`: the app is
  operated without taking the user's screen or keyboard, the foreground is
  restored right away if the app grabs it, and an action that cannot run in the
  background returns `background_unavailable` instead of silently moving to the
  foreground. Background operation can be switched off in the settings page.
- The AI waits for the user to stop typing before a background action.
- A border and a corner notice while the AI operates an app.
- Stop keys: Ctrl+Alt+Esc always stops the AI; a single Esc stops it while it
  operates in the foreground.
- A batch tool that runs up to 20 steps in order.
- Logs that do not record typed text, clipboard contents or screenshots.
- A zip installer for Windows 10 (2004 or later) and Windows 11 that needs
  Node.js 22 and no build tools.

### Known limitations

- A key is occasionally lost when Deskpass falls back to bringing an app to the
  front and attaching to its input thread; about 1 in 30 runs in testing under
  natural conditions.
- If the window in front is one that cannot take focus (for example the
  on-screen keyboard) and the mouse moves during a background action, the
  window stacking order afterwards may differ from before, and the foreground
  can briefly be empty. The result may still report that the foreground was
  restored.
- Some apps (for example Calculator, Settings and Edge) take the foreground when
  a background action reaches them. Deskpass gives it back right away, but the
  screen can flicker and a key typed in that instant can reach the app.
  Preventing this by disabling the target window during the action is being
  evaluated.
- On setups with several monitors at different display scaling, window
  positions can be off by a pixel or two. If an extra keyboard layout is already
  loaded in the session, typing may not return the foreground app to its
  previous layout; this is under investigation.
- Reading the UI tree right after a background click can occasionally time out
  or miss an element (about 3 in 80 runs in testing), and a dropdown's expanded
  state was once reported wrong. Neither has been seen outside tests.
- While the approval dialog is open, a concurrent call can rarely send an app
  identity lookup to the helper before the answer; no input is sent. It was seen
  once in CI and its cause is being confirmed.
- Windows only; elevated (administrator) windows cannot be operated. Unsigned
  apps are recognised by their path, so moving one resets the choices made for
  it.
