import type { Box } from "./place";
import type { Scene } from "./scenes";

export type Rgb = readonly [number, number, number];

/** A sky's picture, decoded once: three bytes a pixel. */
export type Picture = { data: Uint8Array; width: number; height: number };

/** How a scene is printed: each dot keeps the picture's own colour, its light
 *  screened between steps that rise by `ratio` each, from the page's own
 *  `paper` up. A step is the same share of light in the deep blue of the
 *  night as in the day's cloud, so the screen reads alike in both. Below the
 *  horizon the sky thins into the paper; `hold` is how far the sky behind the
 *  page's type gives way to its open blue `still`, so the type reads on cloud. */
export type Ink = { paper: Rgb; ratio: number; hold: number; still: Rgb };

export const INKS: Record<Scene, Ink> = {
  day: { paper: [255, 255, 255], ratio: 1.12, hold: 0.45, still: [104, 168, 238] },
  night: { paper: [10, 30, 54], ratio: 1.28, hold: 0, still: [10, 30, 54] },
};

/** The page's own colour under a sky: what it thins into. */
export const paperOf = (scene: Scene): Rgb => INKS[scene].paper;

const lightOf = (r: number, g: number, b: number) =>
  Math.max(1 / 255, (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255);

/** Device pixels per dot: about one and a half page pixels, never under two
 *  device pixels, so each dot is a crisp square at every density. */
export function dotSize(density: number): number {
  return Math.max(2, Math.round(1.5 * density));
}

const BAYER = (() => {
  let matrix = [[0]];
  while (matrix.length < 8) {
    const n = matrix.length;
    const next = Array.from({ length: n * 2 }, () => new Array<number>(n * 2).fill(0));
    for (let y = 0; y < n; y++)
      for (let x = 0; x < n; x++) {
        const v = matrix[y]![x]! * 4;
        next[y]![x] = v;
        next[y]![x + n] = v + 2;
        next[y + n]![x] = v + 3;
        next[y + n]![x + n] = v + 1;
      }
    matrix = next;
  }
  return Float32Array.from(matrix.flat(), (v) => (v + 0.5) / 64);
})();

/** Keeps a decoded source's colour, three bytes a pixel. */
export function pictureOf(rgba: Uint8ClampedArray, width: number, height: number): Picture {
  const data = new Uint8Array(width * height * 3);
  for (let i = 0, p = 0; p < data.length; i += 4, p += 3) {
    data[p] = rgba[i]!;
    data[p + 1] = rgba[i + 1]!;
    data[p + 2] = rgba[i + 2]!;
  }
  return { data, width, height };
}

export type SkyFrame = {
  /** Dots across and down. */
  columns: number;
  rows: number;
  /** Device pixels per dot. */
  dot: number;
  density: number;
  /** Where the picture lies on the page, in page pixels (see placeSky). */
  sky: Box;
  /** The page's horizon, in page pixels: below it the sky dissolves into paper. */
  horizon: number;
  /** The share of the picture's height its own horizon sits at. */
  pictureHorizon: number;
  /** Where the page's type stands, as an ellipse in page pixels: the sky is
   *  held quieter there. */
  quiet?: Quiet | null;
};

export type Quiet = { x: number; y: number; rx: number; ry: number };

/** The page's size in dots for a page `width`×`height` page pixels. */
export function frameFor(width: number, height: number, density: number) {
  const dot = dotSize(density);
  return {
    dot,
    columns: Math.ceil((width * density) / dot),
    rows: Math.ceil((height * density) / dot),
  };
}

/** Prints the sky into `out` (RGBA, columns × rows): each dot is the picture's
 *  colour at the step of light above or below its own, chosen by an ordered
 *  screen. The steps are laid from the paper, so where the sky has thinned
 *  into it below the horizon the page is one flat colour. */
export function printSky(out: Uint8ClampedArray, picture: Picture, ink: Ink, frame: SkyFrame) {
  const { columns, rows, dot, density, sky, horizon, quiet } = frame;
  const { data, width, height } = picture;
  const { paper, ratio, hold, still } = ink;
  const held = quiet && hold > 0 ? quiet : null;
  const floor = lightOf(paper[0], paper[1], paper[2]);
  const rise = Math.log(ratio);
  const step = dot / density;
  const toX = width / sky.w;
  const toY = height / sky.h;
  const horizonRow = horizon / step;
  const fade = Math.max(1, rows - horizonRow);
  const rgb = [0, 0, 0];
  for (let row = 0; row < rows; row++) {
    const y = ((row + 0.5) * step - sky.y) * toY - 0.5;
    const y0 = Math.min(height - 1, Math.max(0, Math.floor(y)));
    const y1 = Math.min(height - 1, y0 + 1);
    const fy = Math.min(1, Math.max(0, y - y0));
    const below = Math.min(1, Math.max(0, (row - horizonRow) / fade) * 1.15);
    const dy = held ? ((row + 0.5) * step - held.y) / held.ry : 0;
    for (let column = 0; column < columns; column++) {
      const x = ((column + 0.5) * step - sky.x) * toX - 0.5;
      const x0 = Math.min(width - 1, Math.max(0, Math.floor(x)));
      const x1 = Math.min(width - 1, x0 + 1);
      const fx = Math.min(1, Math.max(0, x - x0));
      let give = 0;
      if (held) {
        const dx = ((column + 0.5) * step - held.x) / held.rx;
        const near = 1 - dx * dx - dy * dy;
        if (near > 0) give = hold * near * near * (3 - 2 * near);
      }
      for (let c = 0; c < 3; c++) {
        const top =
          data[(y0 * width + x0) * 3 + c]! * (1 - fx) + data[(y0 * width + x1) * 3 + c]! * fx;
        const bottom =
          data[(y1 * width + x0) * 3 + c]! * (1 - fx) + data[(y1 * width + x1) * 3 + c]! * fx;
        let value = top * (1 - fy) + bottom * fy;
        value += (still[c]! - value) * give;
        rgb[c] = value + (paper[c]! - value) * below;
      }
      const light = lightOf(rgb[0]!, rgb[1]!, rgb[2]!);
      const level = Math.log(light / floor) / rise;
      const base = Math.floor(level);
      const threshold = BAYER[((row & 7) << 3) | (column & 7)]!;
      const scale = (Math.exp((base + (level - base > threshold ? 1 : 0)) * rise) * floor) / light;
      const at = (row * columns + column) * 4;
      out[at] = rgb[0]! * scale;
      out[at + 1] = rgb[1]! * scale;
      out[at + 2] = rgb[2]! * scale;
      out[at + 3] = 255;
    }
  }
}

/** The light the printed sky gives off at its left edge, band by band down the
 *  page: what the window's glass beside the page catches of it. */
export function edgeLight(rgba: Uint8ClampedArray, columns: number, rows: number, bands = 32) {
  const reach = Math.max(1, Math.round(columns * 0.06));
  return Array.from({ length: bands }, (_, band): Rgb => {
    const top = Math.floor((band * rows) / bands);
    const bottom = Math.max(top + 1, Math.floor(((band + 1) * rows) / bands));
    const sum = [0, 0, 0];
    for (let y = top; y < bottom; y++)
      for (let x = 0; x < reach; x++)
        for (let c = 0; c < 3; c++) sum[c]! += rgba[(y * columns + x) * 4 + c]!;
    const count = (bottom - top) * reach;
    return [Math.round(sum[0]! / count), Math.round(sum[1]! / count), Math.round(sum[2]! / count)];
  });
}
