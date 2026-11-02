import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import StartupPages from "../components/StartupPages.svelte";
import HomePageRow from "../components/HomePageRow.svelte";

const state = vi.hoisted(() => ({
  values: {} as Record<string, string>,
  failed: false,
  open: [] as { url: string | null; content?: string }[],
}));

vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => state.values[key] ?? "",
    saving: () => false,
    saveFailed: () => state.failed,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));
vi.mock("$domain/tabs", () => ({ tabs: { tabs: () => state.open } }));

beforeEach(() => {
  state.values = { "tabs.startup-pages": "https://kept.example/", "tabs.home": "" };
  state.failed = false;
  state.open = [];
  vi.mocked(preferences.set).mockClear();
});

test("pages are added, kept to ten and removed one at a time", async () => {
  const screen = await render(StartupPages);
  await screen.getByRole("textbox").fill(" example.com/docs ");
  await screen.getByRole("button", { name: "Add", exact: true }).click();
  expect(preferences.set).toHaveBeenCalledWith(
    "tabs.startup-pages",
    "https://kept.example/\nexample.com/docs",
  );
  await screen.getByRole("button", { name: "Remove https://kept.example/" }).click();
  expect(preferences.set).toHaveBeenLastCalledWith("tabs.startup-pages", "");
});

test("a page native refuses is named invalid and stays in the field", async () => {
  state.failed = true;
  const screen = await render(StartupPages);
  await screen.getByRole("textbox").fill("two words");
  await screen.getByRole("button", { name: "Add", exact: true }).click();
  await expect.element(screen.getByText("Enter a valid website or URL.")).toBeVisible();
  await expect.element(screen.getByRole("textbox")).toHaveValue("two words");
});

test("a full list takes no more", async () => {
  state.values["tabs.startup-pages"] = Array.from(
    { length: 10 },
    (_, n) => `https://example.com/${n}`,
  ).join("\n");
  const screen = await render(StartupPages);
  await screen.getByRole("textbox").fill("example.org");
  await expect.element(screen.getByRole("button", { name: "Add", exact: true })).toBeDisabled();
});

test("current tabs replace the list with web pages only, once each, up to ten", async () => {
  state.open = [
    { url: "https://a.example/" },
    { url: "https://a.example/" },
    { url: null },
    { url: "https://settings.example/", content: "settings" },
    ...Array.from({ length: 12 }, (_, n) => ({ url: `https://b.example/${n}` })),
  ];
  const screen = await render(StartupPages);
  await screen.getByRole("button", { name: "Use current tabs" }).click();
  const [key, value] = vi.mocked(preferences.set).mock.calls.at(-1)!;
  expect(key).toBe("tabs.startup-pages");
  const lines = value.split("\n");
  expect(lines).toHaveLength(10);
  expect(lines[0]).toBe("https://a.example/");
  expect(lines).not.toContain("https://settings.example/");
});

test("the home page is saved as typed and cleared by saving it empty", async () => {
  state.values["tabs.home"] = "https://home.example/";
  const screen = await render(HomePageRow);
  const field = screen.getByRole("textbox");
  await expect.element(field).toHaveValue("https://home.example/");
  await field.fill("");
  await screen.getByRole("button", { name: "Save" }).click();
  expect(preferences.set).toHaveBeenCalledWith("tabs.home", "");
});
