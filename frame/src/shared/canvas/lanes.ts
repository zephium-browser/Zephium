export type Rect = { x: number; y: number; width: number; height: number };

/** Everything on a canvas stands on this grid. */
const GRID = 8;

export const snap = (value: number, grid = GRID) => Math.ceil(value / grid) * grid;

/**
 * A short digest of an id. A line between two items is named by both, and
 * two full ids can run past an id limit.
 */
export function digest(id: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < id.length; index++)
    hash = Math.imul(hash ^ id.charCodeAt(index), 0x01000193);
  return (hash >>> 0).toString(36);
}

/** Two rects closer than `gap` on both axes. */
export const overlaps = (a: Rect, b: Rect, gap = 0) =>
  a.x < b.x + b.width + gap &&
  b.x < a.x + a.width + gap &&
  a.y < b.y + b.height + gap &&
  b.y < a.y + a.height + gap;

/** The box around rects, or null for none. */
export function bounds(rects: Iterable<Rect>): Rect | null {
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const rect of rects) {
    left = Math.min(left, rect.x);
    top = Math.min(top, rect.y);
    right = Math.max(right, rect.x + rect.width);
    bottom = Math.max(bottom, rect.y + rect.height);
  }
  return left === Infinity ? null : { x: left, y: top, width: right - left, height: bottom - top };
}

/**
 * Lanes one under the other from `top`, each `gap` under the bottom of the
 * one before, every top on the grid: where each lane starts.
 */
export function stack(heights: readonly number[], top: number, gap: number): number[] {
  const tops: number[] = [];
  let at = snap(top);
  for (const height of heights) {
    tops.push(at);
    at = snap(at + Math.max(0, height) + gap);
  }
  return tops;
}
