import { expect, test } from "vitest";
import { layerRelease } from "../host/layers";

const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));

function canvas() {
  const root = document.createElement("div");
  const viewport = document.createElement("div");
  viewport.className = "svelte-flow__viewport";
  const walker = document.createElement("div");
  viewport.append(walker);
  root.append(viewport);
  document.body.append(root);
  let released = 0;
  new MutationObserver((records) => {
    released += records.filter((record) => record.addedNodes.length).length;
  }).observe(viewport, { childList: true });
  return { root, walker, released: () => released };
}

test("the canvas lets its layer go once motion settles, never while a helper still moves", async () => {
  const { root, walker, released } = canvas();
  const release = layerRelease(root);
  const walk = walker.animate([{ translate: "0 0" }, { translate: "40px 0" }], {
    duration: 400,
    iterations: Infinity,
  });
  release.moved();
  await wait(700);
  expect(released()).toBe(0);
  walk.cancel();
  await wait(400);
  expect(released()).toBe(1);
  expect(root.querySelectorAll(".svelte-flow__viewport > *")).toHaveLength(1);

  const arrive = walker.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 300 });
  release.moved();
  await wait(200);
  expect(released()).toBe(1);
  await arrive.finished;
  await wait(400);
  expect(released()).toBe(2);

  release.destroy();
  release.moved();
  walker.dispatchEvent(new Event("transitionend", { bubbles: true }));
  await wait(400);
  expect(released()).toBe(2);
  root.remove();
});

test("a moving canvas holds its layers, and lets them go once it stops or the window hides", async () => {
  const { root, released } = canvas();
  const viewport = root.querySelector<HTMLElement>(".svelte-flow__viewport")!;
  const release = layerRelease(root);
  // A pointer resting on the canvas, or crossing it, holds nothing.
  root.dispatchEvent(new PointerEvent("pointermove", { bubbles: true }));
  expect(viewport.dataset.held).toBeUndefined();
  // A pan step does, and it is let go a moment after the canvas stops.
  root.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaX: 4 }));
  expect(viewport.style.willChange).toBe("transform");
  expect(viewport.dataset.held).toBe("");
  await wait(1400);
  expect(viewport.dataset.held).toBeUndefined();
  // A drag holds them too.
  root.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, buttons: 1 }));
  expect(viewport.style.willChange).toBe("transform");
  expect(viewport.dataset.held).toBe("");
  const before = released();
  release.moved();
  await wait(400);
  // Held: nothing lets it go while the canvas may still move.
  expect(released()).toBe(before);
  Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
  document.dispatchEvent(new Event("visibilitychange"));
  expect(viewport.style.willChange).toBe("");
  expect(viewport.dataset.held).toBeUndefined();
  await wait(400);
  expect(released()).toBe(before + 1);
  Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
  release.destroy();
  root.remove();
});
