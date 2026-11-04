# Deskpass

Deskpass lets an AI operate apps on a Windows desktop within the scope the user has approved; the user can see what it does and stop it at any time.

## Language

### How actions are delivered

**Background delivery**:
When the AI acts on an app, the user's foreground window, window stacking order and cursor are the same after the action as before it, and no keyboard input is sent to the window the user is working in. If the app grabs the foreground itself, **foreground restore** puts it back immediately, so the user does not notice.
_Avoid_: background mode (acceptable for the feature as a whole, not for the way a single action is delivered), silent operation, hidden operation

**Foreground restore**:
During background delivery, when the target app grabs the foreground itself, Deskpass immediately puts the foreground and the window stacking order back as they were before the action. Only a foreground grabbed by the target app is restored; windows the user switches themselves are left alone.
_Avoid_: refocus, focus steal-back, auto switch-back

**Foreground delivery**:
Bring the target to the foreground first, then send real mouse or keyboard input, and hand the foreground back to the user afterwards.
_Avoid_: normal mode, regular mode, default mode

**Background unavailable** (`background_unavailable`):
The explicit refusal returned when an action cannot be completed by background delivery. The AI decides whether to redo that one action with foreground delivery; Deskpass never switches to the foreground by itself.
_Avoid_: automatic fallback, auto fallback, silent retry
