import { commands } from "$shared/ipc/bindings";

/**
 * Native page frames retain their bounded original pixels for screen DPI and
 * canvas zoom; photos have small copies at the size the canvas shows them.
 * The canvas keeps decoded copies within a byte budget, and the
 * webview's own cache of pictures is emptied once the works that used them
 * are left or the screen has been still.
 */
const BUDGET = 16 * 1024 * 1024;
const MEDIA = /^(?:zephium-media:\/\/localhost|http:\/\/zephium-media\.localhost(?::\d+)?)\//u;
const FRAME =
  /^(?:zephium-media:\/\/localhost|http:\/\/zephium-media\.localhost(?::\d+)?)\/frame\//u;

/** A picture's address asking for the copy `width` CSS pixels wide, as sharp as the screen shows it. */
export function thumbSrc(url: string, width: number): string {
  if (!MEDIA.test(url) || FRAME.test(url)) return url;
  return `${url}${url.includes("?") ? "&" : "?"}w=${pixelsOf(width)}`;
}
const pixelsOf = (width: number) =>
  Math.ceil(width * Math.min(2, globalThis.devicePixelRatio || 1));

type Kept = { bitmap: Promise<ImageBitmap | null>; bytes: number };
const kept = new Map<string, Kept>();
let held = 0;

/**
 * Drawn in memory, not on the GPU: an accelerated canvas is a compositing
 * layer of its own, and one on the canvas turns the whole transformed canvas
 * into a layer with a backing store the size of everything on it.
 */
const SOFT: CanvasRenderingContext2DSettings = { willReadFrequently: true };

/** Lets go of a canvas's pixels now rather than whenever the collector gets to it. */
function drop(canvas: HTMLCanvasElement) {
  canvas.width = 0;
  canvas.height = 0;
}

async function decode(url: string, width: number): Promise<ImageBitmap | null> {
  const image = new Image();
  image.decoding = "async";
  image.src = thumbSrc(url, width);
  let scratch: HTMLCanvasElement | null = null;
  try {
    await image.decode();
    if (!image.naturalWidth) return null;
    // The native frame is already bounded to 640 pixels and 1 MiB. Decode it
    // once per generation, without a second resize or a lossy JPEG round trip.
    if (FRAME.test(url)) return await createImageBitmap(image);
    const scale = Math.min(1, pixelsOf(width) / image.naturalWidth);
    scratch = document.createElement("canvas");
    scratch.width = Math.max(1, Math.round(image.naturalWidth * scale));
    scratch.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const context = scratch.getContext("2d", SOFT);
    if (!context) return null;
    context.imageSmoothingQuality = "high";
    context.drawImage(image, 0, 0, scratch.width, scratch.height);
    return await createImageBitmap(scratch);
  } catch {
    return null;
  } finally {
    image.src = "";
    if (scratch) drop(scratch);
  }
}

let shown = 0;
let releasing: ReturnType<typeof setTimeout> | undefined;
let releasedAt = 0;
let waking: AbortController | undefined;
/** How long after the last canvas that drew a picture is gone, and after pictures were last decoded with some still on show, the copies go. */
const LEFT_MS = 4000;
const STILL_MS = 60_000;

async function releaseNativeMemory() {
  waking?.abort();
  const listeners = new AbortController();
  waking = listeners;
  let idle = true;
  const restore = () => {
    void commands.workReleaseMemory(false).catch(() => false);
  };
  const wake = () => {
    idle = false;
    listeners.abort();
    restore();
  };
  for (const event of ["pointermove", "pointerdown", "wheel", "keydown", "focusin"]) {
    document.addEventListener(event, wake, {
      capture: true,
      passive: true,
      signal: listeners.signal,
    });
  }
  try {
    await commands.workReleaseMemory(true);
    // Activity may arrive before the native idle request has settled.
    if (!idle) restore();
  } catch {
    listeners.abort();
  }
}

function releaseLater(after: number) {
  clearTimeout(releasing);
  releasing = setTimeout(() => {
    releasing = undefined;
    for (const entry of kept.values()) void entry.bitmap.then((bitmap) => bitmap?.close());
    kept.clear();
    held = 0;
    if (Date.now() - releasedAt < 10_000) return;
    releasedAt = Date.now();
    try {
      void releaseNativeMemory();
    } catch {
      // Off the app (a test page), there is no webview to ask.
    }
  }, after);
}

function evict() {
  for (const [key, entry] of kept) {
    if (held <= BUDGET) return;
    kept.delete(key);
    held -= entry.bytes;
    void entry.bitmap.then((bitmap) => bitmap?.close());
  }
}

/** The frame's thumbnail at a width, made once and kept among the most recently used. */
function frameThumbnail(url: string, width: number): Promise<ImageBitmap | null> {
  const key = FRAME.test(url) ? url : `${width}|${url}`;
  const hit = kept.get(key);
  if (hit) {
    kept.delete(key);
    kept.set(key, hit);
    return hit.bitmap;
  }
  releaseLater(shown ? STILL_MS : LEFT_MS);
  const entry: Kept = { bitmap: decode(url, width), bytes: 0 };
  kept.set(key, entry);
  void entry.bitmap.then((bitmap) => {
    if (kept.get(key) !== entry) return;
    if (!bitmap) {
      kept.delete(key);
      return;
    }
    entry.bytes = bitmap.width * bitmap.height * 4;
    held += entry.bytes;
    evict();
  });
  return entry.bitmap;
}

type Thumb = { url: string; width: number; onmissing?: () => void };
/**
 * Draws a picture's small copy into a canvas, and the next one when the
 * picture changes; one that won't load says so. The canvas lets its pixels
 * go as it leaves the page.
 */
export function thumbnail(canvas: HTMLCanvasElement, frame: Thumb) {
  let current = frame.url;
  let width = frame.width;
  let missing = frame.onmissing;
  let gone = false;
  let generation = 0;
  shown += 1;
  const draw = ({ url, width }: Thumb) => {
    const requested = ++generation;
    void frameThumbnail(url, width).then((bitmap) => {
      if (gone || current !== url || requested !== generation) return;
      if (!bitmap) return missing?.();
      try {
        // Keep DOM canvas pixels at their display size too: the shared bitmap
        // budget does not include each card's software backing store.
        const scale = FRAME.test(url) ? Math.min(1, pixelsOf(width) / bitmap.width) : 1;
        canvas.width = Math.max(1, Math.round(bitmap.width * scale));
        canvas.height = Math.max(1, Math.round(bitmap.height * scale));
        const context = canvas.getContext("2d", SOFT);
        if (context) {
          context.imageSmoothingQuality = "high";
          context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        }
      } catch {
        // A copy let go before it was drawn: the window keeps its sheet.
      }
    });
  };
  draw(frame);
  return {
    update(next: Thumb) {
      missing = next.onmissing;
      if (next.url === current && next.width === width) return;
      current = next.url;
      width = next.width;
      draw(next);
    },
    destroy() {
      gone = true;
      shown -= 1;
      if (!shown) releaseLater(LEFT_MS);
      // After any leaving motion has drawn from it.
      setTimeout(() => {
        if (!canvas.isConnected) drop(canvas);
      }, 1000);
    },
  };
}
