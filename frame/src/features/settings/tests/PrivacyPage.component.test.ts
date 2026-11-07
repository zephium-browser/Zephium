import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import PrivacyPage from "../components/sections/PrivacyPage.svelte";

vi.mock("$shared/platform", () => ({ IS_WINDOWS: false }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings();
});
vi.mock("../lib/settings-model", async (original) => ({
  ...(await original<typeof import("../lib/settings-model")>()),
  showPreviews: false,
}));
vi.mock("$domain/tabs", () => ({ tabs: { profile: () => ({ id: "profile" }) } }));
vi.mock("$domain/permissions", () => ({
  sitePermissions: { list: async () => null, set: vi.fn() },
}));
vi.mock("$domain/blocker", () => ({
  blocker: {
    status: () => ({
      protection: "disabled",
      preference: "authoritative",
      desired_enabled: false,
      can_enable: true,
      can_refresh_sources: true,
      source_phase: "fresh",
      retryable: false,
    }),
    refresh: vi.fn().mockResolvedValue(undefined),
  },
}));
vi.mock("$domain/credentials", () => ({
  browserCredentials: {
    current: () => null,
    activate: vi.fn().mockResolvedValue(undefined),
    deactivate: vi.fn(),
  },
  browserPasskeyStatus: () => "",
}));

afterEach(() => vi.clearAllMocks());

test("the release page offers browsing data that works and none of the previews", async () => {
  const screen = await render(PrivacyPage);

  await expect
    .element(page.getByRole("heading", { name: "Browsing data", exact: true }))
    .toBeVisible();
  await expect.element(page.getByText("Clear browsing data")).toBeVisible();
  await expect.element(page.getByText("Manage site data")).toBeVisible();
  await expect.element(page.getByRole("button", { name: "Clear…" })).toBeVisible();
  await expect.element(page.getByRole("button", { name: "Manage…" })).toBeVisible();

  const text = screen.container.textContent;
  expect(text).not.toContain("Microsoft Defender SmartScreen");
  expect(text).not.toContain("Windows diagnostic settings");
  expect(text).not.toContain("Interface preview");
  expect(text).not.toContain("Site exceptions");
  expect(text).not.toContain("Clear site data when Zephium closes");
});
