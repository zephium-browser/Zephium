import { afterEach, describe, expect, it, vi } from "vitest";
import { decode, isDecoded, SCENES, skyImage } from "../scenes";

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

describe("decode", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** Images that record each picture they decode, failing the first `fails`. */
  function images(fails = 0) {
    const decoded: string[] = [];
    let left = fails;
    vi.stubGlobal(
      "Image",
      class {
        src = "";
        decode() {
          decoded.push(this.src);
          if (left > 0) {
            left -= 1;
            return Promise.reject(new Error("undecodable"));
          }
          return Promise.resolve();
        }
      },
    );
    return decoded;
  }

  it("decodes each picture once, and a narrower one never stands in for a wider one", async () => {
    const decoded = images();
    const narrow = skyImage("day", 1000, 1);
    const wide = skyImage("day", 2400, 1);

    await Promise.all([decode("day", narrow), decode("day", narrow)]);
    expect(isDecoded(narrow)).toBe(true);
    expect(isDecoded(wide)).toBe(false);

    await decode("day", wide);
    await decode("day", narrow);
    expect(isDecoded(wide)).toBe(true);
    expect(decoded).toEqual([narrow, wide]);
  });

  it("tries a picture again after its decode failed", async () => {
    const decoded = images(1);
    const url = skyImage("night", 1000, 1);

    await expect(decode("night", url)).rejects.toThrow("undecodable");
    expect(isDecoded(url)).toBe(false);

    await decode("night", url);
    expect(isDecoded(url)).toBe(true);
    expect(decoded).toEqual([url, url]);
  });

  it("draws from the narrowest decoded picture that covers the sky, else the widest decoded one", async () => {
    images();
    // A session of its own, with nothing decoded yet.
    vi.resetModules();
    const { decode, decodedFor, SCENES, skyImage } = await import("../scenes");
    expect(decodedFor("night", 1000, 1)).toBeNull();
    const middle = skyImage("night", 2400, 1);
    await decode("night", middle);
    expect(decodedFor("night", 1000, 1)).toEqual({ url: middle, covers: true });
    expect(decodedFor("night", 3000, 1)).toEqual({ url: middle, covers: false });
    await decode("night", SCENES.night);
    expect(decodedFor("night", 3000, 1)).toEqual({ url: SCENES.night, covers: true });
    expect(decodedFor("night", 2000, 1)).toEqual({ url: middle, covers: true });
  });
});
