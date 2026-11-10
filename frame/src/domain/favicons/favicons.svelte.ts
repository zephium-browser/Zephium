import { SvelteMap } from "svelte/reactivity";
import type { FaviconEntry, IconRef } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { createLifecycle, listenAll, type Unlisten } from "$shared/lib/lifecycle";

const SIDE = 32;
const BYTE_LENGTH = SIDE * SIDE * 4;
const BASE64_LENGTH = Math.ceil(BYTE_LENGTH / 3) * 4;

// Native tracks which rasters this surface holds and sends only what it has
// not delivered, and it never learns of an eviction here. Raster keys here are
// content-addressed, so a surface never drops an icon on a profile change;
// twice native's own bound of 512 keeps every icon it still believes
// delivered, since at most that many rasters can come and go meanwhile.
const CAPACITY = 1024;

/** How a mark sits against a surface. A near-neutral icon the colour of the
 *  chrome behind it reads as an empty square; a coloured one never does,
 *  however dark it is, which is why chroma decides alongside luminance. */
export type IconTone = "dark" | "light" | "mid";

const DARK_LUMINANCE = 70;
const LIGHT_LUMINANCE = 200;
const NEUTRAL_CHROMA = 40;

type Entry = { image: ImageData; tone: IconTone };

/** Keyed by origin and revision. A revision names exact pixels, so profiles
 *  holding different icons for one site never collide, and a profile that
 *  comes back finds what native will not resend. */
const rasters = new SvelteMap<string, Entry>();
/** The raster each profile last received for a site, for addresses that carry
 *  no reference of their own. Lookups use the profile the surface is showing. */
const sites = new SvelteMap<string, string>();
let owner = $state<string | null>(null);
const lifecycle = createLifecycle();
let initializing: Promise<void> | null = null;
let unlisten: Unlisten | null = null;

/** Site renderers decode page-controlled image formats. This surface accepts
 *  one fixed-size RGBA buffer and paints it directly. */
function decode(encoded: string): ImageData | null {
  if (encoded.length !== BASE64_LENGTH || typeof ImageData === "undefined") return null;
  try {
    const binary = atob(encoded);
    if (binary.length !== BYTE_LENGTH) return null;
    const bytes = new Uint8ClampedArray(BYTE_LENGTH);
    for (let index = 0; index < BYTE_LENGTH; index += 1) bytes[index] = binary.charCodeAt(index);
    return new ImageData(bytes, SIDE, SIDE);
  } catch {
    return null;
  }
}

/** Measured over the opaque pixels only, so a small mark on a transparent
 *  field is judged by the mark rather than by the empty space around it. */
function toneOf(image: ImageData): IconTone {
  const pixels = image.data;
  let luminance = 0;
  let chroma = 0;
  let opaque = 0;
  for (let index = 0; index < pixels.length; index += 4) {
    if (pixels[index + 3]! < 32) continue;
    const red = pixels[index]!;
    const green = pixels[index + 1]!;
    const blue = pixels[index + 2]!;
    opaque += 1;
    luminance += 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    chroma += Math.max(red, green, blue) - Math.min(red, green, blue);
  }
  if (opaque === 0) return "mid";
  const meanLuminance = luminance / opaque;
  const meanChroma = chroma / opaque;
  if (meanChroma >= NEUTRAL_CHROMA) return "mid";
  if (meanLuminance < DARK_LUMINANCE) return "dark";
  return meanLuminance > LIGHT_LUMINANCE ? "light" : "mid";
}

const refKey = (origin: string, revision: string) => `${origin}\u0000${revision}`;
const siteKey = (profile: string, origin: string) => `${profile}\u0000${origin}`;

function trim(map: SvelteMap<string, unknown>) {
  while (map.size > CAPACITY) {
    const oldest = map.keys().next();
    if (oldest.done) break;
    map.delete(oldest.value);
  }
}

function accept(profile: string, entries: readonly FaviconEntry[]) {
  owner = profile;
  for (const entry of entries) {
    const key = refKey(entry.origin, entry.revision);
    const held = rasters.get(key);
    const image = held ? held.image : decode(entry.rgba);
    if (!image) continue;
    rasters.delete(key);
    rasters.set(key, held ?? { image, tone: toneOf(image) });
    sites.delete(siteKey(profile, entry.origin));
    sites.set(siteKey(profile, entry.origin), key);
  }
  trim(rasters);
  trim(sites);
}

/** The pixels behind a projected reference, or null until they arrive. */
export function image(ref: IconRef | null | undefined): ImageData | null {
  if (!ref) return null;
  return rasters.get(refKey(ref.origin, ref.revision))?.image ?? null;
}

/** Whatever raster the shown profile already holds for a page's site, at any
 *  revision. For a stored address with no live reference of its own, such as a
 *  task's linked page; it asks native for nothing, so it may be null. */
export function forPage(url: string): { image: ImageData; tone: IconTone } | null {
  const parts = /^(https?):\/\/(?:[^@/?#]*@)?([^/?#:]+)(?::(\d+))?/iu.exec(url);
  if (!parts) return null;
  const scheme = parts[1]!.toLowerCase();
  const port = parts[3] && parts[3] !== (scheme === "https" ? "443" : "80") ? `:${parts[3]}` : "";
  if (owner === null) return null;
  const key = sites.get(siteKey(owner, `${scheme}://${parts[2]!.toLowerCase()}${port}`));
  const entry = key === undefined ? undefined : rasters.get(key);
  return entry ? { image: entry.image, tone: entry.tone } : null;
}

/** A listed address's mark: the reference native sent with the listing, else
 *  whatever has arrived for its site since. Native fetches what a listing
 *  lacks, so a row fills in without being listed again. */
export function mark(
  ref: IconRef | null | undefined,
  url: string | null | undefined,
): { image: ImageData; tone: IconTone } | null {
  const held = image(ref);
  if (held) return { image: held, tone: tone(ref) };
  return url ? forPage(url) : null;
}

export function tone(ref: IconRef | null | undefined): IconTone {
  if (!ref) return "mid";
  return rasters.get(refKey(ref.origin, ref.revision))?.tone ?? "mid";
}

export function init(): Promise<void> {
  if (initializing) return initializing;
  const generation = lifecycle.begin();
  initializing = listenAll([
    events.favicons.listen((event) => {
      if (lifecycle.isCurrent(generation)) {
        accept(event.payload.profile_id, event.payload.entries);
      }
    }),
  ])
    .then((listeners) => {
      if (!lifecycle.isCurrent(generation)) {
        for (const stop of listeners) stop();
        return;
      }
      unlisten = () => {
        for (const stop of listeners) stop();
      };
    })
    .catch((error: unknown) => {
      initializing = null;
      throw error;
    });
  return initializing;
}

export function dispose() {
  lifecycle.end();
  initializing = null;
  unlisten?.();
  unlisten = null;
  owner = null;
  rasters.clear();
  sites.clear();
}
