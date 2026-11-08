import dayLight from "./assets/sky-day-light.avif";
import nightLight from "./assets/sky-night-light.avif";
import { lumaOf, type Luma } from "./dither";
import type { Scene } from "./scenes";

const SOURCES: Record<Scene, string> = { day: dayLight, night: nightLight };

const lumas = new Map<Scene, Promise<Luma>>();

/** Each sky's light, decoded once a session and kept as one byte a pixel;
 *  the decoded picture itself is let go as soon as it is read. */
export function loadLuma(scene: Scene): Promise<Luma> {
  let pending = lumas.get(scene);
  if (!pending) {
    pending = (async () => {
      const image = new Image();
      image.src = SOURCES[scene];
      await image.decode();
      const { naturalWidth: width, naturalHeight: height } = image;
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) throw new Error("sky: no 2d context");
      context.drawImage(image, 0, 0);
      const luma = lumaOf(context.getImageData(0, 0, width, height).data, width, height);
      canvas.width = 0;
      canvas.height = 0;
      image.src = "";
      return luma;
    })();
    pending.catch(() => lumas.delete(scene));
    lumas.set(scene, pending);
  }
  return pending;
}
