# UIA actions use background delivery; when that is impossible, refuse explicitly instead of falling back to the foreground

An earlier design rule required every element action to bring the window to the front and show an overlay, so the user could see what the AI was changing. That made it impossible for the user and the AI to use the computer at the same time, because every action stole the window the user was typing in. We decided that `computer_element_action`, `computer_set_value`, `computer_click` with an `element_index`, and coordinate clicks that resolve to an element through UI Automation (UIA) are completed by **background delivery** by default: no foreground change and no cursor movement. An action UIA cannot perform returns `background_unavailable`, and the AI may choose to redo it with `delivery_mode: "foreground"`; Deskpass never switches to the foreground on its own. "Visible to the user" is now provided by a border drawn only over the visible part of the target and a stop hint in a screen corner. During background delivery the stop key is Ctrl+Alt+Esc, so that pressing Esc in the user's own app does not stop the AI by mistake.

**Status:** accepted (2026-11-02, agreed in a design discussion). **Partly superseded by ADR 0002:** the premise that "UIA actions do not need the foreground" did not hold up under measurement, so background delivery is now implemented with foreground restore; the other decisions stand. This ADR replaces the earlier rule only for background delivery: the rule that element actions always bring the window to the front, and the rule that numbered clicks go through the real mouse while Invoke is used only when the model chooses it explicitly. The rules for foreground delivery are unchanged.

## Considered Options

- **Driver falls back to the foreground automatically (the early `auto` mode in [trycua/cua](https://github.com/trycua/cua))**: rejected. Having the screen taken over unexpectedly is exactly what background delivery exists to avoid, and cua removed that mode for the same reason.
- **Show a dialog asking the user before every switch to the foreground**: rejected. The user has already approved the AI operating that app through per-app approval, and foreground delivery is the behavior that approval already covered. Logging the `delivery_mode` of every action is enough to trace it.
- **Isolate the AI completely in a separate session or VM**: out of scope for this decision. It addresses several AIs operating at once (listed as follow-up work), not a user and an AI sharing one desktop.
- **Copy cua's pen/touch injection and PostMessage clicks**: deferred to a second phase and decided separately. They are new input paths whose reliability has to be measured app by app.

## Consequences

- A background click on an element is a UIA action (Invoke, then Toggle, then SelectionItem.Select, then ExpandCollapse), not a real click. Right-click, double-click, clicks with modifier keys, and apps that only respond to mouse events return `background_unavailable`.
- A target that is covered by other windows can be operated; a minimized target is refused, because it cannot be seen and a screenshot cannot capture it.
- Detecting the target's dialogs can no longer rely on "which window is in the foreground".
