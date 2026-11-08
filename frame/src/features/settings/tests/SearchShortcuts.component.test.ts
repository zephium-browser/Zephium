import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import { preferences } from "$domain/preferences";
import SearchShortcuts from "../components/SearchShortcuts.svelte";

const native = vi.hoisted(() => ({ stored: "" }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key: string) => (key === "search.shortcuts" ? native.stored : null),
    settingSet: async (_key: string, value: string) => {
      native.stored = value;
      return { accepted: true, operation_id: null };
    },
    searchShortcutDefaults: async () => [
      {
        id: "youtube",
        name: "YouTube",
        keywords: ["yt"],
        domain: "youtube.com",
        enabled: true,
        search_url: "https://youtube.com/?q={query}",
        home_url: "https://youtube.com/",
        suggestions: null,
      },
    ],
  });
});
vi.mock("$domain/operations", () => ({
  settle: async (admission: Promise<unknown>) => {
    await admission;
    return { outcome: "deferred" };
  },
}));
afterEach(() => {
  preferences.dispose();
  native.stored = "";
});

test("toggles, edits, adds, removes and resets persisted shortcuts", async () => {
  await preferences.init();
  const screen = await render(SearchShortcuts);
  await userEvent.click(screen.getByRole("switch", { name: "Enable YouTube" }));
  await expect.poll(() => preferences.searchShortcuts()[0]?.enabled).toBe(false);
  const keyword = screen.getByRole("textbox", { name: "Keyword" }).nth(0);
  await userEvent.fill(keyword, "video");
  await userEvent.click(screen.getByRole("button", { name: "Save", exact: true }));
  await expect.poll(() => preferences.searchShortcuts()[0]?.keywords).toEqual(["video"]);
  await userEvent.fill(screen.getByRole("textbox", { name: "Name", exact: true }), "Example");
  await userEvent.fill(screen.getByRole("textbox", { name: "Keyword" }).nth(1), "ex");
  await userEvent.fill(
    screen.getByRole("textbox", { name: "Search URL with {query}" }),
    "https://example.com/?q={query}",
  );
  await userEvent.click(screen.getByRole("button", { name: "Add shortcut" }));
  await expect.poll(() => preferences.searchShortcuts().length).toBe(2);
  await userEvent.click(screen.getByRole("button", { name: "Remove shortcut" }));
  await expect.poll(() => preferences.searchShortcuts().length).toBe(1);
  await userEvent.click(screen.getByRole("button", { name: "Reset shortcuts to defaults" }));
  await expect.poll(() => native.stored).toBe("");
  expect(preferences.searchShortcuts()[0]).toMatchObject({ enabled: true, keywords: ["yt"] });
});

test("duplicate keywords are refused without saving", async () => {
  await preferences.init();
  const screen = await render(SearchShortcuts);
  await userEvent.fill(screen.getByRole("textbox", { name: "Name", exact: true }), "Example");
  await userEvent.fill(screen.getByRole("textbox", { name: "Keyword" }).nth(1), "YT");
  await userEvent.fill(
    screen.getByRole("textbox", { name: "Search URL with {query}" }),
    "https://example.com/?q={query}",
  );
  await userEvent.click(screen.getByRole("button", { name: "Add shortcut" }));
  await expect.element(screen.getByRole("alert")).toBeVisible();
  expect(native.stored).toBe("");
});
