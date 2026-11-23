import "$styles/global.css";
import type { Component } from "svelte";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import PartsCanvas from "./PartsCanvas.svelte";
import { checkPage, checkScene } from "./needs-you-look";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ faviconProbe: async () => true });
});

// Every time the connection part is built, counted, around the real one.
const built = vi.hoisted(() => ({ count: 0 }));
vi.mock("../components/parts/ConnectionPart.svelte", async (original) => {
  const real = (await original<{ default: Component<Record<string, unknown>> }>()).default;
  const counted: Component<Record<string, unknown>> = (anchor, props) => {
    built.count += 1;
    return real(anchor, props);
  };
  return { default: counted };
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

const signIn = (remaining: number) => ({ ...checkPage(remaining), reason: "sign_in" as const });

test("a helper's view is built once, however often the board measures its row", async () => {
  await page.viewport(1440, 820);
  const target = document.createElement("div");
  target.style.width = "1440px";
  target.style.height = "820px";
  document.body.append(target);
  const screen = await render(PartsCanvas, {
    target,
    props: {
      scene: withGeneration(1),
      viewport: { x: 40, y: 90, zoom: 1 },
      held: [signIn(72_000)],
    },
  });
  await expect.poll(() => screen.container.querySelector(".connection")).not.toBeNull();
  const row = screen.container.querySelector<HTMLElement>("[data-part]:has(.connection)")!;
  const view = row.querySelector(".connection");

  // Each new scene is a new part for every row, and the board measures the rows again.
  for (const generation of [2, 3, 4, 5]) {
    await screen.rerender({
      scene: withGeneration(generation),
      held: [signIn(70_000 - generation)],
    });
    await new Promise((done) => setTimeout(done, 250));
    expect(row.querySelector(".connection")).toBe(view);
  }
  expect(built.count).toBe(1);
  await screen.unmount();
});
