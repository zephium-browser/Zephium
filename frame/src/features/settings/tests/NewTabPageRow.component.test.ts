import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import NewTabPageRow from "../components/NewTabPageRow.svelte";

const state = vi.hoisted(() => ({ saved: "", failed: false }));
vi.mock("$domain/preferences", () => ({
  preferences: {
    value: () => state.saved,
    saving: () => false,
    saveFailed: () => state.failed,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));
beforeEach(() => {
  state.saved = "";
  state.failed = false;
  vi.mocked(preferences.set).mockClear();
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
