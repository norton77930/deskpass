import assert from "node:assert/strict";
import { test } from "node:test";
import { imageToWindow, targetSize, windowSpanToImage } from "../src/scaler.ts";

const samples = [
  [1, 1],
  [28, 28],
  [27, 27],
  [29, 29],
  [100, 100],
  [640, 480],
  [1568, 28],
  [28, 1568],
  [1569, 28],
  [28, 1569],
  [1920, 1080],
  [1080, 1920],
  [3840, 2160],
  [1568, 1568],
  [8000, 8000],
  [100000, 1],
  [1, 100000],
  [4000, 3000],
  [3000, 4000],
];

function fits(width: number, height: number): boolean {
  return Math.max(width, height) <= 1568 && Math.ceil(width / 28) * Math.ceil(height / 28) <= 1568;
}

for (const [width, height] of samples) {
  test(`screenshot budget for ${width}x${height}`, () => {
    const sized = targetSize(width, height);
    assert.ok(fits(sized.width, sized.height), `${sized.width}x${sized.height} is outside the budget`);
    assert.ok(sized.width >= 1 && sized.height >= 1);
    assert.ok(sized.width <= width && sized.height <= height, "must not upscale");
    if (fits(width, height)) {
      assert.deepEqual(sized, { width, height });
      return;
    }
    const longIsWidth = width >= height;
    const longIn = longIsWidth ? width : height;
    const shortIn = longIsWidth ? height : width;
    const chosenLong = longIsWidth ? sized.width : sized.height;
    const biggerLong = chosenLong + 1;
    if (biggerLong > longIn || biggerLong > 1568) return;
    const biggerShort = Math.max(1, Math.floor((shortIn * biggerLong) / longIn));
    const biggerWidth = longIsWidth ? biggerLong : biggerShort;
    const biggerHeight = longIsWidth ? biggerShort : biggerLong;
    const biggerStillInSource = biggerWidth <= width && biggerHeight <= height;
    assert.equal(biggerStillInSource && fits(biggerWidth, biggerHeight), false);
  });
}

// Ticket 45 (F21): windowSpanToImage is the inverse of imageToWindow along one axis.
test("a window span maps to the image pixels that land inside it", () => {
  // Same scale.
  assert.deepEqual(windowSpanToImage(10, 20, 400, 400), [10, 19]);
  // Downscaled by two: image pixel p is window pixel 2p.
  assert.deepEqual(windowSpanToImage(10, 20, 200, 100), [5, 9]);
  // Upscaled by two: image pixel p is window pixel round(p / 2).
  assert.deepEqual(windowSpanToImage(10, 20, 100, 200), [19, 38]);
});

test("a window span is clipped to the image and is null when no image pixel lands in it", () => {
  assert.deepEqual(windowSpanToImage(0, 200, 200, 100), [0, 99]);
  assert.deepEqual(windowSpanToImage(190, 300, 200, 100), [95, 99]);
  assert.equal(windowSpanToImage(200, 300, 200, 100), null);
  assert.equal(windowSpanToImage(10, 10, 200, 100), null);
  // Window pixel 11 only: the image pixels around it show window pixels 10 and 12.
  assert.equal(windowSpanToImage(11, 12, 200, 100), null);
  assert.equal(windowSpanToImage(0, 10, 0, 100), null);
  assert.equal(windowSpanToImage(0, 10, 200, 0), null);
});

test("a window span left of or above the window keeps only the part inside it", () => {
  assert.deepEqual(windowSpanToImage(-50, 5, 200, 100), [0, 2]);
  assert.equal(windowSpanToImage(-50, -10, 200, 100), null);
  assert.equal(windowSpanToImage(-50, 0, 200, 100), null);
});

test("a window span matches an exhaustive scan of imageToWindow", () => {
  for (const [original, image] of [[2560, 1498], [1389, 813], [400, 300], [7, 3], [3, 7], [1, 1]]) {
    const spans = [[0, original], [-5, 3], [1, 2], [original - 3, original + 10], [Math.floor(original / 3), Math.floor(original / 2) + 1]];
    for (const [start, end] of spans) {
      const inside: number[] = [];
      for (let pixel = 0; pixel < image; pixel += 1) {
        const mapped = imageToWindow(pixel, 0, original, 1, image, 1).x;
        if (mapped >= start && mapped < end) inside.push(pixel);
      }
      const expected = inside.length === 0 ? null : [inside[0], inside.at(-1)];
      assert.deepEqual(windowSpanToImage(start, end, original, image), expected, `${start}..${end} of ${original} in ${image}`);
    }
  }
});
