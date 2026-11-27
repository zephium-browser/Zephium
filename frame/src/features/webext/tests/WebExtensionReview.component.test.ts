import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { WebExtensionReview as Review } from "$shared/ipc/bindings";
import "$styles/global.css";
import WebExtensionAccessPrompt from "../components/WebExtensionAccessPrompt.svelte";
import WebExtensionReview from "../components/WebExtensionReview.svelte";

const copy = vi.hoisted(() => ({ localized: false }));
vi.mock("$shared/i18n/messages", async (importOriginal) => {
  const messages = await importOriginal<Record<string, unknown>>();
  return Object.fromEntries(
    Object.entries(messages).map(([key, message]) => [
      key,
      key.startsWith("webext_access_prompt_") && typeof message === "function"
        ? (...args: unknown[]) =>
            copy.localized
              ? `localized ${key}${key.endsWith("title") ? ` ${(args[0] as { name: string }).name}` : ""}`
              : Reflect.apply(message, undefined, args)
        : message,
    ]),
  );
});

const actions = vi.hoisted(() => ({
  confirm: vi.fn(async () => {}),
  cancel: vi.fn(),
  answerAccess: vi.fn(async () => {}),
  accessError: vi.fn<() => string | null>(() => null),
}));

vi.mock("$domain/webext", () => ({
  webext: {
    ...actions,
    isConfirming: () => false,
    error: () => null,
    isAnswering: () => false,
    named: () => null,
  },
}));
vi.mock("$session/sidebar-mode.svelte", () => ({
  COMPACT_WIDTH: 48,
  effectiveWidth: () => 320,
  expandBriefly: () => () => {},
  hasPanel: () => false,
}));

afterEach(() => {
  vi.clearAllMocks();
  actions.accessError.mockReturnValue(null);
  copy.localized = false;
});

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

test("failed access answers show an alert with retry and denial still available", async () => {
  actions.accessError.mockReturnValue("Could not save access.");
  render(WebExtensionAccessPrompt, {
    request: {
      profile_id: "profile",
      request: "1",
      extension_id: "extension",
      warnings: [],
      permissions: [],
      patterns: [],
    },
  });
  await expect.element(page.getByRole("alert")).toHaveTextContent("Could not save access.");
  await expect.element(page.getByRole("button", { name: "Deny", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Allow", exact: true }).click();
  expect(actions.answerAccess).toHaveBeenLastCalledWith(true);
  await page.getByRole("button", { name: "Deny", exact: true }).click();
  expect(actions.answerAccess).toHaveBeenLastCalledWith(false);
});

test("access prompt copy and accessible actions come from the catalog", async () => {
  copy.localized = true;
  render(WebExtensionAccessPrompt, {
    request: {
      profile_id: "profile",
      request: "1",
      extension_id: "extension",
      warnings: [],
      permissions: [],
      patterns: [],
    },
  });
  await expect
    .element(
      page.getByRole("heading", {
        name: "localized webext_access_prompt_title localized webext_access_prompt_extension",
      }),
    )
    .toBeVisible();
  await expect
    .element(page.getByText("localized webext_access_prompt_abilities", { exact: true }))
    .toBeVisible();
  await expect
    .element(page.getByText("localized webext_access_prompt_features", { exact: true }))
    .toBeVisible();
  for (const action of ["deny", "allow"]) {
    await expect
      .element(page.getByRole("button", { name: `localized webext_access_prompt_${action}` }))
      .toBeEnabled();
  }
});
