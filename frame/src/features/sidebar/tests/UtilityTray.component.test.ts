import { expect, test } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import UtilityTrayHost from "./UtilityTrayHost.svelte";

const tray = (props: { warning?: string | null } = {}) => render(UtilityTrayHost, props);

test("one glyph, named for the panel it opens", async () => {
  const screen = await tray();
  const glyph = screen.getByRole("button", { name: "Site information" });
  await expect.element(glyph).toHaveAttribute("aria-expanded", "false");
  await expect.element(glyph).not.toHaveAttribute("title");
});

test("a press opens the panel and takes the keyboard into it", async () => {
  const screen = await tray();
  const glyph = screen.getByRole("button", { name: "Site information" });
  await glyph.click();
  await expect.element(glyph).toHaveAttribute("aria-expanded", "true");
  const panel = screen.getByRole("group", { name: "Site information" });
  await expect.element(panel.getByRole("button", { name: "Inside" })).toBeVisible();
  await expect.poll(() => panel.element().contains(document.activeElement)).toBe(true);
});

test("the keyboard opens it, Escape closes it, and focus returns to the glyph", async () => {
  const screen = await tray();
  const glyph = screen.getByRole("button", { name: "Site information" });
  glyph.element().focus();
  await userEvent.keyboard("{Enter}");
  await expect.element(glyph).toHaveAttribute("aria-expanded", "true");
  const panel = screen.getByRole("group", { name: "Site information" });
  await expect.poll(() => panel.element().contains(document.activeElement)).toBe(true);

  await userEvent.keyboard("{Escape}");
  await expect.element(glyph).toHaveAttribute("aria-expanded", "false");
  await expect.element(glyph).toHaveFocus();
});

test("Escape still closes it once a control inside has focus", async () => {
  const screen = await tray();
  const glyph = screen.getByRole("button", { name: "Site information" });
  await glyph.click();
  const inside = screen.getByRole("button", { name: "Inside" });
  inside.element().focus();
  await userEvent.keyboard("{Escape}");
  await expect.element(glyph).toHaveAttribute("aria-expanded", "false");
});

test("a press outside closes it", async () => {
  const screen = await tray();
  const glyph = screen.getByRole("button", { name: "Site information" });
  await glyph.click();
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
  await expect.element(glyph).toHaveAttribute("aria-expanded", "false");
});

test("the sections are told whether the panel is showing", async () => {
  const screen = await tray();
  const told = () => screen.container.querySelector("output")?.textContent;
  expect(told()).toBe("hidden");
  await screen.getByRole("button", { name: "Site information" }).click();
  await expect.poll(told).toBe("showing");
});

test("a page that is not secure keeps the glyph in view, as a warning with its reason", async () => {
  const screen = await tray({ warning: "Connection is not encrypted" });
  const glyph = screen.getByRole("button", { name: "Site information" });
  await expect.element(glyph).toHaveAttribute("title", "Connection is not encrypted");
  await expect.element(glyph).toHaveClass("utilities-warning");
  expect(getComputedStyle(glyph.element()).opacity).toBe("1");
});

test("a page with nothing wrong waits for the pointer to bring the glyph in", async () => {
  const screen = await tray();
  const glyph = screen.getByRole("button", { name: "Site information" });
  await expect.element(glyph).not.toHaveClass("utilities-warning");
  expect(getComputedStyle(glyph.element()).opacity).toBe("0");
});
