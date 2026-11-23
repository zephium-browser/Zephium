import type {
  WorkRecordMediaRefV1,
  MediaAssetV1_Deserialize as MediaAssetV1,
} from "$shared/ipc/bindings";
import { IS_MAC } from "$shared/platform";

export type { MediaAssetV1 };

const DIGEST = /^[0-9a-f]{64}$/;
const PROFILE = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const STEP_ID = /^[A-Za-z0-9_-]{1,64}$/;
const base = IS_MAC ? "zephium-media://localhost/" : "http://zephium-media.localhost/";

/** The newest frame of one agent page; the generation only busts caches. */
export function pageFrameUrl(attempt: string, step: string, generation: number): string | null {
  if (!STEP_ID.test(attempt) || !STEP_ID.test(step) || !Number.isInteger(generation)) return null;
  return `${base}frame/${attempt}/${step}/${generation}`;
}

/**
 * The privileged media route for one admitted blob. Rust serves only images
 * from the profile's own store to main chrome; anything else is a 404. It
 * sends the picture scaled down to `width` pixels when it is wider, so a card
 * decodes what it shows and not the 1600 px the store keeps; a large view asks
 * for more.
 */
export function mediaUrl(profile: string, digest: string, width = 720): string | null {
  if (!DIGEST.test(digest) || !PROFILE.test(profile)) return null;
  return `${base}${profile}/${digest}?w=${width}`;
}

export function mediaSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** The host serves this exact immutable profile and record; it never fetches a URL. */
export function recordMediaUrl(media: WorkRecordMediaRefV1): string | null {
  if (
    !PROFILE.test(media.profile) ||
    !DIGEST.test(media.source) ||
    !DIGEST.test(media.digest) ||
    media.width < 1 ||
    media.width > 320 ||
    media.height < 1 ||
    media.height > 240
  )
    return null;
  const encoded = btoa(JSON.stringify(media))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/u, "");
  return `${base}record/${encoded}`;
}
export type WorkPicture = { profile: string; digest: string; record?: WorkRecordMediaRefV1 };
export function pictureUrl(picture: WorkPicture, width = 720): string | null {
  return picture.record
    ? recordMediaUrl(picture.record)
    : mediaUrl(picture.profile, picture.digest, width);
}
