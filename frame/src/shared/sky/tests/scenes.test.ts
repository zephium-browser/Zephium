import { describe, expect, it } from "vitest";
import { SCENES, skyImage } from "../scenes";

describe("skyImage", () => {
  it("draws a sky from the narrowest picture that still covers it", () => {
    const small = skyImage("day", 1170, 1);
    const middle = skyImage("day", 1200, 2);
    const whole = skyImage("day", 1700, 2);
    expect(new Set([small, middle, whole]).size).toBe(3);
    expect(small).toMatch(/sky-day-1920/u);
    expect(middle).toMatch(/sky-day-2560/u);
    expect(whole).toBe(SCENES.day);
  });

  it("never draws one narrower than it is shown", () => {
    expect(skyImage("night", 1920, 1)).toMatch(/sky-night-1920/u);
    expect(skyImage("night", 1921, 1)).toMatch(/sky-night-2560/u);
  });

  it("draws the whole sky past its widest picture", () => {
    expect(skyImage("night", 4000, 2)).toBe(SCENES.night);
  });
});
