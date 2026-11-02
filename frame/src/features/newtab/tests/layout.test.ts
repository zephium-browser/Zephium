import { describe, expect, it } from "vitest";
import { layout } from "../lib/layout";

const FULL = { controls: [32, 32], clock: true };

describe("layout", () => {
  it("stands the name low in the open sky with the time and the date under it", () => {
    const page = layout(1200, 800, FULL);
    expect(page.horizon).toBe(480);
    expect(page.mark.w).toBe(432);
    expect(page.mark.x + page.mark.w / 2).toBeCloseTo(600, 0);
    expect(page.when).toBe(page.mark.y + page.mark.h + 16);
    expect(page.when! + 20).toBeLessThan(page.horizon - 40);
  });

  it("gives each control its own round button in the top corner", () => {
    const page = layout(700, 800, FULL);
    expect(page.notch.width).toBe(480);
    expect(page.controls).toEqual([
      { x: 700 - 14 - 32 - 38, y: 9, w: 32, h: 32 },
      { x: 700 - 14 - 32, y: 9, w: 32, h: 32 },
    ]);
  });

  it("gives words a capsule as wide as they are, and keeps the corner where it was", () => {
    const page = layout(700, 800, { ...FULL, controls: [64, 32] });
    // The notch gives up what the wider control needs, on both sides.
    expect(page.notch.width).toBe(480 - 2 * 32);
    expect(page.controls).toEqual([
      { x: 700 - 14 - 32 - 6 - 64, y: 9, w: 64, h: 32 },
      { x: 700 - 14 - 32, y: 9, w: 32, h: 32 },
    ]);
  });

  it("closes up around what is not shown", () => {
    const page = layout(1200, 800, { controls: [], clock: false });
    expect(page.controls).toEqual([]);
    expect(page.when).toBeNull();
  });

  it("never lets the name ride up under the notch on a short page", () => {
    expect(layout(1200, 300, FULL).mark.y).toBeGreaterThanOrEqual(82);
  });
});

it("removes the wordmark's height and clock gap and leaves the controls where they were", () => {
  const normal = layout(1200, 800, FULL);
  const hidden = layout(1200, 800, { ...FULL, wordmark: false });
  expect(hidden.mark.h).toBe(0);
  expect(hidden.when).toBe(hidden.mark.y);
  expect(hidden.when).toBeLessThan(normal.when!);
  expect(hidden.controls).toEqual(normal.controls);
  expect(hidden.notch).toEqual(normal.notch);
  expect(layout(1200, 800, { ...FULL, wordmark: false, clock: false }).when).toBeNull();
});
