import { afterEach, expect, test, vi } from "vitest";
import { callback, callbackSource } from "$shared/testing/svelte-handlers";
import { everyWhileVisible } from "$shared/lib/visible-interval";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
for (const [path, kind, contains] of [
  ["../../media/components/MediaPlayer.svelte", "effect", "player?.playing"],
  ["../components/FocusCover.svelte", "mount", "now ="],
] as const) {
  test(`${path} stops its clock while hidden and catches up on return`, () => {
    vi.useFakeTimers();
    let visibility = "visible";
    const document = new EventTarget();
    Object.defineProperty(document, "visibilityState", { get: () => visibility });
    vi.stubGlobal("document", document);
    let updates = 0;
    const context = {
      player: { playing: true, waiting: false },
      document,
      everyWhileVisible,
      performance: { now: () => 100 },
      Date,
      setInterval,
      clearInterval,
      get now() {
        return 0;
      },
      set now(_value: number) {
        updates++;
      },
    };
    const source = callbackSource(new URL(path, import.meta.url), kind, "", contains)!;
    const stop = callback<() => () => void>(source, context)();
    vi.advanceTimersByTime(2000);
    expect(updates).toBeGreaterThan(0);
    visibility = "hidden";
    document.dispatchEvent(new Event("visibilitychange"));
    const before = updates;
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(10_000);
    expect(updates).toBe(before);
    visibility = "visible";
    document.dispatchEvent(new Event("visibilitychange"));
    expect(updates).toBe(before + 1);
    stop();
    expect(vi.getTimerCount()).toBe(0);
  });
}
