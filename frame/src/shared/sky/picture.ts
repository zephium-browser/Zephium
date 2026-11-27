import day from "./assets/sky-day.avif";
import night from "./assets/sky-night.avif";
import { pictureOf, type Picture } from "./dither";
import type { Scene } from "./scenes";

const SOURCES: Record<Scene, string> = { day, night };

let kept: { scene: Scene; picture: Promise<Picture> } | null = null;

/** A sky's picture, decoded and kept as three bytes a pixel for the scene on
 *  screen alone; the decoded image itself is let go as soon as it is read. */
export function loadPicture(scene: Scene): Promise<Picture> {
  if (kept?.scene === scene) return kept.picture;
  const picture = (async () => {
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
    const decoded = pictureOf(context.getImageData(0, 0, width, height).data, width, height);
    canvas.width = 0;
    canvas.height = 0;
    image.src = "";
    return decoded;
  })();
  const entry = { scene, picture };
  picture.catch(() => {
    if (kept === entry) kept = null;
  });
  kept = entry;
  return picture;
}
