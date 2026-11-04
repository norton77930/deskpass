export interface PixelSize {
  width: number;
  height: number;
}

const TILE = 28;
const MAX_TILES = 1568;
const MAX_EDGE = 1568;

export function withinBudget(width: number, height: number): boolean {
  if (width < 1 || height < 1) return false;
  return Math.max(width, height) <= MAX_EDGE && Math.ceil(width / TILE) * Math.ceil(height / TILE) <= MAX_TILES;
}

/** Largest size that keeps the aspect ratio and stays inside the image budget. Never upscales. */
export function targetSize(width: number, height: number): PixelSize {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width < 1 || height < 1) {
    throw new Error("Screenshot size must be a positive number.");
  }
  const sourceWidth = Math.floor(width);
  const sourceHeight = Math.floor(height);
  if (withinBudget(sourceWidth, sourceHeight)) return { width: sourceWidth, height: sourceHeight };

  const longIsWidth = sourceWidth >= sourceHeight;
  const longSide = longIsWidth ? sourceWidth : sourceHeight;
  const shortSide = longIsWidth ? sourceHeight : sourceWidth;
  const limit = Math.min(longSide, MAX_EDGE);
  for (let nextLong = limit; nextLong >= 1; nextLong -= 1) {
    const nextShort = Math.max(1, Math.floor((shortSide * nextLong) / longSide));
    const nextWidth = longIsWidth ? nextLong : nextShort;
    const nextHeight = longIsWidth ? nextShort : nextLong;
    if (nextWidth <= sourceWidth && nextHeight <= sourceHeight && withinBudget(nextWidth, nextHeight)) {
      return { width: nextWidth, height: nextHeight };
    }
  }
  return { width: 1, height: 1 };
}

/**
 * The image pixels, first and last inclusive, that `imageToWindow` maps into window pixels
 * `start` up to (not including) `end` along one axis, or null when none does.
 */
export function windowSpanToImage(start: number, end: number, original: number, image: number): [number, number] | null {
  if (original <= 0 || image <= 0) return null;
  const toWindow = (pixel: number) => Math.round((pixel * original) / image);
  let first = Math.max(0, Math.floor((start * image) / original) - 1);
  while (first < image && toWindow(first) < start) first += 1;
  let last = Math.min(image - 1, Math.ceil((end * image) / original) + 1);
  while (last >= 0 && toWindow(last) >= end) last -= 1;
  return first <= last ? [first, last] : null;
}

/**
 * The image pixels, first and last inclusive, of window pixels `start` up to (not including)
 * `start + length` along one axis, for an image whose pixel 0 shows window pixel `origin`. Not
 * clamped to the image: a span partly or wholly outside it comes back negative or past its size.
 * At least one pixel wide.
 */
export function windowRangeToImage(start: number, length: number, origin: number, original: number, image: number): [number, number] {
  const toImage = (pixel: number) => Math.round(((pixel - origin) * image) / original);
  const first = toImage(start);
  return [first, Math.max(first, toImage(start + length) - 1)];
}

export function imageToWindow(x: number, y: number, originalWidth: number, originalHeight: number, imageWidth: number, imageHeight: number): { x: number; y: number } {
  return {
    x: Math.round((x * originalWidth) / imageWidth),
    y: Math.round((y * originalHeight) / imageHeight),
  };
}
