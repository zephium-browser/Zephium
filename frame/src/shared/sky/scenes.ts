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

/** Whether this session has shown the sky already. */
export const seen = (scene: Scene) => shown.has(scene);

/** Decodes the picture a sky is drawn from, off the main thread, so it is
 *  shown whole or not at all: the one for `width`, or the whole sky. */
export function decode(scene: Scene, width?: number): Promise<void> {
  const url = width === undefined ? SCENES[scene] : skyImage(scene, width);
  if (decoded.has(url)) return Promise.resolve();
  const image = new Image();
  image.src = url;
  return image.decode().then(() => {
    decoded.add(url);
    shown.add(scene);
  });
}
