import { describe, expect, it } from "vitest";
import { dotSize, edgeLight, frameFor, INKS, pictureOf, printSky } from "../dither";

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
    f: ReturnType<typeof frame>,
    scene: "day" | "night",
  ) => {
    const out = new Uint8ClampedArray(f.columns * f.rows * 4);
    printSky(out, picture, INKS[scene], f);
    return out;
  };
  const mean = (out: Uint8ClampedArray, channel: number) => {
    let sum = 0;
    for (let i = channel; i < out.length; i += 4) sum += out[i]!;
    return sum / (out.length / 4);
  };

  it("keeps the picture's own colour on average, in a handful of levels", () => {
    const f = frame(48, 48, 48);
    const out = print(flat([123, 185, 245]), f, "night");
    expect(mean(out, 0)).toBeCloseTo(123, -1);
    expect(mean(out, 1)).toBeCloseTo(185, -1);
    expect(mean(out, 2)).toBeCloseTo(245, -1);
    const reds = new Set<number>();
    for (let i = 0; i < out.length; i += 4) reds.add(out[i]!);
    expect(reds.size).toBe(2);
  });

  it("ends in the paper exactly, with no screen left in it", () => {
    for (const scene of ["day", "night"] as const) {
      const f = frame(40, 40, 10);
      const out = print(flat([120, 160, 220]), f, scene);
      const lastRow = (f.rows - 1) * f.columns * 4;
      for (let i = lastRow; i < out.length; i += 4)
        expect([out[i], out[i + 1], out[i + 2]]).toEqual([...INKS[scene].paper]);
    }
  });

  it("holds the day sky to its open blue behind the type and nowhere else", () => {
    const f = { ...frame(60, 60, 60), quiet: { x: 30, y: 30, rx: 12, ry: 12 } };
    const out = print(flat([250, 250, 250], 4, 4), f, "day");
    const at = (x: number, y: number) =>
      out[(Math.floor(y / 1.5) * f.columns + Math.floor(x / 1.5)) * 4]!;
    expect(at(30, 30)).toBeLessThan(220);
    expect(at(3, 3)).toBeGreaterThan(240);
  });

  it("reads the light at the edge band by band", () => {
    const rgba = new Uint8ClampedArray(4 * 4 * 4).fill(200);
    expect(edgeLight(rgba, 4, 4, 2)).toEqual([
      [200, 200, 200],
      [200, 200, 200],
    ]);
  });
});
