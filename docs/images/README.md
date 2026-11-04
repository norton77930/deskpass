# Images

## `approval-en.png`

The screenshot at the top of [README.md](../../README.md) and
[README.zh-TW.md](../../README.zh-TW.md). It shows the English UI: the
"Deskpass: Approve app" dialog (buttons Deny, Allow once, Always allow) and,
at the same time, the corner notice at the bottom right of the screen
("AI is operating ‹app› · Ctrl+Alt+Esc to stop") from a background action.

### Capture from outside the Windows session

The approval dialog, the border and the corner notice are excluded from screen
capture on purpose (`WDA_EXCLUDEFROMCAPTURE` in `helper/src/ask.rs` and
`helper/src/overlay.rs`), so that they never appear in the AI's screenshots.
The same exclusion hides them from Print Screen, the Snipping Tool and any
other screenshot tool running in the same session. Capture the screen from
outside it instead, for example by running Deskpass in a virtual machine and
taking the screenshot of the VM window on the host, or by photographing the
monitor. Check the first capture: if the dialog or the notice is missing or
black, the method still goes through the excluded path.

### Steps

1. Install Deskpass from the release zip (or build it with `npm run build`).
   Use a release build: in a debug build the border can be made capturable for
   tests, the dialog cannot.
2. Start your MCP client with the English UI. In PowerShell:

   ```powershell
   $env:DESKPASS_LANG = "en"
   claude
   ```

   Deskpass decides the language once when the server starts, so the variable
   has to be set in the environment the client starts the server from.
3. Make sure Notepad and a second signed app (for example Paint)
   have no "Always allow" grant: on the settings page ("Deskpass Settings" in
   the Start menu) revoke it if they do. A signed app in the normal tier is
   asked through the Deskpass dialog, never the client's own prompt, so the
   dialog appears even in a client that supports elicitation.
4. Open Notepad and ask the agent to operate it with a background action, for
   example "read Notepad's UI tree and click the Edit menu by its element
   number". Approve Notepad with "Allow once". The corner notice now shows
   "AI is operating Notepad · Ctrl+Alt+Esc to stop" and stays for 10 seconds
   after the last action.
5. Within those 10 seconds, ask the agent to operate the second app. When its
   approval dialog appears with the corner notice still showing, capture the
   whole screen. If the notice has gone, have the agent repeat a background
   action on Notepad first and ask for the second app in the same message.
6. Crop to the dialog and the bottom-right corner if the screen is large, keep
   the width at 1600 pixels or less, and save it here as `approval-en.png`.
   Answer the dialog with "Deny" afterwards.
7. Check that the picture shows no personal information: user name, file
   names, other windows' contents, the taskbar's pinned apps.
8. v0.1.0 shipped without the picture. Once it is saved here, put this block
   back under the badges in `README.md`, and the same block with a zh-TW `alt`
   in `README.zh-TW.md`:

   ```html
   <p align="center">
     <img src="docs/images/approval-en.png" width="100%"
       alt="Deskpass with an English Windows UI: the 'Deskpass: Approve app' dialog asks whether the AI may operate an app, with Deny, Allow once and Always allow buttons, while the corner notice at the bottom right reads 'AI is operating Notepad · Ctrl+Alt+Esc to stop'">
   </p>
   ```
