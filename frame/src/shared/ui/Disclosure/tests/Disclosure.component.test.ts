import { expect, test } from "vitest";
import { userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import MenuHost from "./MenuHost.svelte";

test("arrow keys open a menu and skip disabled items", async () => {
  const screen = await render(MenuHost);
  const trigger = screen.getByRole("button", { name: "Actions", exact: true });
  trigger.element().focus();
  await userEvent.keyboard("{ArrowDown}");
  await expect.element(trigger).toHaveAttribute("aria-expanded", "true");
  await expect.element(screen.getByRole("menuitem", { name: "First", exact: true })).toHaveFocus();
  await userEvent.keyboard("{ArrowDown}");
  await expect.element(screen.getByRole("menuitem", { name: "Last", exact: true })).toHaveFocus();
  await userEvent.keyboard("{Escape}");
  await expect.element(trigger).toHaveFocus();
  await userEvent.keyboard("{ArrowUp}");
  await expect.element(screen.getByRole("menuitem", { name: "Last", exact: true })).toHaveFocus();
});
