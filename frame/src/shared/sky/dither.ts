import type { Box } from "./place";
import type { Scene } from "./scenes";

export type Rgb = readonly [number, number, number];

/** A sky's light, read from its source: one byte per pixel. */
export type Luma = {
  data: Uint8Array;
  width: number;
  height: number;
  /** The darkest and brightest light that matter, so a palette spans the picture. */
  low: number;
  high: number;
};

/** The tones a sky is printed in, darkest first. The page below the horizon is
 *  the scene's paper, one end of the palette. */
/** `curve` bends the light before it is printed: above 1 keeps the open sky
 *  in its deepest tone and saves the dots for the clouds. */
export type Palette = { tones: readonly Rgb[]; paper: number; contrast: number; curve: number };

export const PALETTES: Record<Scene, Palette> = {
  day: {
    tones: [
      [120, 150, 196],
      [184, 205, 231],
      [232, 238, 247],
      [251, 252, 254],
    ],
    paper: 1,
    contrast: 1,
    curve: 2.6,
  },
  night: {
    tones: [
      [10, 12, 18],
      [37, 44, 63],
      [107, 119, 148],
      [208, 214, 228],
    ],
    paper: 0,
    contrast: 1,
    curve: 1.35,
  },
};

/** The page's own colour under a sky. */
export const paperOf = (scene: Scene): Rgb => {
  const { tones, paper } = PALETTES[scene];
  return tones[paper ? tones.length - 1 : 0]!;
};

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

/** Reads the light that matters from a decoded source: the 1st and 99.5th
 *  percentiles, so a stray bright pixel cannot flatten the whole palette. */
export function lumaOf(rgba: Uint8ClampedArray, width: number, height: number): Luma {
  const data = new Uint8Array(width * height);
  const counts = new Uint32Array(256);
  for (let i = 0, p = 0; p < data.length; i += 4, p++) {
    const value = Math.round(0.2126 * rgba[i]! + 0.7152 * rgba[i + 1]! + 0.0722 * rgba[i + 2]!);
    data[p] = value;
    counts[value]!++;
  }
  const at = (share: number) => {
    const target = data.length * share;
    let seen = 0;
    for (let v = 0; v < 256; v++) if ((seen += counts[v]!) >= target) return v;
    return 255;
  };
  return { data, width, height, low: at(0.01), high: Math.max(at(0.995), at(0.01) + 1) };
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
};

/** The page's size in dots for a page `width`×`height` page pixels. */
export function frameFor(width: number, height: number, density: number) {
  const dot = dotSize(density);
  return {
    dot,
    columns: Math.ceil((width * density) / dot),
    rows: Math.ceil((height * density) / dot),
  };
}

/** Prints the sky into `out` (RGBA, columns × rows): its light mapped onto the
 *  palette's tones, each dot taking the tone above or below by an ordered
 *  screen, and below the horizon thinning into the paper. */
export function printSky(out: Uint8ClampedArray, luma: Luma, palette: Palette, frame: SkyFrame) {
  const { columns, rows, dot, density, sky, horizon } = frame;
  const { data, width, height, low, high } = luma;
  const { tones, paper, contrast, curve } = palette;
  const last = tones.length - 1;
  const step = dot / density;
  const range = high - low;
  const toX = width / sky.w;
  const toY = height / sky.h;
  const horizonRow = horizon / step;
  const fade = Math.max(1, rows - horizonRow);
  for (let row = 0; row < rows; row++) {
    const y = ((row + 0.5) * step - sky.y) * toY - 0.5;
    const y0 = Math.min(height - 1, Math.max(0, Math.floor(y)));
    const y1 = Math.min(height - 1, y0 + 1);
    const fy = Math.min(1, Math.max(0, y - y0));
    const below = Math.min(1, Math.max(0, (row - horizonRow) / fade) * 1.15);
    const toward = paper * below;
    const keep = 1 - below;
    for (let column = 0; column < columns; column++) {
      const x = ((column + 0.5) * step - sky.x) * toX - 0.5;
      const x0 = Math.min(width - 1, Math.max(0, Math.floor(x)));
      const x1 = Math.min(width - 1, x0 + 1);
      const fx = Math.min(1, Math.max(0, x - x0));
      const top = data[y0 * width + x0]! * (1 - fx) + data[y0 * width + x1]! * fx;
      const bottom = data[y1 * width + x0]! * (1 - fx) + data[y1 * width + x1]! * fx;
      let light = ((top * (1 - fy) + bottom * fy - low) / range - 0.5) * contrast + 0.5;
      light = Math.min(1, Math.max(0, light)) ** curve * keep + toward;
      const level = light * last;
      const base = Math.floor(level);
      const tone = Math.min(
        last,
        base + (level - base > BAYER[((row & 7) << 3) | (column & 7)]! ? 1 : 0),
      );
      const rgb = tones[tone]!;
      const at = (row * columns + column) * 4;
      out[at] = rgb[0];
      out[at + 1] = rgb[1];
      out[at + 2] = rgb[2];
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
