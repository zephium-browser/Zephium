import { expect, test } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { PICTURE, placeSky } from "..";
import SkyPrint from "../SkyPrint.svelte";

function props(width: number, height: number, onready: () => void) {
  const horizon = Math.round(height * 0.72);
  return {
    scene: "day" as const,
    width,
    height,
    sky: placeSky(width, horizon),
    horizon,
    pictureHorizon: PICTURE.horizon,
    onready,
  };
}

async function mount(width: number, height: number) {
  let ready = 0;
  const screen = await render(
    SkyPrint,
    props(width, height, () => ready++),
  );
  await expect.poll(() => ready, { timeout: 10_000 }).toBeGreaterThan(0);
  return { screen, ready: () => ready };
}

const parts = (root: HTMLElement) => ({
  window: root.querySelector<HTMLElement>(".sky-print-window")!,
  canvas: root.querySelector<HTMLCanvasElement>("canvas.sky-print")!,
});

function shown(root: HTMLElement) {
  const { window, canvas } = parts(root);
  const box = window.getBoundingClientRect();
  const scale = canvas.width / canvas.getBoundingClientRect().width;
  const columns = Math.round(box.width * scale);
  const rows = Math.round(box.height * scale);
  const pixels = canvas.getContext("2d")!.getImageData(0, 0, columns, rows).data;
  return { width: box.width, height: box.height, scale, columns, rows, pixels };
}

test("a sky that shrinks shows exactly what a fresh print at its new size shows", async () => {
  const wide = await mount(936, 784);
  const before = parts(wide.screen.container);
  const backing = [before.canvas.width, before.canvas.height];
  const count = wide.ready();
  await wide.screen.rerender(props(776, 784, () => {}));
  await expect.poll(() => before.window.getBoundingClientRect().width).toBeLessThan(800);
  await new Promise((done) => setTimeout(done, 300));
  expect([before.canvas.width, before.canvas.height]).toEqual(backing);
  const reused = shown(wide.screen.container);
  wide.screen.unmount();
  expect(count).toBeGreaterThan(0);

  const fresh = await mount(776, 784);
  const printed = shown(fresh.screen.container);
  expect([reused.width, reused.height, reused.columns, reused.rows]).toEqual([
    printed.width,
    printed.height,
    printed.columns,
    printed.rows,
  ]);
  expect(reused.scale).toBeCloseTo(printed.scale, 6);
  expect(reused.pixels).toEqual(printed.pixels);
  fresh.screen.unmount();
});

test("a sky that grows within the window keeps its backing and matches a fresh print", async () => {
  await page.viewport(1200, 800);
  const narrow = await mount(776, 784);
  const before = parts(narrow.screen.container);
  const backing = [before.canvas.width, before.canvas.height];
  await narrow.screen.rerender(props(936, 784, () => {}));
  await expect.poll(() => before.window.getBoundingClientRect().width).toBeGreaterThan(900);
  await new Promise((done) => setTimeout(done, 300));
  expect([before.canvas.width, before.canvas.height]).toEqual(backing);
  const grown = shown(narrow.screen.container);
  narrow.screen.unmount();

  const fresh = await mount(936, 784);
  const printed = shown(fresh.screen.container);
  expect([grown.columns, grown.rows]).toEqual([printed.columns, printed.rows]);
  expect(grown.pixels).toEqual(printed.pixels);
  fresh.screen.unmount();
});
