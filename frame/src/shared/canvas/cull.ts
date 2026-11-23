import type { Viewport } from "./camera";
import type { Rect } from "./lanes";

/**
 * The items that have come within a screen of the view, on any side, and
 * were not near before. A caller that keeps the union draws an item's
 * content once and keeps it while the flow keeps the item.
 */
export function cameNear(
  items: Iterable<{ id: string; rect: Rect }>,
  near: ReadonlySet<string>,
  view: Viewport,
  width: number,
  height: number,
): string[] {
  const left = -view.x / view.zoom - width / view.zoom;
  const top = -view.y / view.zoom - height / view.zoom;
  const right = left + (width * 3) / view.zoom;
  const bottom = top + (height * 3) / view.zoom;
  const added: string[] = [];
  for (const { id, rect } of items) {
    if (near.has(id)) continue;
    if (
      rect.x > right ||
      rect.y > bottom ||
      rect.x + rect.width < left ||
      rect.y + rect.height < top
    )
      continue;
    added.push(id);
  }
  return added;
}

/** Seen from far under half size, with a margin either way so a pinch around the line doesn't flicker. */
const FAR = { below: 0.5, above: 0.6 } as const;
export const seenFar = (far: boolean, zoom: number) => (far ? zoom <= FAR.above : zoom < FAR.below);
