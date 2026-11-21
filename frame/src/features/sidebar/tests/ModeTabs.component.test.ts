import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { surface } from "$domain/surface";
import { preferences } from "$domain/preferences";
import { emitNativeEvent } from "$shared/testing/native-events";
import ModeTabs from "../components/ModeTabs.svelte";

const native = vi.hoisted(() => ({
  run: vi.fn(async () => ({ accepted: true, operation_id: null })),
  settings: {} as Record<string, string>,
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    runCommand: native.run,
    settingGet: async (key: string) => native.settings[key] ?? null,
  });
});

afterEach(() => {
  surface.dispose();
  preferences.dispose();
  native.settings = {};
});

test("the lit half follows native confirmation rather than command admission", async () => {
  await surface.init();
  const screen = await render(ModeTabs);
  const browse = screen.getByRole("radio", { name: "Browse", exact: true });
  const work = screen.getByRole("radio", { name: "Work", exact: true });

  await expect.element(browse).toHaveAttribute("aria-checked", "true");

  await work.click();
  expect(native.run).toHaveBeenCalledWith("browser.work");
  // Admission is not arrival: the thumb stays on Browse until native says so.
  await expect.element(work).toHaveAttribute("aria-checked", "false");
  emitNativeEvent("uiCommand", "browser.work");
  await expect.element(work).toHaveAttribute("aria-checked", "true");
  await expect.element(browse).toHaveAttribute("aria-checked", "false");

  await browse.click();
  expect(native.run).toHaveBeenCalledWith("browser.return");
  emitNativeEvent("uiCommand", "browser.return");
  await expect.element(browse).toHaveAttribute("aria-checked", "true");
});

test("re-selecting the side already showing asks native for nothing", async () => {
  await surface.init();
  const screen = await render(ModeTabs);
  native.run.mockClear();

  await screen.getByRole("radio", { name: "Browse", exact: true }).click();
  expect(native.run).not.toHaveBeenCalled();
});

test("the rail's switch marks where you are and goes to the other in one click", async () => {
  await surface.init();
  const screen = await render(ModeTabs, { compact: true });
  const browse = screen.getByRole("button", { name: "Browse", exact: true });
  const work = screen.getByRole("button", { name: "Work", exact: true });
  await expect.element(browse).toHaveAttribute("aria-pressed", "true");
  await work.click();
  expect(native.run).toHaveBeenCalledWith("browser.work");
  // The thumb moves only once native has moved there.
  await expect.element(work).toHaveAttribute("aria-pressed", "false");
  emitNativeEvent("uiCommand", "browser.work");
  await expect.element(work).toHaveAttribute("aria-pressed", "true");
});

test("with Work off the column has no switch at either width, and it comes back on", async () => {
  native.settings["work.enabled"] = "false";
  await preferences.init();
  await surface.init();
  const wide = await render(ModeTabs);
  const rail = await render(ModeTabs, { compact: true, standalone: true });
  await expect.element(wide.getByRole("radio", { name: "Work" })).not.toBeInTheDocument();
  await expect.element(rail.getByRole("button", { name: "Work" })).not.toBeInTheDocument();
  expect(wide.container.textContent?.trim()).toBe("");

  emitNativeEvent("uiCommand", "preference.work.enabled=true");
  await expect.element(wide.getByRole("radio", { name: "Work", exact: true })).toBeVisible();
  await expect.element(rail.getByRole("button", { name: "Work", exact: true })).toBeVisible();
});

test("the visibility preference hides every switch shape while keeping Work enabled", async () => {
  native.settings["ui.mode-switch"] = "false";
  await preferences.init();
  await surface.init();
  for (const props of [{}, { compact: true }, { compact: true, standalone: true }]) {
    const screen = await render(ModeTabs, props);
    expect(screen.container.textContent?.trim()).toBe("");
    await screen.unmount();
  }
  expect(preferences.enabled("work")).toBe(true);
  expect(preferences.commandOffered("browser.work")).toBe(true);
  const screen = await render(ModeTabs, { compact: true, standalone: true });
  emitNativeEvent("uiCommand", "preference.ui.mode-switch=true");
  await expect.element(screen.getByRole("button", { name: "Work", exact: true })).toBeVisible();
});
