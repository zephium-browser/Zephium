import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import PartsCanvas from "./PartsCanvas.svelte";
import { checkPage, checkScene } from "./needs-you-look";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ faviconProbe: async () => true });
});

const signIn = (remaining: number) => ({
  ...checkPage(remaining),
  reason: "sign_in" as const,
});

const withGeneration = (generation: number) => {
  const scene = checkScene("waiting");
  scene.pages = scene.pages.map((each) => ({
    ...each,
    live: true,
    frame: { generation, width: 1280, height: 800 },
  }));
  return scene;
};

const boxes = (row: HTMLElement) =>
  [row, ...row.querySelectorAll<HTMLElement>("*")].map((node) => {
    const box = node.getBoundingClientRect();
    // Keep subpixel movement, discarding only floating-point noise. Whole-pixel
    // rounding turns a half-pixel center's adjacent doubles into a 1px jump.
    return [box.x, box.y, box.width, box.height].map((value) => value.toFixed(6)).join(",");
  });

async function settle() {
  await new Promise((done) => setTimeout(done, 1200));
}

function canvasTarget() {
  const target = document.createElement("div");
  // Mount into the real room: sizing after async render lets camera follow
  // start a glide against the canvas's temporary 240px minimum height.
  target.style.width = "1440px";
  target.style.height = "820px";
  document.body.append(target);
  return target;
}

test("stillness samples keep a half-pixel center across floating-point noise", () => {
  const node = document.createElement("div");
  const rect = vi.spyOn(node, "getBoundingClientRect");
  // The 28px bar's 1px border leaves 27px around its 14px children: y = 504.5.
  rect.mockReturnValue(new DOMRect(634, 504.5 - Number.EPSILON * 504.5, 14, 14));
  const before = boxes(node);
  rect.mockReturnValue(new DOMRect(634, 504.5, 14, 14));
  expect(boxes(node)).toEqual(before);
  rect.mockRestore();
});

test.each([0, 1, 2, 3])("stillness samples detect subpixel motion in coordinate %i", (index) => {
  const node = document.createElement("div");
  const values: [number, number, number, number] = [634, 504.5, 14, 14];
  const rect = vi.spyOn(node, "getBoundingClientRect").mockReturnValue(new DOMRect(...values));
  const before = boxes(node);
  values[index]! += 1 / 64;
  rect.mockReturnValue(new DOMRect(...values));
  expect(boxes(node)).not.toEqual(before);
  rect.mockRestore();
});

test("a sign-in the run waits on holds its page still while frames refresh", async () => {
  await page.viewport(1440, 820);
  const screen = await render(PartsCanvas, {
    target: canvasTarget(),
    props: {
      scene: withGeneration(1),
      viewport: { x: 40, y: 90, zoom: 1 },
      held: [signIn(72_000)],
    },
  });
  await expect.poll(() => screen.container.querySelector("[data-part] .window")).not.toBeNull();
  const row = screen.container.querySelector<HTMLElement>("[data-part]:has(.window)")!;
  await settle();
  const matrix = new DOMMatrixReadOnly(
    getComputedStyle(screen.container.querySelector(".svelte-flow__viewport")!).transform,
  );
  expect([matrix.m41, matrix.m42, matrix.a]).toEqual([40, 90, 1]);
  const before = boxes(row);
  const view = screen.container.querySelector(".svelte-flow__viewport")!.getAttribute("style");

  for (const generation of [2, 3, 4]) {
    await screen.rerender({
      scene: withGeneration(generation),
      held: [signIn(70_000 - generation)],
    });
    await new Promise((done) => setTimeout(done, 150));
    expect(row.querySelector(".arrived")).toBeNull();
    expect(boxes(row)).toEqual(before);
  }
  await settle();

  const endless = (root: Element) =>
    root
      .getAnimations({ subtree: true })
      .filter((animation) => animation.playState === "running")
      .filter((animation) => animation.effect?.getComputedTiming().iterations === Infinity)
      .map((animation) => (animation as CSSAnimation).animationName);
  expect(endless(row)).toEqual([]);
  expect(
    row.getAnimations({ subtree: true }).map((each) => (each as CSSAnimation).animationName),
  ).toEqual([]);
  expect(screen.container.querySelector(".svelte-flow__viewport")!.getAttribute("style")).toBe(
    view,
  );
  expect(boxes(row)).toEqual(before);
  await screen.unmount();
});

test("empty space around a part's windows pans the canvas, its windows and name keep the pointer", async () => {
  await page.viewport(1440, 820);
  const screen = await render(PartsCanvas, {
    target: canvasTarget(),
    props: {
      scene: withGeneration(1),
      viewport: { x: 40, y: 90, zoom: 1 },
      held: [signIn(72_000)],
    },
  });
  await expect.poll(() => screen.container.querySelector("[data-part] .page")).not.toBeNull();
  const row = screen.container.querySelector<HTMLElement>("[data-part]:has(.page)")!;
  const node = row.closest<HTMLElement>(".svelte-flow__node")!;
  const nodeBox = node.getBoundingClientRect();
  const pageBox = row.querySelector(".page")!.getBoundingClientRect();
  const at = (x: number, y: number) => document.elementFromPoint(x, y);
  // On the window and on the name: the row's.
  expect(row.contains(at(pageBox.x + 20, pageBox.y + 60))).toBe(true);
  const name = row.querySelector(".name")!.getBoundingClientRect();
  expect(row.contains(at(name.x + 4, name.y + 4))).toBe(true);
  // Below the window and left of it, still inside the row's box: the pane's.
  const below = at(pageBox.x + 20, Math.min(nodeBox.bottom, pageBox.bottom) + 8);
  if (nodeBox.bottom > pageBox.bottom + 8)
    expect(below?.classList.contains("svelte-flow__pane")).toBe(true);
  const gap = at(pageBox.x - 6, pageBox.y + 100);
  expect(gap?.closest("[data-part]")).toBeNull();
  await screen.unmount();
});
