import type { Rect } from "./lanes";
import type { Point } from "./lines";

export type Viewport = { x: number; y: number; zoom: number };
/** What can be seen of the canvas: its size, and how much of its top and bottom the chrome covers. */
export type Room = { width: number; height: number; top: number; bottom: number };

/** An item centred in the room at a zoom, never closer than 24 to the room's start. */
export function centreOn(rect: Rect, room: Room, zoom = 1): Viewport {
  return {
    x: Math.max(24, (room.width - rect.width * zoom) / 2) - rect.x * zoom,
    y:
      room.top +
      Math.max(0, (room.height - room.bottom - room.top - rect.height * zoom) / 2) -
      rect.y * zoom,
    zoom,
  };
}

/**
 * The view that holds a span across the room's width: at 100% when it fits,
 * down to `least`; wider still, its right end stays in view.
 */
export function frameSpan(
  rect: { x: number; y: number; width: number },
  room: { width: number; top: number },
  air = 64,
  least = 0.5,
): Viewport {
  const space = Math.max(1, room.width - air * 2);
  const zoom = Math.max(least, Math.min(1, space / Math.max(1, rect.width)));
  const over = Math.max(0, rect.width * zoom - space);
  return { x: air - rect.x * zoom - over, y: room.top + 40 - rect.y * zoom, zoom };
}

/** The least pan that brings a rect into the room with `air` around it; null when it is in view. */
export function reveal(rect: Rect, view: Viewport, room: Room, air = 48): Viewport | null {
  const left = rect.x * view.zoom + view.x;
  const top = rect.y * view.zoom + view.y;
  const right = left + rect.width * view.zoom;
  const bottom = top + rect.height * view.zoom;
  const shift = (from: number, to: number, min: number, max: number) =>
    to - from > max - min ? min - from : to > max ? max - to : from < min ? min - from : 0;
  const dx = shift(left, right, air, room.width - air);
  const dy = shift(top, bottom, room.top + air, room.height - room.bottom - air);
  if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return null;
  return { ...view, x: view.x + dx, y: view.y + dy };
}

/** A canvas point in client coordinates. */
export const toScreen = (point: Point, origin: Point, view: Viewport): Point => ({
  x: origin.x + point.x * view.zoom + view.x,
  y: origin.y + point.y * view.zoom + view.y,
});

/** Where a client point lands on the canvas. */
export const toCanvas = (client: Point, origin: Point, view: Viewport): Point => ({
  x: (client.x - origin.x - view.x) / view.zoom,
  y: (client.y - origin.y - view.y) / view.zoom,
});

/** A `cubic-bezier()` token as a function of time, for the flow's own tween. */
export function bezier(token: string): (t: number) => number {
  const [a, b, c, d] = (token.match(/-?[\d.]+/gu) ?? []).map(Number);
  if ([a, b, c, d].some((value) => value === undefined || Number.isNaN(value))) return (t) => t;
  const at = (p: number, q: number, u: number) =>
    3 * p * u * (1 - u) ** 2 + 3 * q * u ** 2 * (1 - u) + u ** 3;
  return (t) => {
    let low = 0;
    let high = 1;
    for (let step = 0; step < 24; step++) {
      const mid = (low + high) / 2;
      if (at(a!, c!, mid) < t) low = mid;
      else high = mid;
    }
    return at(b!, d!, (low + high) / 2);
  };
}
