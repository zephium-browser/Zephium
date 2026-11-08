import { describe, expect, it } from "vitest";
import { dotSize, edgeLight, frameFor, lumaOf, PALETTES, paperOf, printSky } from "../dither";

const flat = (value: number, width = 8, height = 8) => {
  const rgba = new Uint8ClampedArray(width * height * 4).fill(value);
  return lumaOf(rgba, width, height);
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

  it("prints only the palette's tones", () => {
    const rgba = new Uint8ClampedArray(64 * 64 * 4);
    for (let i = 0; i < rgba.length; i += 4) rgba.fill((i / 4) % 256, i, i + 3);
    const luma = lumaOf(rgba, 64, 64);
    const f = frame(40, 40, 40);
    const out = new Uint8ClampedArray(f.columns * f.rows * 4);
    printSky(out, luma, PALETTES.night, f);
    const tones = new Set(PALETTES.night.tones.map((tone) => tone.join()));
    for (let i = 0; i < out.length; i += 4)
      expect(tones.has(`${out[i]},${out[i + 1]},${out[i + 2]}`)).toBe(true);
  });

  it("dissolves into the paper by the foot of the page", () => {
    const luma = { ...flat(255), low: 0, high: 255 };
    const f = frame(40, 40, 10);
    const out = new Uint8ClampedArray(f.columns * f.rows * 4);
    printSky(out, luma, PALETTES.night, f);
    const lastRow = (f.rows - 1) * f.columns * 4;
    expect([out[lastRow], out[lastRow + 1], out[lastRow + 2]]).toEqual([...paperOf("night")]);
  });

  it("reads the light at the edge band by band", () => {
    const rgba = new Uint8ClampedArray(4 * 4 * 4).fill(200);
    const bands = edgeLight(rgba, 4, 4, 2);
    expect(bands).toEqual([
      [200, 200, 200],
      [200, 200, 200],
    ]);
  });
});
