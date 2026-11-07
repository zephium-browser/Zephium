import day from "./assets/sky-day.avif";
import day1920 from "./assets/sky-day-1920.avif";
import day2560 from "./assets/sky-day-2560.avif";
import night from "./assets/sky-night.avif";
import night1920 from "./assets/sky-night-1920.avif";
import night2560 from "./assets/sky-night-2560.avif";
import frostDay from "./assets/frost-day.png";
import frostNight from "./assets/frost-night.png";
import { PICTURE } from "./place";

export type Scene = "day" | "night";

/** Each sky whole, for a stage that zooms in on it. */
export const SCENES: Record<Scene, string> = { day, night };

/** Each sky at the widths it is kept at, narrowest first. A picture is
 *  decoded whole however small it is drawn, and stays decoded while the page
 *  is open, so a sky is drawn from the narrowest one that still covers it. */
const SIZES: Record<Scene, readonly (readonly [number, string])[]> = {
  day: [
    [1920, day1920],
    [2560, day2560],
    [PICTURE.width, day],
  ],
  night: [
    [1920, night1920],
    [2560, night2560],
    [PICTURE.width, night],
  ],
};

/** The sky to draw `width` page pixels across on a screen of `density`. */
export function skyImage(
  scene: Scene,
  width: number,
  density: number = globalThis.devicePixelRatio || 1,
): string {
  const needed = width * density;
  const sizes = SIZES[scene];
  const fits = sizes.find(([kept]) => kept >= needed);
  return fits ? fits[1] : SCENES[scene];
}

/** Each sky seen through frosted glass: small enough that stretching it is
 *  the blur, so a surface wears it without drawing one. */
export const FROSTS: Record<Scene, string> = { day: frostDay, night: frostNight };

const shown = new Set<Scene>();
const decoded = new Set<string>();
const decoding = new Map<string, Promise<void>>();

/** Whether this session has shown the sky already. */
export const seen = (scene: Scene) => shown.has(scene);

/** Whether this picture is decoded, so it can be shown at once. */
export const isDecoded = (url: string) => decoded.has(url);

/** The decoded picture to draw a sky `width` page pixels across from: the
 *  narrowest decoded one that covers it, else the widest decoded one, else
 *  none. `covers` says whether it is wide enough to keep. */
export function decodedFor(
  scene: Scene,
  width: number,
  density: number = globalThis.devicePixelRatio || 1,
): { url: string; covers: boolean } | null {
  const needed = width * density;
  const held = SIZES[scene].filter(([, url]) => decoded.has(url));
  const covering = held.find(([kept]) => kept >= needed);
  if (covering) return { url: covering[1], covers: true };
  const widest = held.at(-1);
  return widest ? { url: widest[1], covers: false } : null;
}

/** Decodes a picture a sky is drawn from, off the main thread, so it is
 *  shown whole or not at all: by default the whole sky. */
export function decode(scene: Scene, url: string = SCENES[scene]): Promise<void> {
  if (decoded.has(url)) return Promise.resolve();
  const pending = decoding.get(url);
  if (pending) return pending;
  const image = new Image();
  image.src = url;
  const done = image
    .decode()
    .then(() => {
      decoded.add(url);
      shown.add(scene);
    })
    .finally(() => decoding.delete(url));
  decoding.set(url, done);
  return done;
}
