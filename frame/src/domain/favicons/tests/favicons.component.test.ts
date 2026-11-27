import { describe, expect, it, afterEach } from "vitest";
import { favicons } from "..";
import { emitNativeEvent } from "$shared/testing/native-events";

const SIDE = 32;
const BYTE_LENGTH = SIDE * SIDE * 4;

function raster(fill: number): string {
  let binary = "";
  for (let index = 0; index < BYTE_LENGTH; index += 1) binary += String.fromCharCode(fill);
  return btoa(binary);
}

/** One opaque colour across the whole square. */
function solid(red: number, green: number, blue: number): string {
  let binary = "";
  for (let index = 0; index < BYTE_LENGTH; index += 4) {
    binary += String.fromCharCode(red, green, blue, 255);
  }
  return btoa(binary);
}

function deliver(entries: { origin: string; revision: string; rgba: string }[], profile = "p") {
  emitNativeEvent("favicons", { surface: "chrome", profile_id: profile, entries });
}

afterEach(() => favicons.dispose());

describe("favicon rasters", () => {
  it("paints an origin once its pixels arrive", async () => {
    await favicons.init();
    const ref = { origin: "https://example.com", revision: "a" };
    expect(favicons.image(ref)).toBeNull();

    deliver([{ origin: ref.origin, revision: "a", rgba: raster(7) }]);

    const image = favicons.image(ref);
    expect(image?.width).toBe(SIDE);
    expect(image?.data[0]).toBe(7);
  });

  it("withholds pixels that belong to a superseded revision", async () => {
    await favicons.init();
    deliver([{ origin: "https://example.com", revision: "a", rgba: raster(7) }]);

    // Native published a new reference; the old pixels must not be drawn for it.
    expect(favicons.image({ origin: "https://example.com", revision: "b" })).toBeNull();

    deliver([{ origin: "https://example.com", revision: "b", rgba: raster(9) }]);
    expect(favicons.image({ origin: "https://example.com", revision: "b" })?.data[0]).toBe(9);
  });

  it("ignores a payload that is not one fixed-size raster", async () => {
    await favicons.init();
    deliver([
      { origin: "https://short.example", revision: "a", rgba: btoa("nope") },
      { origin: "https://bad.example", revision: "a", rgba: "!!!not base64!!!" },
    ]);

    expect(favicons.image({ origin: "https://short.example", revision: "a" })).toBeNull();
    expect(favicons.image({ origin: "https://bad.example", revision: "a" })).toBeNull();
  });

  it("holds at least as many origins as native tracks", async () => {
    await favicons.init();
    const first = { origin: "https://origin-0.example", revision: "a" };
    deliver(
      Array.from({ length: 512 }, (_, index) => ({
        origin: `https://origin-${index}.example`,
        revision: "a",
        rgba: raster(1),
      })),
    );

    // Native only resends what it believes this surface lacks, so evicting
    // inside its own bound would strand an origin permanently.
    expect(favicons.image(first)).not.toBeNull();
  });
});

describe("a stored page's site", () => {
  it("shows whatever the surface holds for the page's origin", async () => {
    await favicons.init();
    deliver([{ origin: "https://github.com", revision: "a", rgba: raster(5) }]);
    expect(favicons.forPage("https://github.com/zephium/browser?tab=1")?.image.data[0]).toBe(5);
    expect(favicons.forPage("HTTPS://user@GitHub.com:443/")?.image.data[0]).toBe(5);
    expect(favicons.forPage("https://github.com:8443/")).toBeNull();
    expect(favicons.forPage("https://gitlab.com/")).toBeNull();
    expect(favicons.forPage("file:///tmp/page.html")).toBeNull();
  });
});

describe("a page's own icon", () => {
  it("is painted for its reference and does not replace the site's", async () => {
    await favicons.init();
    const page = { origin: "https://example.com/sheets/d/1", revision: "p" };
    deliver([
      { origin: "https://example.com", revision: "a", rgba: raster(5) },
      { origin: page.origin, revision: page.revision, rgba: raster(9) },
    ]);

    expect(favicons.image(page)?.data[0]).toBe(9);
    expect(favicons.image({ origin: "https://example.com", revision: "a" })?.data[0]).toBe(5);
    expect(favicons.forPage("https://example.com/sheets/d/1")?.image.data[0]).toBe(5);
  });
});

describe("icon tone", () => {
  // Measured from real cached favicons: GitHub's mark averages luminance 22 at
  // chroma 2, while YouTube's averages luminance 69 at chroma 240. Only the
  // first is invisible on dark chrome.
  it("marks a near-neutral dark mark, and leaves a coloured one alone", async () => {
    await favicons.init();
    deliver([
      { origin: "https://neutral-dark.example", revision: "a", rgba: solid(20, 21, 22) },
      { origin: "https://coloured-dark.example", revision: "a", rgba: solid(255, 0, 0) },
      { origin: "https://neutral-light.example", revision: "a", rgba: solid(238, 238, 238) },
      { origin: "https://midtone.example", revision: "a", rgba: solid(128, 128, 128) },
    ]);

    expect(favicons.tone({ origin: "https://neutral-dark.example", revision: "a" })).toBe("dark");
    expect(favicons.tone({ origin: "https://coloured-dark.example", revision: "a" })).toBe("mid");
    expect(favicons.tone({ origin: "https://neutral-light.example", revision: "a" })).toBe("light");
    expect(favicons.tone({ origin: "https://midtone.example", revision: "a" })).toBe("mid");
  });

  it("judges the mark rather than the empty field around it", async () => {
    await favicons.init();
    // A small dark mark on a transparent square: averaging every pixel would
    // read as transparent-neutral and miss it.
    let binary = "";
    for (let index = 0; index < BYTE_LENGTH; index += 4) {
      const pixel = index / 4;
      const inside = pixel % 32 < 4 && pixel < 32 * 4;
      binary += inside ? String.fromCharCode(16, 16, 16, 255) : String.fromCharCode(0, 0, 0, 0);
    }
    deliver([{ origin: "https://sparse.example", revision: "a", rgba: btoa(binary) }]);

    expect(favicons.tone({ origin: "https://sparse.example", revision: "a" })).toBe("dark");
  });

  it("has no opinion about an origin whose pixels have not arrived", async () => {
    await favicons.init();
    expect(favicons.tone({ origin: "https://unknown.example", revision: "a" })).toBe("mid");
    expect(favicons.tone(null)).toBe("mid");
  });
});

describe("profile switches", () => {
  const ref = { origin: "https://example.com", revision: "a" };

  it("keeps a profile's icons while another is shown, since native will not resend them", async () => {
    await favicons.init();
    deliver([{ origin: ref.origin, revision: "a", rgba: raster(7) }], "personal");
    deliver([{ origin: "https://other.example", revision: "a", rgba: raster(9) }], "private");
    deliver([], "personal");

    expect(favicons.image(ref)?.data[0]).toBe(7);
    expect(favicons.image({ origin: "https://other.example", revision: "a" })?.data[0]).toBe(9);
  });

  it("gives each profile its own icon for one site", async () => {
    await favicons.init();
    deliver([{ origin: ref.origin, revision: "a", rgba: raster(7) }], "personal");
    deliver([{ origin: ref.origin, revision: "b", rgba: raster(9) }], "work");
    deliver([], "personal");

    expect(favicons.image(ref)?.data[0]).toBe(7);
    expect(favicons.image({ origin: ref.origin, revision: "b" })?.data[0]).toBe(9);
    expect(favicons.forPage("https://example.com/inbox")?.image.data[0]).toBe(7);
    deliver([], "work");
    expect(favicons.forPage("https://example.com/inbox")?.image.data[0]).toBe(9);
  });

  it("never shows another profile's icon for a stored address", async () => {
    await favicons.init();
    deliver([{ origin: "https://secret.example", revision: "a", rgba: raster(7) }], "private");
    deliver([], "personal");

    expect(favicons.forPage("https://secret.example/")).toBeNull();
    deliver([], "private");
    expect(favicons.forPage("https://secret.example/")?.image.data[0]).toBe(7);
  });

  it("survives A to B to A with no resend", async () => {
    await favicons.init();
    const entries = Array.from({ length: 40 }, (_, index) => ({
      origin: `https://site-${index}.example`,
      revision: "a",
      rgba: raster(index + 1),
    }));
    deliver(entries, "a");
    deliver(
      entries.slice(0, 10).map((entry) => ({ ...entry, revision: "b", rgba: raster(200) })),
      "b",
    );
    deliver([], "a");

    for (const entry of entries) {
      expect(favicons.image({ origin: entry.origin, revision: "a" })).not.toBeNull();
    }
  });
});
