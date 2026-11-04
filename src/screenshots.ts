export interface StoredScreenshot {
  screenshotId: string;
  windowId: string;
  originalWidth: number;
  originalHeight: number;
  imageWidth: number;
  imageHeight: number;
  windowX: number;
  windowY: number;
  /** The window's visible frame (get_window width/height) when this was taken; a resize makes the scale stale. */
  windowWidth: number;
  windowHeight: number;
  /**
   * Window pixel of image pixel (0, 0): zero, or negative when the image also shows the target's
   * own popups (a menu) left of or above the window. Image pixels map to window pixels as
   * origin + image * original / image size.
   */
  originX: number;
  originY: number;
  kind: "full" | "zoom";
}

const MAX_PER_WINDOW = 8;

/** What steps record and find screenshots through: the store, or a batch's view of it (`asOfNow`). */
export type ScreenshotView = Pick<ScreenshotStore, "add" | "get" | "latestFull">;

export class ScreenshotStore {
  private nextId = 1;
  private byWindow = new Map<string, StoredScreenshot[]>();
  private byId = new Map<string, StoredScreenshot>();

  add(shot: Omit<StoredScreenshot, "screenshotId">): StoredScreenshot {
    const stored = { ...shot, screenshotId: `ss_${this.nextId}` };
    this.nextId += 1;
    const existing = this.byWindow.get(shot.windowId) ?? [];
    existing.push(stored);
    const full = existing.filter((item) => item.kind === "full");
    const dropped = full.length > MAX_PER_WINDOW ? full.slice(0, full.length - MAX_PER_WINDOW) : [];
    for (const item of dropped) this.byId.delete(item.screenshotId);
    const keptFull = full.slice(-MAX_PER_WINDOW);
    const zooms = existing.filter((item) => item.kind === "zoom");
    this.byWindow.set(shot.windowId, [...keptFull, ...zooms]);
    this.byId.set(stored.screenshotId, stored);
    return stored;
  }

  get(screenshotId: string): StoredScreenshot | undefined {
    return this.byId.get(screenshotId);
  }

  latestFull(windowId: string): StoredScreenshot | undefined {
    const items = this.byWindow.get(windowId) ?? [];
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const item = items[index];
      if (item?.kind === "full") return item;
    }
    return undefined;
  }

  /**
   * Ticket 76: the store as a batch sees it. Screenshots are recorded and found by id as usual, but
   * each window's latest full screenshot stays the one it is now: a step without screenshot_id
   * keeps to the screenshot the batch began with. The view holds those records (no image data is
   * kept anywhere), so the batch's own screenshots pushing it out of the store do not lose it; a
   * screenshot only goes stale by its window's size, which the record carries.
   */
  asOfNow(): ScreenshotView {
    const latest = new Map<string, StoredScreenshot>();
    for (const windowId of this.byWindow.keys()) {
      const shot = this.latestFull(windowId);
      if (shot) latest.set(windowId, shot);
    }
    return {
      add: (shot) => this.add(shot),
      get: (screenshotId) => this.get(screenshotId),
      latestFull: (windowId) => latest.get(windowId),
    };
  }
}
