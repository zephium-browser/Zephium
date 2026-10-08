import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import type { UpdateStatus } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { IS_MAC } from "$shared/platform";
import { preferences } from "$domain/preferences";
import { runtime } from "$domain/runtime";
import { updates } from "$domain/updates";
import { searchSettings } from "../lib/settings-model";
import AboutPage from "../components/sections/AboutPage.svelte";

const native = vi.hoisted(() => ({
  status: { state: "upToDate" } as UpdateStatus,
  check: vi.fn(async (): Promise<UpdateStatus> => ({ state: "ready", version: "1.0.2" })),
  relaunch: vi.fn(async () => true),
  openUrl: vi.fn(async () => ({ accepted: true, operation_id: null })),
  softwareUpdate: vi.fn(async () => true),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    aboutInfo: async () => ({ version: "1.0.1", os: "macOS 26.1.0", arch: "aarch64" }),
    updateStatus: async () => native.status,
    updateCheck: native.check,
    updateRelaunch: native.relaunch,
    browserOpenUrl: native.openUrl,
    openSoftwareUpdate: native.softwareUpdate,
  });
});

vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => ({ "updates.auto-check": "true" })[key],
    saving: () => false,
    saveFailed: () => false,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));

beforeEach(async () => {
  vi.clearAllMocks();
  await runtime.init();
});
afterEach(() => {
  updates.dispose();
  runtime.dispose();
});

test("updates are checked, downloaded and relaunched from About", async () => {
  native.status = { state: "upToDate" };
  const screen = await render(AboutPage);
  await expect.element(screen.getByText("Zephium is up to date")).toBeVisible();
  await expect.element(screen.getByText("1.0.1", { exact: true })).toBeVisible();

  await screen.getByRole("button", { name: "Check now" }).click();
  expect(native.check).toHaveBeenCalledOnce();
  await expect.element(screen.getByText("Version 1.0.2 is ready")).toBeVisible();

  native.status = { state: "installing" };
  await screen.getByRole("button", { name: "Relaunch to update" }).click();
  expect(native.relaunch).toHaveBeenCalledOnce();
  await expect.element(screen.getByText("Updating…")).toBeVisible();

  await screen.getByRole("switch", { name: "Check for updates automatically" }).click();
  expect(preferences.set).toHaveBeenCalledWith("updates.auto-check", "false");

  await screen.getByRole("button", { name: "Release Notes" }).click();
  expect(native.openUrl).toHaveBeenCalledWith(
    "https://github.com/zephium-browser/Zephium/releases/tag/v1.0.1",
    true,
  );
});

test("a failed check says so and can be tried again", async () => {
  native.status = { state: "idle" };
  native.check.mockResolvedValueOnce({ state: "failed" });
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Check now" }).click();
  await expect
    .element(
      screen.getByText("Couldn't complete the update. Try again, or download the latest version."),
    )
    .toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Check now" })).toBeEnabled();
});

test("a development build only says that updates are off", async () => {
  native.status = { state: "unavailable" };
  const screen = await render(AboutPage);
  await expect.element(screen.getByText("Updates are off in development builds")).toBeVisible();
  expect(screen.getByRole("button", { name: "Check now" }).query()).toBeNull();
  expect(
    screen.getByRole("switch", { name: "Check for updates automatically" }).query(),
  ).toBeNull();
  expect(screen.getByRole("button", { name: "Release Notes" }).query()).toBeNull();
});

test("a system behind on security updates is explained where the rail sends people", async () => {
  native.status = { state: "upToDate" };
  emitNativeEvent("runtimeStatusChanged", {
    restart_required: false,
    session_set_aside: false,
    user_content_degraded_scope_count: 0,
    security_advisories: [
      { kind: "review_overdue", update_target: "zephium" },
      { kind: "update_recommended", update_target: "operating_system" },
    ],
  });
  const screen = await render(AboutPage);
  await expect.element(screen.getByText("Install them to keep browsing safely.")).toBeVisible();
  expect(screen.container.textContent).not.toContain("review");
  if (IS_MAC) {
    await screen.getByRole("button", { name: "Open Software Update" }).click();
    expect(native.softwareUpdate).toHaveBeenCalledOnce();
  }
});

test("settings search finds the update controls", () => {
  expect(searchSettings("updates automatically").map((result) => result.target)).toContain(
    "updates.auto-check",
  );
  expect(searchSettings("new version").map((result) => result.target)).toContain("about.updates");
});

test("manual installation opens the exact release instead of retrying an impossible swap", async () => {
  native.status = { state: "manualInstall", version: "1.0.2" };
  const screen = await render(AboutPage);
  await screen.getByRole("button", { name: "Download and install manually" }).click();
  expect(native.openUrl).toHaveBeenCalledWith(
    "https://github.com/zephium-browser/Zephium/releases/tag/v1.0.2",
    true,
  );
  expect(screen.getByRole("button", { name: "Relaunch to update" }).query()).toBeNull();
});

test("a postponed update remains ready with its failure explained", async () => {
  native.status = {
    state: "ready",
    version: "1.0.2",
    retry_reason:
      "The previous update did not finish. Your downloaded update is still ready to install.",
  };
  const screen = await render(AboutPage);
  await expect.element(screen.getByText(native.status.retry_reason!)).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Relaunch to update" })).toBeEnabled();
});
