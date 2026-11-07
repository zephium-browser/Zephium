import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import ToolShelf from "../components/ToolShelf.svelte";

const native = vi.hoisted(() => ({
  run: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    runCommand: native.run,
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    settingGet: async () => null,
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: vi.fn(async () => undefined),
  });
});

test("with no menu bar, the stack still ends at Settings; Help lives in Settings → About", async () => {
  await page.viewport(1200, 800);
  const screen = await render(ToolShelf);
  screen.container.style.paddingBlockStart = "480px";
  await screen.getByRole("button", { name: /^Tools/ }).click();
  const steps = [...screen.container.querySelectorAll<HTMLElement>(".shelf-item")].map((item) => [
    item.textContent?.trim(),
    Number(item.style.getPropertyValue("--step")),
  ]);
  expect(steps.at(-1)).toEqual(["Settings", 0]);
  expect(screen.getByRole("menuitem", { name: "Help" }).query()).toBeNull();
});
