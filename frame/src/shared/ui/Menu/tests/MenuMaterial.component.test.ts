import { expect, test } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import Host from "./MenuMaterialHost.svelte";

for (const opaque of [true, false]) {
  test(
    opaque
      ? "sidebar policy follows menus through portals"
      : "unmarked Settings and macOS menus retain blur",
    async () => {
      const root = document.documentElement;
      const previous = root.getAttribute("style");
      root.style.setProperty("--color-float", "rgb(61, 61, 68)");
      root.style.setProperty("--color-menu", "rgba(61, 61, 68, 0.66)");
      const screen = await render(Host, { opaque });
      try {
        for (const label of ["Actions", "Choice", "Utilities"]) {
          await page.getByRole("button", { name: label, exact: true }).click();
          const surface = () =>
            document.querySelector<HTMLElement>(
              '.ui-menu[data-state="open"], .ui-menu[data-open="true"]',
            );
          await expect
            .poll(() => surface()?.getAttribute("data-menu-material"))
            .toBe(opaque ? "opaque" : null);
          await expect
            .poll(() => surface() && getComputedStyle(surface()!).backdropFilter)
            .toBe(opaque ? "none" : "blur(24px)");
          await expect
            .poll(() => surface() && getComputedStyle(surface()!).backgroundColor)
            .toBe(opaque ? "rgb(61, 61, 68)" : "rgba(61, 61, 68, 0.66)");
          await userEvent.keyboard("{Escape}");
        }
      } finally {
        await screen.unmount();
        if (previous === null) root.removeAttribute("style");
        else root.setAttribute("style", previous);
      }
    },
  );
}

test("checkmarked entries expose their checked state and ordinary actions stay menuitems", async () => {
  render(Host, {
    entries: [
      { kind: "item", id: "25", label: "25 minutes", checked: true },
      { kind: "item", id: "50", label: "50 minutes", checked: false },
      { kind: "item", id: "breaks", label: "Breaks", checked: false },
      { kind: "item", id: "action", label: "Action" },
    ],
  });
  await page.getByRole("button", { name: "Actions", exact: true }).click();
  await expect
    .element(page.getByRole("menuitemcheckbox", { name: "25 minutes" }))
    .toHaveAttribute("aria-checked", "true");
  for (const name of ["50 minutes", "Breaks"]) {
    await expect
      .element(page.getByRole("menuitemcheckbox", { name }))
      .toHaveAttribute("aria-checked", "false");
  }
  await expect
    .element(page.getByRole("menuitem", { name: "Action", exact: true }))
    .not.toHaveAttribute("aria-checked");
  await userEvent.keyboard("{Escape}");
});
