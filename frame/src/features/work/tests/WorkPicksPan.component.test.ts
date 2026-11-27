import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import BoardCanvas from "./BoardCanvas.svelte";
// This geometry test starts after the real lazy renderer has loaded. A cold CI
// Vite transform is not canvas readiness and can exceed the poll budget.
import "../components/objects/Picks.svelte";
import { tripScene } from "./board-fixtures";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ faviconProbe: async () => true });
});

test("the gaps of a set of cards belong to the canvas, its cards to the set", async () => {
  await page.viewport(1600, 1000);
  const screen = await render(BoardCanvas, {
    scene: tripScene(),
    viewport: { x: 0, y: 0, zoom: 0.6 },
  });
  screen.container.style.width = "1600px";
  screen.container.style.height = "1000px";
  await expect.poll(() => screen.container.querySelector(".object.picks .set > *")).toBeTruthy();
  const set = screen.container.querySelector(".object.picks .set")!;
  const [first, second] = [...set.children] as HTMLElement[];
  const node = set.closest(".svelte-flow__node")!;
  const a = first!.getBoundingClientRect();
  const b = second!.getBoundingClientRect();
  // Between the first two cards, level with their middle.
  const gap = document.elementFromPoint((a.right + b.left) / 2, a.top + a.height / 2);
  expect(node.contains(gap)).toBe(false);
  const card = document.elementFromPoint(a.left + a.width / 2, a.top + a.height / 2);
  expect(first!.contains(card)).toBe(true);
  await screen.unmount();
});
