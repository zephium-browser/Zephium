import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import PrivacyPage from "../components/sections/PrivacyPage.svelte";

const native = vi.hoisted(() => ({ history: vi.fn() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ historyCall: native.history as never });
});
vi.mock("$domain/tabs", () => ({ tabs: { profile: () => ({ id: "profile" }) } }));
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

test("clearing browsing data offers history only and clears the chosen range", async () => {
  native.history.mockResolvedValue({ kind: "removed", count: 3 });
  const screen = await render(PrivacyPage);
  await screen.getByRole("button", { name: "Clear…" }).click();

  const dialog = page.getByRole("dialog");
  await expect.element(dialog.getByRole("checkbox")).toHaveLength(1);
  await expect.element(dialog.getByRole("checkbox", { name: "Browsing history" })).toBeChecked();
  const text = dialog.element().textContent;
  expect(text).not.toContain("Cookies");
  expect(text).not.toContain("do not change browser behavior");

  await dialog.getByLabelText("Time range").click();
  await page.getByRole("option", { name: "Last 7 days" }).click();
  await dialog.getByRole("button", { name: "Clear history" }).click();

  expect(native.history).toHaveBeenCalledExactlyOnceWith("profile", {
    kind: "clear",
    range: "week",
  });
  await expect.element(page.getByRole("status")).toHaveTextContent("History cleared.");
});

test("a failed clear keeps the dialog open and says so", async () => {
  native.history.mockRejectedValue(new Error("offline"));
  const screen = await render(PrivacyPage);
  await screen.getByRole("button", { name: "Clear…" }).click();
  await page.getByRole("button", { name: "Clear history" }).click();
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't clear history");
  await expect.element(page.getByRole("dialog")).toBeVisible();
});

test("a native storage rejection never reports that history was cleared", async () => {
  native.history.mockResolvedValue({ kind: "error", error: "unavailable" });
  const screen = await render(PrivacyPage);
  await screen.getByRole("button", { name: "Clear…" }).click();
  await page.getByRole("button", { name: "Clear history" }).click();
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't clear history");
  await expect.element(page.getByRole("dialog")).toBeVisible();
  expect(screen.container.textContent).not.toContain("History cleared.");
});
