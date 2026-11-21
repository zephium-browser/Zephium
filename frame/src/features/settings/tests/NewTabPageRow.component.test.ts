import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import NewTabPageRow from "../components/NewTabPageRow.svelte";
import NewTabPage from "../components/sections/NewTabPage.svelte";

const state = vi.hoisted(() => ({ saved: "", failed: false }));
vi.mock("$domain/preferences", () => ({
  preferences: {
    value: () => state.saved,
    saving: () => false,
    saveFailed: () => state.failed,
    enabled: () => false,
    set: vi.fn().mockResolvedValue(undefined),
    resetNewTab: vi.fn(),
  },
}));
beforeEach(() => {
  state.saved = "";
  state.failed = false;
  vi.mocked(preferences.set).mockClear();
  vi.mocked(preferences.resetNewTab).mockReset();
});

test.each([" https://example.com/start ", ""])(
  "saves and clears the destination (%s)",
  async (value) => {
    state.saved = "https://old.example/";
    const screen = await render(NewTabPageRow);
    await screen.getByRole("textbox").fill(value);
    await screen.getByRole("button", { name: "Save" }).click();
    expect(preferences.set).toHaveBeenCalledWith("tabs.new-page", value.trim());
  },
);

test("keeps a refused destination editable and reports the error", async () => {
  state.failed = true;
  const screen = await render(NewTabPageRow);
  await screen.getByRole("textbox").fill("javascript:alert(1)");
  await screen.getByRole("button", { name: "Save" }).click();
  await expect.element(screen.getByRole("textbox")).toHaveAttribute("aria-invalid", "true");
  await expect.element(screen.getByRole("textbox")).toHaveValue("javascript:alert(1)");
});

test("an older save cannot overwrite newer typing", async () => {
  let finish!: () => void;
  vi.mocked(preferences.set).mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  const screen = await render(NewTabPageRow);
  await screen.getByRole("textbox").fill("https://first.example/");
  await screen.getByRole("button", { name: "Save" }).click();
  await screen.getByRole("textbox").fill("https://second.example/");
  state.saved = "https://first.example/";
  finish();
  await expect.element(screen.getByRole("textbox")).toHaveValue("https://second.example/");
});

test.each([false, true])(
  "reset clears the destination draft and error (refused: %s)",
  async (refused) => {
    state.saved = "https://saved.example/";
    const screen = await render(NewTabPage);
    await screen.getByRole("textbox", { name: "New tab URL" }).fill("javascript:alert(1)");
    if (refused) {
      state.failed = true;
      await screen.getByRole("button", { name: "Save", exact: true }).click();
      await expect
        .element(screen.getByRole("textbox", { name: "New tab URL" }))
        .toHaveAttribute("aria-invalid", "true");
    }
    vi.mocked(preferences.resetNewTab).mockImplementationOnce(async () => {
      state.saved = "";
      state.failed = false;
      return true;
    });
    await screen.getByRole("button", { name: "Reset to default", exact: true }).click();
    expect(preferences.resetNewTab).toHaveBeenCalledOnce();
    await expect.element(screen.getByRole("textbox", { name: "New tab URL" })).toHaveValue("");
    await expect
      .element(screen.getByRole("textbox", { name: "New tab URL" }))
      .not.toHaveAttribute("aria-invalid", "true");
  },
);

test("a failed section reset keeps the destination draft", async () => {
  const screen = await render(NewTabPage);
  await screen.getByRole("textbox", { name: "New tab URL" }).fill("https://draft.example/");
  vi.mocked(preferences.resetNewTab).mockResolvedValueOnce(false);
  await screen.getByRole("button", { name: "Reset to default", exact: true }).click();
  await expect
    .element(screen.getByRole("textbox", { name: "New tab URL" }))
    .toHaveValue("https://draft.example/");
  await expect.element(screen.getByRole("alert")).toBeVisible();
});
