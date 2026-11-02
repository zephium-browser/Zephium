import day from "./assets/sky-day.avif";
import night from "./assets/sky-night.avif";
import frostDay from "./assets/frost-day.png";
import frostNight from "./assets/frost-night.png";

export type Scene = "day" | "night";

export const SCENES: Record<Scene, string> = { day, night };

/** Each sky seen through frosted glass: small enough that stretching it is
 *  the blur, so a surface wears it without drawing one. */
export const FROSTS: Record<Scene, string> = { day: frostDay, night: frostNight };

const decoded = new Set<Scene>();

/** Whether this session has shown the sky already. */
export const seen = (scene: Scene) => decoded.has(scene);

/** Decodes a sky off the main thread, so it is shown whole or not at all. */
export function decode(scene: Scene): Promise<void> {
  if (decoded.has(scene)) return Promise.resolve();
  const image = new Image();
  image.src = SCENES[scene];
  return image.decode().then(() => {
    decoded.add(scene);
  });
}
