# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-08

### Added

- `computer_wait_for` waits for a window instead of for a guessed number of
  seconds: `window_appears` (a listed window whose title contains `text`, case
  ignored; the result has the window's info and `window_id`) and `window_gone`
  (the window `window_id` has closed). `timeout_seconds` is 0.1 to 30, default
  10. It needs no approval and no lock and sends no input; it looks every 300 ms
  after the previous look ended. `window_gone` is met only when Windows says the
  window no longer exists, not by a window that is hidden, on another virtual
  desktop or whose app cannot be identified, and not by a read that failed. A
  success has `met_after_ms`; a timeout is the new error `wait_timeout`, naming
  the condition and the time waited and saying no input was sent. Esc fails it
  with `interrupted` at once. As a `computer_batch` step a timeout stops the
  batch, and the wait never runs past the batch's 60 seconds. `computer_wait` is
  unchanged.
- `computer_wait_for` also waits for an element: `element_appears` and
  `element_gone` take `window_id` and `text` and match like the `query` of
  `computer_get_ui_tree` (name or value, case ignored; a password field is never
  searched). Unlike the window conditions they need the app's approval and the
  operator lock, and a blocked app is refused. The polls keep no element
  references and do not replace the window's latest tree, so the `tree_id` and
  numbers the AI holds stay valid however long the wait; once `element_appears`
  is met, one normal read returns that tree with its `tree_id` (if that read no
  longer finds the element, the wait goes on). A read that was cut short and found
  nothing does not make `element_gone` true, and a wait that only had such reads
  says at the timeout that it could not be confirmed; a match in a cut-short tree
  does make `element_appears` true. A window that closes fails the wait with
  `window_not_found`. The wait counts as lock activity. New helper method
  `find_in_ui_tree`; helper protocol version 12.
- `computer_get_ui_tree` takes two optional parameters to read less of a large
  app. `max_depth` (1 to 30, anything else is `invalid_params`) shows that many
  levels; the children below are counted in `(omitted N children)` and the tree
  ends with `truncated: depth`. `interactive_only` returns only the numbered
  elements and the lines above them; a label with nothing numbered below it is
  left out. The 500-line limit counts the lines that are output, so
  `interactive_only` reaches further down a large app; the 60-children limit and
  the 3-second budget are unchanged. Both work together and with `query`. A tree
  read with either begins with a line saying it is not the whole tree. It has its
  own `tree_id`, its numbers work with `computer_click` and the other element
  tools, and it becomes the window's latest tree. Without them the result is
  exactly what it was, and `computer_screenshot`'s `include_ui_tree` is
  unchanged. Helper protocol version 13.
- `computer_get_ui_tree` can read one part of a window: `root_element_index` (a
  number from the window's latest tree, or from the tree `tree_id` names) starts
  the walk at that element, which is level 0, and returns only what is below it.
  `max_depth` counts from the root (1 is the root alone, with its children counted
  as omitted), and `interactive_only` and `query` work as usual. The window's
  popups (menus, drop-down lists) are not included in this mode. A root that no
  longer exists is refused with the same error any `element_index` tool gives, and
  a `tree_id` without `root_element_index` is `invalid_params`. The subtree has its
  own `tree_id`, its numbers work with `computer_click` and the other element
  tools, and it becomes the window's latest tree. The result begins with a line
  saying which element it is the subtree of (the element's name is quoted and
  escaped like any app text). Without `root_element_index` the result is exactly
  what it was; `computer_screenshot`'s `include_ui_tree` and `find_in_ui_tree` are
  unchanged. Helper protocol version 14.
- `computer_read_text` reads the text of one element of a UI tree through UI
  Automation, so a model need not read it off a screenshot. `scope` is
  `selection` (the text the user has selected, from the element's text pattern
  only; several ranges are joined with line breaks and counted; nothing selected
  is a normal result with empty text) or `document` (the default: the text
  pattern's whole text, else the element's value; an element with neither is
  `not_supported`, with a hint to take a screenshot). `max_chars` is 1 to 20,000
  (the default); text past it is cut on a character, never inside a surrogate
  pair, and the result says `more: true`. The element's password state is read
  from the element itself, not from the tree, before any text is read: a
  password field is `password_field`. It uses the app's approval like a
  screenshot (risk tier, `expected_app`, elevated windows), is cut off by a stop
  (`interrupted`), gives `read_timeout` with no partial text when the app does not
  answer within 3 seconds, and changes nothing of the user's: no input, no
  clipboard, foreground, pointer or selection change. The text is returned in a
  block of its own between begin and end lines carrying a random code, after a
  line that gives the character count and `more` and says it is app content; no
  error holds app text. The action log gets a `text_read` event with the app,
  `scope`, character count and `more`, never the text. It can be a
  `computer_batch` step. Helper protocol version 15.
- `computer_read_text` reads a long text in pieces with `offset` (default 0, in
  characters, that is code points; negative or not a whole number is
  `invalid_params`). Reading in order by `offset`, as the result's note says, until
  `more` is false gives the whole text with nothing lost or repeated, and a piece
  never ends inside a surrogate pair. An offset past the end is a normal result
  with empty text. For a selection, `offset` counts in the ranges' text joined with
  line breaks, and the note counts the ranges of the selection. The text is read
  from its start up to the piece, so that providers that count characters
  differently cannot lose or repeat text: a later piece of a very long document can
  take longer, and over 3 seconds it is `read_timeout` with no partial text. Helper
  protocol version 16.

### Security

- `computer_press_key` refuses key combinations that act beyond the approved app
  until you allow them: anything with the Win key (the Win key alone too),
  `alt+f4`, `ctrl+alt+delete`, `alt+tab`, `ctrl+alt+tab`, `alt+escape`,
  `ctrl+escape`, `ctrl+shift+escape`, `ctrl+alt+escape`, `alt+f8` and `alt+f11`,
  in any order, under any key alias and with more keys held. You are asked in
  Deskpass's own Windows dialog, never through the MCP client, for one combination
  in one app. "Always allow" does not cover it, and the answer is kept in memory
  until the MCP server restarts. A refusal is remembered that long too; no answer,
  or a dialog that could not be shown, is not. Each step of `computer_batch` goes
  through the same check. Log events: `approval` with `subject: "system_key"`, the
  app and the `combination`. Helper protocol version 11.
- `computer_click` and `computer_drag` with `win` in `modifiers` (or its aliases)
  go through the same consent, under the same combination as `computer_press_key`,
  so an answer about `win` in an app holds for all three; a batch step too. A
  refusal sends nothing and brings nothing to the front. A click in the background
  stays `background_unavailable` (`needs_pointer`) with no dialog; only its
  foreground retry asks. `ctrl`, `shift` and `alt` are unchanged.
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
  entries are unchanged. A signed Electron app (VS Code, for one), whose version
  information names Electron whatever its file is called, keeps its identity key
  and its "Always allow". A tier you raised for an app under its signer still
  applies when the app's key changes this way. Helper protocol version 10.
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

- `interactive_only` now drops numbered elements that can only be scrolled into
  view: Chromium gives almost every element that ability, so in Edge it removed
  nothing. It keeps the numbered elements that can do more (an interactive
  control type, or invoke, toggle, a settable value, expand/collapse, selection
  or keyboard focus) and their ancestors. Numbering without it is unchanged.
- **`computer_scroll` now defaults to `delivery_mode: "background"`** (it was
  foreground). A background scroll does not move the cursor or turn the real
  wheel: the helper finds the deepest element of the window's own UI tree at the
  point and goes up to the first element whose ScrollPattern scrolls in that
  direction. It first posts `amount` wheel messages to the window under the point,
  and only when the position did not move (an app such as Edge ignores them, or
  the point is in a popup) calls SmallIncrement or SmallDecrement `amount` times.
  That call makes some apps (Notepad, Word, Edge) come forward for a moment, and
  the foreground is given back; the posted wheel does not. `amount` then counts
  wheel notches or small increments, and one small increment need not be the
  distance of a wheel notch, which the tool description says. The result gives
  the scroll position in percent before and after (`scroll_percent_before`,
  `scroll_percent_after`) and `moved`; an element already at the end is a success
  with `moved: false`. Nothing that scrolls at the point is
  `background_unavailable` with the new reason `no_scroll_pattern`; a minimized
  window keeps `minimized`. The border and stop notice, the wait for the user to
  stop, `foreground_restored`, the stale-screenshot checks and the user's
  "Allow background operation" switch work as for a background click (with the switch
  off a scroll is done in the foreground, with the note). Ask for the old behavior
  with `delivery_mode: "foreground"`, which is unchanged. In a `computer_batch`, a
  scroll step takes the same default. The server waits 15 seconds for a
  scroll, as for a click. Helper protocol version 17.

- Releases are built by a GitHub Actions workflow from the tagged source, not
  on a workstation: it runs the checks, packages the zip, publishes its SHA-256
  and a signed build provenance (`gh attestation verify`), and opens a draft
  release. CI's actions are pinned by commit SHA with read-only permissions,
  the Rust toolchain is pinned (`rust-toolchain.toml`), and Dependabot proposes
  updates.

- When the user has raised an app to a stricter tier, the reason given to the AI
  now says the tier was raised on the settings page, not in the control panel,
  which the settings page replaced.

### Fixed

- The first tree read of a Chromium window (Edge, Chrome, VS Code) no longer
  ends with "This app did not expose elements for its web content" when the
  web content was built while the tree was read.
- A background `computer_scroll` the app took as wheel messages now says it
  scrolled by wheel notches; it said "small increments", which is only what
  the ScrollPattern fallback does.
- `press_key`: shift plus an arrow, Home, End, PageUp, PageDown, Insert or
  Delete now selects text when NumLock is on. These keys were sent without the
  extended-key flag, so Windows read them as numpad keys and, with NumLock on,
  released shift.
- Fewer keys are lost while the user types and Deskpass falls back to bringing
  an app to the front (the 0.1.0 known limitation). Deskpass now attaches to
  the app's input thread only when bringing it forward without attaching
  failed. In a test that types every 30 ms through 6 switches per round, the
  median lost keys fell from about 4.7% to about 1% of those typed; some keys
  can still be lost, so the limitation is narrowed, not gone.
- The 0.1.0 known limitations listed a rare app identity lookup sent to the
  helper while the approval dialog is open. It was a fault in the test, not in
  Deskpass: the lookup belongs to the call that opens the dialog and is sent
  before the dialog, and Deskpass sends nothing to the helper while the dialog
  is open. The test was corrected before 0.1.0 was released, so 0.1.0 does not
  have this limitation.

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
