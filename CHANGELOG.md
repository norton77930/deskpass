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
