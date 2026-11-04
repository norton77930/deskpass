<h1 align="center">Deskpass</h1>

<h3 align="center">Let coding agents use your Windows desktop, one app at a time.</h3>

<p align="center">
  Deskpass is a computer use MCP server for Windows. Your coding agent can see,<br>
  click and type in desktop apps, and you stay in charge of which ones.
</p>

<table align="center">
  <tr>
    <td align="center" width="200"><b>Every app needs your approval</b><br><sub>a dialog asks before the AI touches an app for the first time</sub></td>
    <td align="center" width="200"><b>Works in the background</b><br><sub>buttons, checkboxes and fields change without taking your focus</sub></td>
    <td align="center" width="200"><b>Ctrl+Alt+Esc stops it</b><br><sub>at any time, from any app</sub></td>
    <td align="center" width="200"><b>Reads the UI tree first</b><br><sub>controls by role, name and state, not only pixels</sub></td>
  </tr>
</table>

<p align="center">
  <a href="https://github.com/norton77930/deskpass/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/norton77930/deskpass/ci.yml?branch=master&style=flat-square&labelColor=16181d&label=CI" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-2ea44f?style=flat-square&labelColor=16181d" alt="MIT license"></a>
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> · <a href="#mcp-client-setup">MCP client setup</a> · <a href="#how-it-works">How it works</a> · <a href="#evidence">Evidence</a> · <a href="README.zh-TW.md">繁體中文</a>
</p>

## Why not screenshot-and-click computer use?

The usual way to let a model use a desktop is to send it a screenshot and let it
click at pixel coordinates with the real mouse and keyboard. It works on
anything that is drawn on screen, and Deskpass keeps it for exactly that. What
it cannot do is tell the model what a control is, or leave the desktop to you
while the model works.

| When… | Screenshot and coordinates | Deskpass |
| :--- | :--- | :--- |
| the target is a small button in a dense window | the model estimates a pixel position on a scaled-down image and can miss | the model reads the window's UI tree and clicks the element by its number |
| the window is resized after the screenshot | coordinates from the old image land somewhere else | an element action does not depend on coordinates; a coordinate from a screenshot taken before the resize is refused (`stale_screenshot`) and nothing is sent |
| the model needs to know a control's state | it judges from pixels whether a box is ticked or a list is open | the tree reports value and states (on, off, expanded, focused, disabled), and actions read the new state back |
| the model fills in a text field | text is typed key by key, at the app's pace, and an input method can compose or hold characters | `computer_set_value` writes the whole value through UI Automation and reads it back |
| you keep working while the AI acts | every click needs the window in front and moves your cursor | button, checkbox, list and field actions run in the background; if the app grabs the foreground, it is given back at once, and an action that cannot run in the background is refused (`background_unavailable`) instead of quietly taking your screen |

The screenshot is still there: `computer_screenshot` and `computer_zoom` cover
what the tree does not, such as web content and canvases.

## Install

You need Windows 10 (2004 or later) or Windows 11, and
[Node.js](https://nodejs.org/) 22 or newer. No administrator rights and no build
tools.

1. Download `deskpass-<version>.zip` from
   [GitHub Releases](https://github.com/norton77930/deskpass/releases).
2. Check it against the SHA-256 in the release notes:

   ```powershell
   Get-FileHash .\deskpass-0.1.0.zip -Algorithm SHA256
   ```

3. Extract the zip, open PowerShell in the extracted folder and run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

The installer copies Deskpass to `%LOCALAPPDATA%\Programs\Deskpass`, registers
it in Claude Code when `claude` is on PATH, creates the Start menu shortcut
"Deskpass Settings", and prints the MCP server definition for your machine and
copies it to the clipboard, for any other client (see
[MCP client setup](#mcp-client-setup)). Then restart your AI client.

**SmartScreen:** the helper and the scripts are not code-signed. `install.ps1`
removes the downloaded-from-the-internet mark from the installed files, so
SmartScreen usually stays quiet; if it does warn, check that the zip matches the
published SHA-256, then choose "More info" and "Run anyway". If your antivirus
or company policy blocks `deskpass-helper.exe`, ask your IT department rather
than turning protection off.

The interface (approval dialog, corner notice, settings page and installer)
follows the Windows display language: Traditional Chinese on zh-TW, zh-HK, zh-MO
or any zh-Hant language, English otherwise. Set `DESKPASS_LANG` to `en` or
`zh-TW` to choose.

To uninstall:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Deskpass\uninstall.ps1"
```

Settings and logs in `%LOCALAPPDATA%\deskpass` are kept unless you add
`-RemoveData`.

## How it works

Deskpass has two processes. A Node.js MCP server holds every policy decision:
approval, risk tiers, the single-operator lock, the stop state, screenshot
scaling and the action log. A Rust helper does the Win32 work it is told to do:
window capture, UI Automation, input, the border and corner notice, the approval
dialog and the stop keys.

- **Per-app approval.** The first time the AI wants an app, you are asked. "Allow
  once" lasts for the session; "Always allow" is offered only for signed apps
  and is kept in a signed grants file. Terminals, the registry editor, Task
  Manager and UAC prompts are blocked; browsers, mail, the file manager and
  password managers are asked every session.
- **UI tree first.** `computer_get_ui_tree` lists a window's controls with
  numbers; `computer_click` with `element_index`, `computer_element_action` and
  `computer_set_value` act on them. Screenshots are scaled to the model's image
  budget and coordinates are mapped back.
- **Background delivery.** Element actions run through UI Automation without
  bringing the window forward. Before each one Deskpass waits until your keyboard
  has been idle for 0.3 seconds (at most 3 seconds); if the app takes the
  foreground anyway, it is handed back to your window right away.
- **Stop keys.** A physical Ctrl+Alt+Esc stops the AI at any time; a single Esc
  stops it while it works in the foreground. Keys the AI sends itself never
  count. After a stop, Deskpass asks you before the AI may continue.
- **Logs without content.** The log records each action and the app it was on,
  never typed text, clipboard contents or screenshots.

Design notes, every tool, and the technical limits are in
[docs/development.md](docs/development.md). The terms are defined in
[CONTEXT.md](CONTEXT.md) and the decisions in [docs/adr/](docs/adr/).

## Supported

- **Windows 10 (2004 or later) and Windows 11, x64.** ARM64 is untested. There is
  no macOS or Linux version.
- **Tested with Claude Code.**
- **CodeBuddy, Cursor, Claude Desktop and Codex CLI:** setup is documented below,
  and the protocol was verified with a generic stdio MCP client that has no
  elicitation, but Deskpass has not been tested inside those clients.
- Any other stdio MCP client should work the same way; it is untested.

## MCP client setup

Deskpass is a stdio MCP server: the command is `node` and the one argument is the
absolute path of the installed `dist\index.js`. `install.ps1` prints the exact
definition for your machine and copies it to the clipboard; paste that. The
examples use the default location, `%LOCALAPPDATA%\Programs\Deskpass\dist\index.js`;
replace `<you>` with your Windows user name. In JSON and TOML every backslash is
written twice.

```json
{
  "mcpServers": {
    "deskpass": {
      "command": "node",
      "args": ["C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js"]
    }
  }
}
```

| Client | Where the configuration goes | What to add |
|---|---|---|
| Claude Code | The `claude` command, run in any folder. `install.ps1` runs it for you when `claude` is on PATH. | `claude mcp add --scope user --transport stdio deskpass -- node "C:\Users\<you>\AppData\Local\Programs\Deskpass\dist\index.js"`, then `claude mcp list` shows `deskpass`. |
| CodeBuddy | CodeBuddy Settings (top right of the chat panel), MCP tab, Add MCP (JSON editor). | The block below. CodeBuddy needs `"type": "stdio"`. |
| Cursor | `%USERPROFILE%\.cursor\mcp.json` for all projects, or `.cursor\mcp.json` inside one project. | The printed definition as it is. |
| Claude Desktop | `%APPDATA%\Claude\claude_desktop_config.json` (Settings, Developer, Edit Config). | The printed definition as it is; if the file already has an `mcpServers` object, add only the `"deskpass": { ... }` entry to it. Restart Claude Desktop. |
| Codex CLI | `%USERPROFILE%\.codex\config.toml` | The TOML below. |

CodeBuddy:

```json
{
  "mcpServers": {
    "deskpass": {
      "type": "stdio",
      "command": "node",
      "args": ["C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js"]
    }
  }
}
```

Codex CLI (`config.toml`):

```toml
[mcp_servers.deskpass]
command = "node"
args = ["C:\\Users\\<you>\\AppData\\Local\\Programs\\Deskpass\\dist\\index.js"]
```

Any other stdio MCP client works the same way: command `node`, one argument (the
absolute path of `dist\index.js`), and any server name you like. After editing a
client's configuration, restart the client.

Known limitation: a client that does not support MCP elicitation gets the Windows
approval dialog instead of an in-client prompt. If that client's tool-call
timeout is shorter than the time you take to answer the dialog, the call may fail
on the client side even though you approved. In that case approve the dialog,
then ask the agent to retry the action; the approval is remembered according to
your settings.

The full user guide in Traditional Chinese (first approval, the settings page,
background operation, emergency stop, SmartScreen, uninstalling) is
[docs/user-guide.zh-TW.md](docs/user-guide.zh-TW.md).

## Evidence

Reproducible numbers only; no speed or success-rate benchmarks.

- **npm tests:** 711 tests, 707 pass and 4 skipped (the end-to-end tests that
  open real windows skip without a desktop), run as CI runs them with
  `DESKPASS_NO_DESKTOP=1`.
- **Rust unit tests:** 109 pass (`cargo test --bins` in `helper/`).
- **Rust desktop tests:** the integration tests in `helper/tests` drive real
  windows, so CI only checks that they compile; they run on a real desktop.
- **CI:** every push to `master` and every pull request runs the TypeScript build,
  the Rust unit tests and the npm tests on `windows-latest`
  ([workflow](.github/workflows/ci.yml)).

## Known limitations

- Windows 10 (2004 or later) and Windows 11 only.
- Windows running as administrator cannot be operated, and Deskpass never
  elevates itself.
- Unsigned apps are recognised by their path: if one moves, or later ships
  signed, the choices you made for it (for example blocking it) no longer apply,
  and the log does not point this out.
- The AI cannot type or paste into password fields and asks you to type instead,
  but a password field the app does not mark as one is not recognised.
- Typing and key presses follow the app's pace, slower than pasting; long text is
  sent in several calls. In some built-in UWP apps (Calculator, Settings)
  Deskpass cannot confirm every key was read, so the AI is asked to check with a
  screenshot.
- A key is occasionally lost when Deskpass falls back to bringing an app to the
  front; about 1 in 30 runs in testing under natural conditions.
- Background operation:
  - Not every app supports it. Web content and apps that draw their own surface
    (for example the Paint canvas) usually have no elements to act on, so the AI
    gets `background_unavailable` and may redo the action in the foreground.
  - A background click is a UI Automation action, not a real mouse click. A few
    apps react only to the real mouse: the action reports success but nothing
    changes. The AI checks with a screenshot.
  - Ticking a checkbox in the background occasionally has no effect in a few
    apps; the result reports the unchanged state, so the AI can redo it.
  - A key you type in the instant between the app grabbing the foreground and
    Deskpass handing it back can reach that app. This is why Deskpass waits for
    you to stop typing first.
  - Occasionally the foreground cannot be handed back: the action is done but
    your window lost focus. The AI is told and does not repeat the action; click
    back into your window.
  - If your front window is one that cannot take focus (such as the on-screen
    keyboard), the window stacking order afterwards may differ from before.
  - A drop-down that the app draws as its own popup can appear on top of your
    window; Deskpass does not move it.

## Tests

You need Node.js 22 or newer and Rust stable with the MSVC toolchain (Visual
Studio C++ build tools).

```powershell
npm ci
npm run build:ts
$env:DESKPASS_NO_DESKTOP = "1"   # skip the end-to-end tests that open real windows, as CI does
npm test
cargo test --manifest-path helper/Cargo.toml --bins
```

Without `DESKPASS_NO_DESKTOP`, `npm test` also runs the end-to-end tests, and
`npm run test:rust` runs the desktop tests in `helper/tests`. Those send real
input to the project's own test windows: keep the screen unlocked, leave the
mouse and keyboard alone, and do not run the two suites at the same time.
[docs/development.md](docs/development.md) has the details.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for building, testing and keeping both
READMEs in step. Release notes are in [CHANGELOG.md](CHANGELOG.md). To report a
security problem, follow [SECURITY.md](SECURITY.md).

## Acknowledgements

- [trycua/cua](https://github.com/trycua/cua): its Windows driver's explicit
  background and foreground delivery modes shaped
  [ADR 0001](docs/adr/0001-uia-actions-background-delivery.md). Deskpass took
  ideas from it; no code was copied.
- [Hallpass](https://github.com/norton77930/hallpass): the sibling project for
  Chrome, whose installer approach Deskpass follows.

## License

[MIT](LICENSE).
