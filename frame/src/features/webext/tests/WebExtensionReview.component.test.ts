import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { WebExtensionReview as Review } from "$shared/ipc/bindings";
import "$styles/global.css";
import WebExtensionReview from "../components/WebExtensionReview.svelte";

const actions = vi.hoisted(() => ({ confirm: vi.fn(async () => {}), cancel: vi.fn() }));

vi.mock("$domain/webext", () => ({
  webext: { ...actions, isConfirming: () => false, error: () => null },
}));
vi.mock("$session/sidebar-mode.svelte", () => ({
  COMPACT_WIDTH: 48,
  effectiveWidth: () => 320,
  expandBriefly: () => () => {},
  hasPanel: () => false,
}));

afterEach(() => vi.clearAllMocks());

const review: Review = {
  review_id: "displayed-review",
  id: "extension-id",
  name: "Fixture",
  version: "1.0",
  description: "",
  warnings: [],
  limitations: [],
  icon: null,
  update: false,
  from_file: false,
};

test("approval sends the nonce of the displayed review", async () => {
  render(WebExtensionReview, { review });
  await userEvent.click(page.getByRole("button", { name: "Add", exact: true }));
  expect(actions.confirm).toHaveBeenCalledExactlyOnceWith("displayed-review");
});

test("Escape cancels only the displayed review", async () => {
  render(WebExtensionReview, { review });
  await userEvent.keyboard("{Escape}");
  expect(actions.cancel).toHaveBeenCalledExactlyOnceWith("displayed-review");
});

test("limitations wait behind one line, apart from the access it asks for", async () => {
  render(WebExtensionReview, {
    review: {
      ...review,
      warnings: ["Manage your downloads"],
      limitations: ["Can't show its side panel"],
    },
  });
  const limits = page.getByRole("button", { name: "Some features may not work in Zephium" });
  await expect.element(limits).toHaveAttribute("aria-expanded", "false");
  await expect.element(page.getByText("Can't show its side panel")).not.toBeInTheDocument();
  await limits.click();
  await expect.element(page.getByText("Can't show its side panel")).toBeVisible();
});

test("no limitations adds no section", async () => {
  render(WebExtensionReview, { review });
  await expect
    .element(page.getByText("Some features may not work in Zephium"))
    .not.toBeInTheDocument();
});
