import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import SidebarLook from "./SidebarLook.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    runCommand: vi.fn(async () => ({ accepted: true, operation_id: null })),
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: vi.fn(async () => undefined),
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
  });
});

// A rendered reference for the column's resting shape, in both themes and
// both widths. Static checks cannot tell whether the base of the sidebar
// reads as one band.
test("the sidebar column at rest", async () => {
  const screen = await render(SidebarLook);
  await page.viewport(420, 700);
  document.documentElement.dataset.theme = "dark";
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/sidebar-dark.png" });
  document.documentElement.dataset.theme = "light";
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/sidebar-light.png" });
  // Kept sites stay one row, however many there are.
  const row = screen.container.querySelector<HTMLElement>(".dock-row")!;
  expect(getComputedStyle(row).gridAutoFlow).toBe("column");

  // And the shelf opened, which is the only state the column grows into.
  document.documentElement.dataset.theme = "dark";
  await screen.getByRole("button", { name: "Tools", exact: true }).click();
  const stack = screen.container.querySelector<HTMLElement>(".shelf-stack")!;
  await expect.poll(() => stack.inert).toBe(false);
  // The stack unfolds; capture it settled, not mid-rise.
  const farthest = stack.querySelector<HTMLElement>(".shelf-item")!;
  await expect.poll(() => getComputedStyle(farthest).opacity).toBe("1");
  await expect.poll(() => getComputedStyle(stack).opacity).toBe("1");
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/sidebar-shelf.png" });
});

test("the rail at rest", async () => {
  const screen = await render(SidebarLook, { props: { compact: true } });
  await page.viewport(420, 700);
  document.documentElement.dataset.theme = "dark";
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/rail-dark.png" });

  // The rail's stack is glyphs only: at 56px a name has nowhere to go, so
  // each is named for assistive technology and by its tooltip instead.
  await screen.getByRole("button", { name: "Tools", exact: true }).click();
  const stack = screen.container.querySelector<HTMLElement>(".shelf-stack")!;
  await expect.poll(() => stack.inert).toBe(false);
  const farthest = stack.querySelector<HTMLElement>(".shelf-item")!;
  await expect.poll(() => getComputedStyle(farthest).opacity).toBe("1");
  await expect.poll(() => getComputedStyle(stack).opacity).toBe("1");
  // You and a private window head the stack as glyphs; then the tools in the
  // native menu's order, and Settings last, nearest the case.
  await expect
    .element(page.getByRole("menuitem", { name: "New Incognito Window" }))
    .toBeInTheDocument();
  const items = [...stack.querySelectorAll<HTMLElement>(".shelf-item")];
  expect(items.map((item) => item.getAttribute("aria-label"))).toEqual([
    "Ask Zephium",
    "Notes",
    "Tasks",
    "Time",
    "History",
    "Downloads",
    "Bookmarks",
    "Settings",
  ]);
  expect(items.every((item) => item.textContent?.trim() === "")).toBe(true);
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/rail-shelf.png" });
});

test("in Work the rail ends in the account, where Browse keeps its tools", async () => {
  const screen = await render(SidebarLook, { props: { compact: true, work: true } });
  await page.viewport(420, 700);
  document.documentElement.dataset.theme = "dark";
  await expect.element(screen.getByRole("button", { name: "Account" })).toBeVisible();
  expect(screen.container.querySelector(".shelf")).toBeNull();
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/rail-work-dark.png" });
  await screen.getByRole("button", { name: "Account" }).click();
  await expect.element(page.getByRole("menuitem", { name: "Settings", exact: true })).toBeVisible();
  await page.screenshot({ path: "../../../../../target/rail-work-menu.png" });
});
