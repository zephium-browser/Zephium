/** A box in the page's own pixels. */
export type Box = { x: number; y: number; w: number; h: number };

/** A sky picture's size, and where its horizon lies as a share of its height.
 *  Below the horizon the picture dissolves into the page through its own
 *  halftone, baked in, and ends in the page's colour. */
export type SkyPicture = { width: number; height: number; horizon: number };

/** Both skies are one place at two hours, framed alike. */
export const PICTURE: SkyPicture = { width: 1586, height: 992, horizon: 0.72 };

/** The picture laid over the page: across all of it, from above its top, with
 *  its horizon on the page's. Whatever it leaves below is the page. */
export function placeSky(width: number, horizon: number, picture: SkyPicture = PICTURE): Box {
  if (width <= 0 || horizon <= 0) return { x: 0, y: 0, w: 0, h: 0 };
  const scale = Math.max(width / picture.width, horizon / (picture.horizon * picture.height));
  const w = picture.width * scale;
  const h = picture.height * scale;
  return { x: (width - w) / 2, y: horizon - picture.horizon * h, w, h };
}
