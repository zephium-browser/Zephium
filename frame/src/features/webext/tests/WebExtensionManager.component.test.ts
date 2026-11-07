import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import "$styles/global.css";
import type { WebExtensionView } from "$shared/ipc/bindings";
import WebExtensionManager from "../components/WebExtensionManager.svelte";

const native = vi.hoisted(() => ({ list: vi.fn(), retry: vi.fn(), close: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ webExtensionList: native.list, webExtensionRetry: native.retry });
});
vi.mock("$domain/surface", () => ({ surface: { close: native.close } }));
vi.mock("../lib/catalog", () => ({ catalog: [], storeListing: () => "" }));

const extension: WebExtensionView = {
  profile_id: "profile-a",
  install_id: "install-a",
  id: "same-extension",
  name: "Fixture",
  version: "1.0",
  description: "",
  enabled: true,
  icon: null,
  state: "starting",
  error: null,
  warnings: [],
  limitations: [],
  access: "all",
  sites: [],
  site_scoped: false,
  has_options: false,
  held_update: null,
  sideloaded: false,
  pinned: false,
};

test("startup preparation failure becomes visible and retry retains the displayed installation", async () => {
  const message =
    "The extension package could not be prepared. Retry, or reinstall it from its original source.";
  native.list.mockResolvedValueOnce({ status: "ok", data: [extension] });
  native.list.mockResolvedValue({
    status: "ok",
    data: [{ ...extension, state: "failed", error: message }],
  });
  native.retry.mockImplementation(async () => {
    native.list.mockResolvedValue({ status: "ok", data: [{ ...extension, state: "running" }] });
    return { status: "ok", data: null };
  });
  render(WebExtensionManager);
  await expect.element(page.getByText("Starting…")).toBeVisible();
  await expect.element(page.getByText(`Couldn't start: ${message}`)).toBeVisible();
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  expect(native.retry).toHaveBeenCalledExactlyOnceWith("profile-a", "install-a");
  await expect.element(page.getByText("Version 1.0")).toBeVisible();
});
