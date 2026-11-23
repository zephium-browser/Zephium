export type CameraView = { x: number; y: number; zoom: number };
/** The part of the canvas the person can see: its width, and the band between the chrome above and below. */
export type CameraRoom = { width: number; top: number; bottom: number };
/** A span of the canvas, in canvas units. */
export type CameraBox = { left: number; top: number; right: number; bottom: number };

const AIR = 48;
/** Room kept ahead of what is followed, so the next step does not move the view again. */
const LEAD = 0.14;
const MIN_MS = 250;
const MAX_MS = 400;

/** A move is a pan: the zoom is the person's and never changes. */
export type CameraMove = { view: CameraView; ms: number };

export const glideMs = (reduced: boolean, base: number) =>
  reduced ? 0 : Math.min(MAX_MS, Math.max(MIN_MS, base));

const onScreen = (view: CameraView, box: CameraBox): CameraBox => ({
  left: box.left * view.zoom + view.x,
  top: box.top * view.zoom + view.y,
  right: box.right * view.zoom + view.x,
  bottom: box.bottom * view.zoom + view.y,
});

/** The least shift along one axis that brings [from, to] inside [min, max], with `lead` spare ahead. */
function shift(from: number, to: number, min: number, max: number, lead: number) {
  if (to - from > max - min) return min - from;
  if (to > max) return max - lead - to;
  if (from < min) return min + lead - from;
  return 0;
}

function pan(view: CameraView, dx: number, dy: number, reduced: boolean, base: number) {
  if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return null;
  return { view: { ...view, x: view.x + dx, y: view.y + dy }, ms: glideMs(reduced, base) };
}

/**
 * Where the camera goes to keep the run's newest activity in view: nowhere
 * while the person has taken the camera or it is already in view, otherwise
 * the shortest pan that brings it in, with room ahead of it. A span wider
 * than the room is anchored by its left end, so the run reads left to right.
 */
export function followMove(input: {
  view: CameraView;
  room: CameraRoom;
  newest: CameraBox;
  following: boolean;
  reduced: boolean;
  base: number;
}): CameraMove | null {
  const { view, room, newest, following, reduced, base } = input;
  if (!following) return null;
  const seen = onScreen(view, newest);
  const dx = shift(seen.left, seen.right, AIR, room.width - AIR, room.width * LEAD);
  const dy = shift(seen.top, seen.bottom, room.top + AIR, room.bottom - AIR, 0);
  return pan(view, dx, dy, reduced, base);
}

/**
 * The one move a run's end may make: the result's top comes into view when
 * it is not. A result already showing its top does not move the camera.
 */
export function settleMove(input: {
  view: CameraView;
  room: CameraRoom;
  result: CameraBox;
  following: boolean;
  reduced: boolean;
  base: number;
}): CameraMove | null {
  const { view, room, result, following, reduced, base } = input;
  if (!following) return null;
  const seen = onScreen(view, result);
  const topShown = seen.top >= room.top && seen.top <= room.bottom - AIR * 2;
  const sideShown = seen.right > AIR * 2 && seen.left < room.width - AIR * 2;
  if (topShown && sideShown) return null;
  const dy = room.top + AIR - seen.top;
  const dx = shift(seen.left, seen.right, AIR, room.width - AIR, 0);
  return pan(view, dx, topShown ? 0 : dy, reduced, base);
}
