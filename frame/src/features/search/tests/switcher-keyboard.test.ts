import { expect, test, vi } from "vitest";
import { callback, callbackSource } from "$shared/testing/svelte-handlers";

test("explicit switcher grid navigation includes New Tab without changing CtrlTab cycling", () => {
  const source = callbackSource(
    new URL("../components/TabSwitcher.svelte", import.meta.url),
    "function",
    "keydown",
  )!;
  const send = vi.fn();
  const snapshot = { selected: 0, keyboard: false, entries: [{}, {}] };
  const run = callback<(event: object) => void>(source, {
    snapshot,
    frozen: false,
    across: 2,
    count: 3,
    send,
  });
  const event = (key: string) => ({
    key,
    isComposing: false,
    preventDefault: vi.fn(),
    stopImmediatePropagation: vi.fn(),
  });
  run(event("ArrowDown"));
  expect(send).toHaveBeenLastCalledWith({ type: "select", index: 2 });
  snapshot.selected = 2;
  run(event("Enter"));
  expect(send).toHaveBeenLastCalledWith({ type: "commit" });
  snapshot.keyboard = true;
  snapshot.selected = 0;
  run(event("Tab"));
  expect(send).toHaveBeenLastCalledWith({ type: "step", reverse: undefined });
});
