import { expect, test } from "vitest";
import { callback, callbackSource } from "$shared/testing/svelte-handlers";

test("Escape cannot natively close a controlled dialog whose owner vetoes dismissal", () => {
  const file = new URL("../components/account/AccountDialog.svelte", import.meta.url);
  const source = callbackSource(file, "event", "oncancel");
  let pending = true;
  let nativeOpen = true;
  let controlledOpen = true;
  const context = {
    get open() {
      return controlledOpen;
    },
    set open(value: boolean) {
      if (!pending) controlledOpen = value;
    },
  };
  const escape = () => {
    const event = new Event("cancel", { cancelable: true });
    if (source) callback<(event: Event) => void>(source, context)(event);
    if (!event.defaultPrevented) nativeOpen = false;
    // The production effect closes only after the controlled binding accepts.
    if (!context.open) nativeOpen = false;
  };
  escape();
  expect(nativeOpen).toBe(true);
  pending = false;
  escape();
  expect(controlledOpen).toBe(false);
  expect(nativeOpen).toBe(false);
});
