import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { mockBindings } from "$shared/testing/bindings";

vi.mock("$shared/ipc/bindings", () => mockBindings({ workReleaseMemory: vi.fn(async () => true) }));

const frame = (generation: number) =>
  `http://zephium-media.localhost/frame/attempt/step/${generation}`;
const pending = new Map<string, Promise<void>>();
let bitmaps: Array<{ width: number; height: number; close: ReturnType<typeof vi.fn> }>;
let images: string[];
let scratch: ReturnType<typeof vi.fn>;
const canvas = () => {
  const drawImage = vi.fn();
  return {
    width: 0,
    height: 0,
    isConnected: false,
    getContext: vi.fn(() => ({ drawImage, imageSmoothingQuality: "low" })),
    drawImage,
  };
};
const settle = async () => {
  for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
};

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  pending.clear();
  bitmaps = [];
  images = [];
  scratch = vi.fn(canvas);
  vi.stubGlobal("devicePixelRatio", 1);
  vi.stubGlobal("document", { createElement: scratch, addEventListener: vi.fn() });
  vi.stubGlobal(
    "Image",
    class {
      src = "";
      decoding = "";
      naturalWidth = 640;
      naturalHeight = 512;
      decode() {
        images.push(this.src);
        return pending.get(this.src) ?? Promise.resolve();
      }
    },
  );
  vi.stubGlobal(
    "createImageBitmap",
    vi.fn(async (source: { width?: number; height?: number }) => {
      const bitmap = {
        width: source.width ?? 640,
        height: source.height ?? 512,
        close: vi.fn(),
      };
      bitmaps.push(bitmap);
      return bitmap;
    }),
  );
});
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

test("only ordinary media pictures request a width; native frames retain their PNG address", async () => {
  const { thumbSrc } = await import("../frame-thumbs");
  const media = "zephium-media://localhost/01M3CTWDG20GFZPACD7905RSRJ/" + "b".repeat(64);
  expect(thumbSrc(media, 224)).toBe(`${media}?w=224`);
  expect(thumbSrc(`${media}?v=2`, 224)).toBe(`${media}?v=2&w=224`);
  expect(thumbSrc(frame(1), 288)).toBe(frame(1));
  expect(thumbSrc("zephium-media://localhost/frame/a/b/1", 288)).toBe(
    "zephium-media://localhost/frame/a/b/1",
  );
  expect(thumbSrc("data:image/png;base64,AAAA", 224)).toBe("data:image/png;base64,AAAA");
  expect(thumbSrc("https://example.com/a.png", 224)).toBe("https://example.com/a.png");
});

test("a native generation decodes once across display widths, without scratch or JPEG resize", async () => {
  const { thumbnail, thumbSrc } = await import("../frame-thumbs");
  const small = canvas();
  const large = canvas();
  thumbnail(small as unknown as HTMLCanvasElement, { url: frame(1), width: 288 });
  thumbnail(large as unknown as HTMLCanvasElement, { url: frame(1), width: 360 });
  await settle();
  expect(images).toEqual([frame(1)]);
  expect(scratch).not.toHaveBeenCalled();
  expect(bitmaps).toHaveLength(1);
  expect(small.width).toBe(288);
  expect(large.width).toBe(360);
  expect(small.drawImage.mock.calls[0]?.[0]).toBe(bitmaps[0]);
  expect(large.drawImage.mock.calls[0]?.[0]).toBe(bitmaps[0]);
  vi.stubGlobal("devicePixelRatio", 2);
  expect(thumbSrc(frame(1), 360)).toBe(frame(1));
  const sharp = canvas();
  thumbnail(sharp as unknown as HTMLCanvasElement, { url: frame(1), width: 360 });
  await settle();
  expect(sharp.width).toBe(640);
  expect(bitmaps).toHaveLength(1);
  expect(thumbSrc("http://zephium-media.localhost/photo", 360)).toMatch(/\?w=720$/u);
});

test("a late older generation cannot draw over the newest page pixels", async () => {
  let release!: () => void;
  pending.set(frame(1), new Promise<void>((resolve) => (release = resolve)));
  const { thumbnail } = await import("../frame-thumbs");
  const shown = canvas();
  const action = thumbnail(shown as unknown as HTMLCanvasElement, { url: frame(1), width: 360 });
  action.update({ url: frame(2), width: 288 });
  await settle();
  const newest = bitmaps[0];
  expect(shown.drawImage).toHaveBeenCalledOnce();
  release();
  await settle();
  expect(shown.drawImage).toHaveBeenCalledOnce();
  expect(shown.drawImage.mock.calls[0]?.[0]).toBe(newest);
  expect(shown.width).toBe(288);
});

test("bitmap eviction closes the least recently used generation within 16 MiB", async () => {
  const { thumbnail } = await import("../frame-thumbs");
  for (let generation = 0; generation < 13; generation += 1) {
    thumbnail(canvas() as unknown as HTMLCanvasElement, { url: frame(generation), width: 360 });
    await settle();
  }
  expect(bitmaps).toHaveLength(13);
  // Twelve 640 x 512 RGBA bitmaps fit; the thirteenth exceeds the byte bound.
  expect(bitmaps[0]?.close).toHaveBeenCalledOnce();
  expect(bitmaps[12]?.close).not.toHaveBeenCalled();
  thumbnail(canvas() as unknown as HTMLCanvasElement, { url: frame(12), width: 288 });
  await settle();
  expect(bitmaps).toHaveLength(13);
  thumbnail(canvas() as unknown as HTMLCanvasElement, { url: frame(0), width: 360 });
  await settle();
  expect(bitmaps).toHaveLength(14);
  expect(bitmaps[1]?.close).toHaveBeenCalledOnce();
});

test("an updated width wins while the same generation is still decoding", async () => {
  let release!: () => void;
  pending.set(frame(1), new Promise<void>((resolve) => (release = resolve)));
  const { thumbnail } = await import("../frame-thumbs");
  const shown = canvas();
  const action = thumbnail(shown as unknown as HTMLCanvasElement, { url: frame(1), width: 360 });
  action.update({ url: frame(1), width: 288 });
  release();
  await settle();
  expect(bitmaps).toHaveLength(1);
  expect(shown.drawImage).toHaveBeenCalledOnce();
  expect(shown.width).toBe(288);
});
