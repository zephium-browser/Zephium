import type { Rect } from "./lanes";

/** A place the keyboard can stand on a canvas: an item, or a row inside one. */
export type FocusStop = {
  id: string;
  /** Where it stands, in canvas units. */
  rect: Rect;
};
export type Direction = "up" | "down" | "left" | "right";

export const DIRECTIONS: Readonly<Record<string, Direction>> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
};

/** Off-axis distance counts this much more than distance along the way asked. */
const ASIDE = 3;
/** Among stops that line up alike, the one most level with where the keyboard is. */
const LEVEL = 0.1;

/**
 * The stop the keyboard moves to from `from`, going one way: the nearest
 * whose box starts past `from`'s middle that way, preferring one that lines
 * up with it across. Null when nothing stands that way.
 */
export function nextStop(
  stops: readonly FocusStop[],
  from: string,
  direction: Direction,
): string | null {
  const here = stops.find((stop) => stop.id === from)?.rect;
  if (!here) return stops[0]?.id ?? null;
  const vertical = direction === "up" || direction === "down";
  const sign = direction === "down" || direction === "right" ? 1 : -1;
  const middle = (rect: Rect) => (vertical ? rect.y + rect.height / 2 : rect.x + rect.width / 2);
  const span = (rect: Rect): [number, number] =>
    vertical ? [rect.x, rect.x + rect.width] : [rect.y, rect.y + rect.height];
  const lead = (rect: Rect) => (vertical ? rect.y : rect.x);
  const tail = (rect: Rect) => (vertical ? rect.y + rect.height : rect.x + rect.width);
  const [a, b] = span(here);
  let best: { id: string; score: number } | null = null;
  for (const stop of stops) {
    if (stop.id === from) continue;
    const rect = stop.rect;
    const ahead = sign > 0 ? lead(rect) - middle(here) : middle(here) - tail(rect);
    if (ahead < 0 || (sign > 0 ? middle(rect) <= middle(here) : middle(rect) >= middle(here)))
      continue;
    const [c, d] = span(rect);
    const aside = Math.max(0, c - b, a - d);
    const off = Math.abs((c + d) / 2 - (a + b) / 2);
    const score = Math.abs(middle(rect) - middle(here)) + aside * ASIDE + off * LEVEL;
    if (!best || score < best.score) best = { id: stop.id, score };
  }
  return best?.id ?? null;
}
