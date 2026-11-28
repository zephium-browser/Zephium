import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { flushSync } from "svelte";
import * as settings from "../lib/settings-state.svelte";
import "$styles/global.css";
import SettingsNavigation from "../components/SettingsNavigation.svelte";

const native = vi.hoisted(() => ({
  available: true,
  runCommand: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { accountFixture } = await import("$shared/testing/account");
  return mockBindings({
    accountGet: async () => ({
      account: accountFixture({ available: native.available }),
      fault: null,
      retry_after: null,
    }),
    runCommand: native.runCommand,
  });
});

test("the current page's plate travels to the page chosen next", async () => {
  await page.viewport(400, 900);
  const screen = await render(SettingsNavigation);
  const items = [...screen.container.querySelectorAll<HTMLElement>(".settings-nav-item")];
  const current = items.find((item) => item.getAttribute("aria-current") === "page")!;
  const next = items.at(-1)!;
  expect(next).not.toBe(current);

  next.click();
  flushSync();
  const glides = screen.container.querySelectorAll<HTMLElement>(".selection-glide");
  expect(glides).toHaveLength(1);
  expect(next.getAttribute("aria-current")).toBe("page");
  expect(next.hasAttribute("data-gliding")).toBe(true);
  await expect.poll(() => glides[0]!.isConnected, { timeout: 2000 }).toBe(false);
  expect(next.hasAttribute("data-gliding")).toBe(false);
});

test("the identity card opens Account and holds the plate while it is current", async () => {
  await page.viewport(400, 900);
  settings.select("general");
  const screen = await render(SettingsNavigation);
  const card = screen.getByRole("button", { name: /^Sign in/u });
  await expect.element(card).toBeVisible();
  const groups = [...screen.container.querySelectorAll(".settings-nav-group h2")].map(
    (heading) => heading.textContent,
  );
  expect(groups).toEqual(["Browser", "Work", "App"]);
  expect(screen.container.textContent).not.toContain("Plugins");

  await card.click();
  expect(settings.section()).toBe("account");
  expect(card.element().getAttribute("aria-current")).toBe("page");
  expect(screen.container.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  await screen.unmount();
  settings.select("general");
});

test("the identity card is gone where accounts are unavailable", async () => {
  native.available = false;
  const screen = await render(SettingsNavigation);
  await expect.poll(() => screen.container.querySelector(".identity-slot")).toBeNull();
  native.available = true;
  await screen.unmount();
});

test("Extensions opens the extension manager instead of a settings section", async () => {
  await page.viewport(400, 900);
  const screen = await render(SettingsNavigation);
  const button = screen.getByRole("button", { name: "Extensions" });
  await expect.element(button).toBeVisible();
  await button.click();
  expect(native.runCommand).toHaveBeenCalledExactlyOnceWith("browser.extensions");
  expect(screen.container.querySelector('[aria-current="page"]')).not.toBe(button.element());
  await screen.unmount();
});
