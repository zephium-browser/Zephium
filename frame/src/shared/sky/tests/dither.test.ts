import { describe, expect, it } from "vitest";
import { dotSize, edgeLight, frameFor, INKS, paperOf, pictureOf, printSky } from "../dither";

const flat = (rgb: readonly number[], width = 8, height = 8) => {
  const rgba = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < rgba.length; i += 4) rgba.set([...rgb, 255], i);
  return pictureOf(rgba, width, height);
};

describe("dotSize", () => {
  it("keeps every dot a whole number of device pixels, never under two", () => {
    expect(dotSize(1)).toBe(2);
    expect(dotSize(1.25)).toBe(2);
    expect(dotSize(2)).toBe(3);
    expect(dotSize(3)).toBe(5);
  });
});

describe("printSky", () => {
  const frame = (width: number, height: number, horizon: number) => ({
    ...frameFor(width, height, 2),
    density: 2,
    sky: { x: 0, y: 0, w: width, h: height },
    horizon,
    pictureHorizon: 0.69,
  });
  const print = (
    picture: ReturnType<typeof flat>,
    f: ReturnType<typeof frame> & { quiet?: { x: number; y: number; rx: number; ry: number } },
    scene: "day" | "night",
  ) => {
    const out = new Uint8ClampedArray(f.columns * f.rows * 4);
    printSky(out, picture, INKS[scene], f);
    return out;
  };
  const colours = (out: Uint8ClampedArray) => {
    const seen = new Set<string>();
    for (let i = 0; i < out.length; i += 4) seen.add(`${out[i]},${out[i + 1]},${out[i + 2]}`);
    return seen;
  };

  it("keeps the picture's own colour, screening its light between two steps", () => {
    for (const scene of ["day", "night"] as const) {
      const out = print(flat([60, 110, 190]), frame(40, 40, 40), scene);
      const seen = colours(out);
      expect(seen.size).toBe(2);
      for (const colour of seen) {
        const [r, g, b] = colour.split(",").map(Number) as [number, number, number];
        expect(r / b).toBeCloseTo(60 / 190, 1);
        expect(g / b).toBeCloseTo(110 / 190, 1);
      }
    }
  });

  it("ends in the paper exactly, with no screen left in it", () => {
    for (const scene of ["day", "night"] as const) {
      const f = frame(40, 40, 10);
      const out = print(flat([120, 160, 220]), f, scene);
      const lastRow = (f.rows - 1) * f.columns * 4;
      for (let i = lastRow; i < out.length; i += 4)
        expect([out[i], out[i + 1], out[i + 2]]).toEqual([...paperOf(scene)]);
    }
  });

  it("holds the day sky still behind the type and nowhere else", () => {
    const f = { ...frame(60, 60, 60), quiet: { x: 30, y: 30, rx: 12, ry: 12 } };
    const out = print(flat([255, 255, 255], 4, 4), f, "day");
    const at = (x: number, y: number) =>
      out[(Math.floor(y / 1.5) * f.columns + Math.floor(x / 1.5)) * 4]!;
    expect(at(30, 30)).toBeLessThan(255);
    expect(at(3, 3)).toBe(255);
  });

  it("reads the light at the edge band by band", () => {
    const rgba = new Uint8ClampedArray(4 * 4 * 4).fill(200);
    expect(edgeLight(rgba, 4, 4, 2)).toEqual([
      [200, 200, 200],
      [200, 200, 200],
    ]);
  });
});
