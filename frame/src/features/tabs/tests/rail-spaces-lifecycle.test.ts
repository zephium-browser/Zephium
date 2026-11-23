import { expect, test } from "vitest";
import { callback, callbackSource } from "$shared/testing/svelte-handlers";

test("destroying an open rail spaces stack removes its window listeners", () => {
  const file = new URL("../components/RailSpaces.svelte", import.meta.url);
  const listeners = new Map<string, Set<unknown>>();
  const context = {
    window: {
      addEventListener(type: string, listener: unknown) {
        const current = listeners.get(type) ?? new Set();
        current.add(listener);
        listeners.set(type, current);
      },
      removeEventListener(type: string, listener: unknown) {
        listeners.get(type)?.delete(listener);
      },
    },
    read: () => {},
    lost: () => {},
    ctrl: false,
  };
  callback<() => void>(callbackSource(file, "function", "listen")!, context)();
  expect([...listeners.values()].reduce((n, set) => n + set.size, 0)).toBe(3);
  const destroy = callbackSource(file, "destroy", "onDestroy");
  if (destroy) callback<() => void>(destroy, context)();
  expect([...listeners.values()].reduce((n, set) => n + set.size, 0)).toBe(0);
});
