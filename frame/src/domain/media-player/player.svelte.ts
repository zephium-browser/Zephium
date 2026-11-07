import type { MediaPlayerAction, MediaPlayerView } from "$shared/ipc/bindings";
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { settle } from "$domain/operations";

type Held = { view: MediaPlayerView; at: number };

let held = $state.raw<Held | null>(null);
let lifecycle = 0;
let initializing: Promise<void> | null = null;
let unlisten: (() => void) | null = null;

/** The sidebar's player, or null while there is none. */
export const view = () => held?.view ?? null;

/**
 * Where playback is at `now` (a `performance.now()` time): native sends the
 * position as of each change, and it runs on from there while playing.
 */
export function position(now: number): number {
  if (!held) return 0;
  const { view, at } = held;
  const from = view.position ?? 0;
  const moving = view.playing && !view.waiting;
  const ran = moving ? Math.max(0, (now - at) / 1000) * (view.rate ?? 1) : 0;
  return view.duration == null ? from + ran : Math.min(view.duration, from + ran);
}

export function init(): Promise<void> {
  if (initializing) return initializing;
  const generation = ++lifecycle;
  initializing = events.mediaPlayerChanged
    .listen((event) => {
      if (generation === lifecycle)
        held = event.payload ? { view: event.payload, at: performance.now() } : null;
    })
    .then((stop) => {
      if (generation === lifecycle) unlisten = stop;
      else stop();
    });
  return initializing;
}

export function dispose() {
  lifecycle += 1;
  initializing = null;
  unlisten?.();
  unlisten = null;
  held = null;
}

function act(action: MediaPlayerAction) {
  const id = held?.view.tab_id;
  if (id) void settle(commands.mediaPlayer(id, action));
}

export const play = () => act({ kind: "play" });
export const pause = () => act({ kind: "pause" });
export const seek = (seconds: number) => act({ kind: "seek", seconds });
export const pictureInPicture = () => act({ kind: "picture_in_picture" });
/** Puts the player away until its tab plays something else. */
export const close = () => act({ kind: "close" });
/** Brings the player's tab to front, in its own space. */
export const show = () => act({ kind: "show" });
