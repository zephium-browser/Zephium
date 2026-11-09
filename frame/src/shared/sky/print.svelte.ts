import { SvelteSet } from "svelte/reactivity";
import { INKS, edgeLight, printSky, type Picture, type Rgb, type SkyFrame } from "./dither";
import type { Scene } from "./scenes";

let last: { key: string; pixels: ImageData } | null = null;
const shown = new SvelteSet<Scene>();

/** Whether this session has printed the scene already. */
export const seen = (scene: Scene) => shown.has(scene);
const keyOf = (scene: Scene, frame: SkyFrame) =>
  [
    scene,
    frame.columns,
    frame.rows,
    frame.dot,
    frame.density,
    frame.sky.x,
    frame.sky.y,
    frame.sky.w,
    frame.sky.h,
    frame.horizon,
    frame.quiet ? `${frame.quiet.x},${frame.quiet.y},${frame.quiet.rx},${frame.quiet.ry}` : "",
  ].join(" ");

/** The printed sky for a frame. The last one is kept, so the next new tab of
 *  the same size shows it without printing again. */
export function printed(scene: Scene, picture: Picture, frame: SkyFrame): ImageData {
  const key = keyOf(scene, frame);
  if (last?.key === key) return last.pixels;
  const pixels = new ImageData(frame.columns, frame.rows);
  printSky(pixels.data, picture, INKS[scene], frame);
  last = { key, pixels };
  shown.add(scene);
  light = { scene, bands: edgeLight(pixels.data, frame.columns, frame.rows) };
  return pixels;
}

let light = $state.raw<{ scene: Scene; bands: Rgb[] } | null>(null);

/** The light the last printed sky throws from its left edge, band by band down
 *  the page, for the window's glass beside it. */
export const skyEdgeLight = () => light;

const FROST_WIDTH = 24;
/** How a frost is evened out under the interface's ink: pulled toward its own
 *  mean light, then veiled in the scene's paper. */
const FROSTING: Record<
  Scene,
  { flatten: number; veil: readonly [number, number, number, number] }
> = {
  day: { flatten: 0.5, veil: [255, 255, 255, 0.25] },
  night: { flatten: 0.3, veil: [10, 30, 54, 0.26] },
};

let frosts = $state.raw<Partial<Record<Scene, string>>>({});

/** The printed sky seen through frosted glass: a handful of pixels a surface
 *  stretches over itself, so the stretch is the blur. */
export const skyFrost = (scene: Scene) => frosts[scene] ?? null;

export function frostFrom(scene: Scene, source: HTMLCanvasElement) {
  if (frosts[scene] || source.width === 0) return;
  const height = Math.max(1, Math.round((FROST_WIDTH * source.height) / source.width));
  const middle = document.createElement("canvas");
  middle.width = FROST_WIDTH * 8;
  middle.height = height * 8;
  const small = document.createElement("canvas");
  small.width = FROST_WIDTH;
  small.height = height;
  const step = middle.getContext("2d");
  const context = small.getContext("2d", { willReadFrequently: true });
  if (!step || !context) return;
  step.imageSmoothingQuality = "high";
  step.drawImage(source, 0, 0, middle.width, middle.height);
  context.imageSmoothingQuality = "high";
  context.drawImage(middle, 0, 0, FROST_WIDTH, height);
  const image = context.getImageData(0, 0, FROST_WIDTH, height);
  const { flatten, veil } = FROSTING[scene];
  const data = image.data;
  let mean = 0;
  for (let i = 0; i < data.length; i += 4)
    mean += 0.2126 * data[i]! + 0.7152 * data[i + 1]! + 0.0722 * data[i + 2]!;
  mean /= data.length / 4;
  for (let i = 0; i < data.length; i += 4) {
    const own = 0.2126 * data[i]! + 0.7152 * data[i + 1]! + 0.0722 * data[i + 2]!;
    const lift = (mean - own) * flatten;
    for (let c = 0; c < 3; c++) {
      const flat = data[i + c]! + lift;
      data[i + c] = flat + (veil[c]! - flat) * veil[3];
    }
  }
  context.putImageData(image, 0, 0);
  frosts = { ...frosts, [scene]: small.toDataURL() };
  middle.width = 0;
}
