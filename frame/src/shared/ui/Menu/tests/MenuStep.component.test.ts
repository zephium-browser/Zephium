import { expect, test } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import Host from "./MenuMaterialHost.svelte";
import type { MenuEntry } from "../Menu.svelte";

const entries: MenuEntry[] = Array.from({ length: 60 }, (_, index) => ({
  kind: "item",
  id: `row-${index}`,
  label: `Row ${index}`,
}));

const ends = () => {
  const menu = [...document.querySelectorAll<HTMLElement>('.ui-menu[role="menu"]')].at(-1)!;
  const step = (dir: string) =>
    menu.querySelector<HTMLElement>(`.ui-menu-step[data-dir="${dir}"]`)!;
  return { menu, up: step("up"), down: step("down") };
};

test("a long menu shows the end it can scroll to, flush with its edge", async () => {
  await render(Host, { entries });
  await page.getByRole("button", { name: "Actions", exact: true }).click();
  await expect.poll(() => ends().down.hasAttribute("data-shown")).toBe(true);
  const { menu, up, down } = ends();
  expect(up.hasAttribute("data-shown")).toBe(false);
  const box = menu.getBoundingClientRect();
  expect(Math.abs(down.getBoundingClientRect().bottom - box.bottom)).toBeLessThanOrEqual(1);

  menu.scrollTop = 200;
  await expect.poll(() => ends().up.hasAttribute("data-shown")).toBe(true);
  expect(Math.abs(ends().up.getBoundingClientRect().top - box.top)).toBeLessThanOrEqual(1);
});

test("resting on the lower end glides the list to its last row", async () => {
  await render(Host, { entries });
  await page.getByRole("button", { name: "Actions", exact: true }).click();
  await expect.poll(() => ends().down.hasAttribute("data-shown")).toBe(true);
  ends().down.dispatchEvent(new PointerEvent("pointerenter"));
  await expect
    .poll(
      () => {
        const { menu } = ends();
        return menu.scrollHeight - menu.clientHeight - menu.scrollTop;
      },
      { timeout: 5000 },
    )
    .toBeLessThanOrEqual(1);
  await expect.poll(() => ends().down.hasAttribute("data-shown")).toBe(false);
});

test("a short menu shows neither end", async () => {
  await render(Host);
  await page.getByRole("button", { name: "Actions", exact: true }).click();
  await expect.element(page.getByRole("menuitem", { name: "One" })).toBeVisible();
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  expect(ends().up.hasAttribute("data-shown")).toBe(false);
  expect(ends().down.hasAttribute("data-shown")).toBe(false);
});
