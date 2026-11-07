import { refuse, type Refusal } from "./steps.ts";

/**
 * Ticket 138 (spec-background "tool interface"): how a tool's action reaches the target. `background`
 * neither brings the target to the foreground nor changes the z-order or moves the cursor;
 * `foreground` is the behavior before background delivery (activation, real input).
 */
export type DeliveryMode = "background" | "foreground";

export const DELIVERY_MODES: readonly DeliveryMode[] = ["background", "foreground"];

/** The `delivery_mode` parameter as a tool lists it; `fallback` is that tool's default. */
export function deliveryModeSchema(fallback: DeliveryMode) {
  return {
    type: "string",
    enum: [...DELIVERY_MODES],
    description: `How the action reaches the window. Defaults to "${fallback}".`,
  } as const;
}

/**
 * The tool's `delivery_mode` argument: absent is the tool's `fallback`; any value but the two modes
 * is invalid_params, which does not quote the model's text back.
 */
export function readDeliveryMode(value: unknown, fallback: DeliveryMode): DeliveryMode | Refusal {
  if (value === undefined) return fallback;
  if (typeof value === "string" && (DELIVERY_MODES as readonly string[]).includes(value)) return value as DeliveryMode;
  return refuse("invalid_params", `delivery_mode must be "background" or "foreground".`);
}

/**
 * spec-background "`background_unavailable`": why an action could not be delivered in the
 * background. The helper answers the same reasons (`input::BackgroundReason`).
 */
export type BackgroundUnavailableReason = "needs_pointer" | "no_click_pattern" | "no_element_at_point" | "focus_needs_foreground" | "minimized" | "no_scroll_pattern" | "tool_needs_foreground";

export const BACKGROUND_UNAVAILABLE = "background_unavailable";

/**
 * Ticket 146: the line a result ends with when the user turned "Allow background operation" off in the settings
 * page and so an action asked for in the background (or left to the default) was delivered in the foreground.
 */
export const BACKGROUND_OFF_NOTE = "The user turned off background operation, so this action was done in the foreground.";

const RETRY = 'Retry this action with delivery_mode: "foreground".';

const WHY: Record<BackgroundUnavailableReason, string> = {
  needs_pointer: "This click needs the real pointer (a right click, a double click or held modifiers), so it cannot be delivered in the background",
  no_click_pattern: "The element supports none of Invoke, Toggle, SelectionItem or ExpandCollapse, so a click on it cannot be delivered in the background",
  no_element_at_point: "No element that can be acted on was found at that point of the target window, so the click cannot be delivered in the background",
  focus_needs_foreground: "Moving the keyboard focus brings the window forward, so focus cannot be delivered in the background",
  minimized: "The target window is minimized, so the action cannot be delivered in the background",
  no_scroll_pattern: "Nothing at that point of the target window scrolls in that direction, so the scroll cannot be delivered in the background",
  tool_needs_foreground: "This tool needs real keyboard or pointer input, so it cannot be delivered in the background",
};

/** A refused background action: what the model reads and the structured facts it decides by. */
export interface BackgroundUnavailable extends Refusal {
  data: { reason: BackgroundUnavailableReason; recommended: "foreground" };
}

/** `background_unavailable` for `reason`: nothing was sent, the target was not brought forward. */
export function backgroundUnavailable(reason: BackgroundUnavailableReason): BackgroundUnavailable {
  return { ...refuse(BACKGROUND_UNAVAILABLE, `${WHY[reason]}; nothing was sent. ${RETRY}`), data: { reason, recommended: "foreground" } };
}

/** The structured facts of a helper's background_unavailable, kept to the two the model reads. */
export function backgroundUnavailableData(data: Record<string, unknown> | undefined): BackgroundUnavailable["data"] | undefined {
  const reason = data?.reason;
  if (typeof reason !== "string" || !(reason in WHY)) return undefined;
  return { reason: reason as BackgroundUnavailableReason, recommended: "foreground" };
}
