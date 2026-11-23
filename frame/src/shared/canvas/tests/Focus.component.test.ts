import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import FocusStage from "./FocusStage.svelte";

const stopsOf = (root: HTMLElement) =>
  [...root.querySelectorAll<HTMLElement>("[data-canvas-stop]")].map((element) => ({
    id: element.dataset.canvasStop,
    tab: element.tabIndex,
  }));

test("one stop is in the tab order; arrows move by position, Enter acts, Escape leaves", async () => {
  await page.viewport(1000, 700);
  const onactivate = vi.fn();
  const onleave = vi.fn();
  const onmove = vi.fn();
  const screen = await render(FocusStage, { keyboard: { onactivate, onleave, onmove } });
  const group = screen.getByRole("group", { name: "Searches" });
  await expect.element(group).toHaveAttribute("aria-roledescription", "canvas");
  await expect
    .poll(() => stopsOf(screen.container).filter((stop) => stop.tab === 0))
    .toEqual([{ id: "a", tab: 0 }]);
  // xyflow's own node stops and arrow moves are off: only the top's stops rove.
  expect(screen.container.querySelector(".svelte-flow__node[tabindex]")).toBeNull();

  await userEvent.tab();
  await expect.element(screen.getByRole("button", { name: "a", exact: true })).toHaveFocus();
  await userEvent.keyboard("{ArrowDown}");
  await expect.element(screen.getByRole("link", { name: "a1" })).toHaveFocus();
  expect(onmove).toHaveBeenLastCalledWith("a1");
  expect(stopsOf(screen.container).filter((stop) => stop.tab === 0)).toEqual([
    { id: "a1", tab: 0 },
  ]);
  await userEvent.keyboard("{ArrowDown}{ArrowDown}");
  await expect.element(screen.getByRole("button", { name: "b", exact: true })).toHaveFocus();
  await userEvent.keyboard("{ArrowUp}");
  await expect.element(screen.getByRole("link", { name: "a2" })).toHaveFocus();

  await userEvent.keyboard("{Enter}");
  expect(onactivate).toHaveBeenCalledWith("a2");
  await userEvent.keyboard("{Escape}");
  expect(onleave).toHaveBeenCalledTimes(1);
  await screen.unmount();
});

test("a stop off screen comes into view before the keyboard lands on it", async () => {
  await page.viewport(1000, 700);
  const screen = await render(FocusStage, { keyboard: {} });
  await userEvent.tab();
  const viewport = screen.container.querySelector<HTMLElement>(".svelte-flow__viewport")!;
  const before = viewport.style.transform;
  await userEvent.keyboard("{ArrowRight}");
  await expect.element(screen.getByRole("button", { name: "c", exact: true })).toHaveFocus();
  await expect.poll(() => viewport.style.transform).not.toBe(before);
  const card = () =>
    screen.getByRole("button", { name: "c", exact: true }).element().getBoundingClientRect();
  const room = screen.container.querySelector(".canvas-host")!.getBoundingClientRect();
  await expect.poll(() => card().right).toBeLessThanOrEqual(room.right);
  expect(card().left).toBeGreaterThanOrEqual(room.left);
  await screen.unmount();
});

test("a canvas that takes no keyboard focus leaves its items to the flow", async () => {
  const screen = await render(FocusStage, { keyboard: { off: true } });
  await expect
    .poll(() => screen.container.querySelectorAll(".svelte-flow__node[tabindex='0']").length)
    .toBe(3);
  expect(screen.container.querySelector("[data-canvas-stop]")).toBeNull();
  expect(screen.container.querySelector(".canvas-host")!.getAttribute("role")).toBeNull();
  await screen.unmount();
});
