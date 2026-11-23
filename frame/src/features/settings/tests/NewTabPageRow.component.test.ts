import "$styles/global.css";
import { page } from "vitest/browser";
import type { PreferenceFixture } from "$shared/testing/preferences.svelte";
import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import NewTabPageRow from "../components/NewTabPageRow.svelte";
import NewTabPage from "../components/sections/NewTabPage.svelte";

const native = vi.hoisted(() => ({ state: null as unknown as PreferenceFixture }));
vi.mock("$domain/preferences", async () => {
  const { PreferenceFixture } = await import("$shared/testing/preferences.svelte");
  native.state = new PreferenceFixture();
  return { preferences: native.state };
});
beforeEach(() => {
  native.state.reset();
  vi.spyOn(preferences, "set").mockClear();
  vi.spyOn(preferences, "resetNewTab").mockClear();
});

test.each([" https://example.com/start ", ""])(
  "saves and clears the destination (%s)",
  async (value) => {
    native.state.values["tabs.new-page"] = "https://old.example/";
    const screen = await render(NewTabPageRow);
    await screen.getByRole("textbox").fill(value);
    await screen.getByRole("button", { name: "Save" }).click();
    expect(preferences.set).toHaveBeenCalledWith("tabs.new-page", value.trim());
  },
);

test("keeps a refused destination editable and reports the error", async () => {
  native.state.failed = true;
  const screen = await render(NewTabPageRow);
  await screen.getByRole("textbox").fill("javascript:alert(1)");
  await screen.getByRole("button", { name: "Save" }).click();
  await expect.element(screen.getByRole("textbox")).toHaveAttribute("aria-invalid", "true");
  await expect.element(screen.getByRole("textbox")).toHaveValue("javascript:alert(1)");
});

test("an older save cannot overwrite newer typing", async () => {
  let finish!: () => void;
  vi.mocked(preferences.set).mockImplementationOnce(async () => {
    native.state.pending = true;
    await new Promise<void>((resolve) => {
      finish = resolve;
    });
    native.state.values["tabs.new-page"] = "https://first.example/";
    native.state.pending = false;
  });
  const screen = await render(NewTabPageRow);
  await screen.getByRole("textbox").fill("https://first.example/");
  await screen.getByRole("button", { name: "Save" }).click();
  await screen.getByRole("textbox").fill("https://second.example/");
  await expect.element(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  finish();
  await expect.element(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  await expect.element(screen.getByRole("textbox")).toHaveValue("https://second.example/");
});

test.each([false, true])(
  "reset clears the destination draft and error (refused: %s)",
  async (refused) => {
    native.state.values["tabs.new-page"] = "https://saved.example/";
    const screen = await render(NewTabPage);
    await screen.getByRole("textbox", { name: "New tab URL" }).fill("javascript:alert(1)");
    if (refused) {
      native.state.failed = true;
      await screen.getByRole("button", { name: "Save", exact: true }).click();
      await expect
        .element(screen.getByRole("textbox", { name: "New tab URL" }))
        .toHaveAttribute("aria-invalid", "true");
    }
    vi.mocked(preferences.resetNewTab).mockImplementationOnce(async () => {
      native.state.values["tabs.new-page"] = "";
      native.state.failed = false;
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

test("a pending section reset blocks edits and submissions until it settles", async () => {
  let settleReset!: (ok: boolean) => void;
  vi.mocked(preferences.resetNewTab).mockReturnValueOnce(
    new Promise<boolean>((resolve) => {
      settleReset = resolve;
    }),
  );
  const screen = await render(NewTabPage);
  const field = screen.getByRole("textbox", { name: "New tab URL" });
  const save = screen.getByRole("button", { name: "Save", exact: true });
  await field.fill("https://draft.example/");
  await screen.getByRole("button", { name: "Reset to default", exact: true }).click();
  try {
    expect(field.element()).toBeDisabled();
    expect(save.element()).toBeDisabled();
    const form = (field.element() as HTMLInputElement).form!;
    form.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));
    expect(preferences.set).not.toHaveBeenCalled();
  } finally {
    settleReset(false);
  }
  await expect.element(field).toBeEnabled();
  await expect.element(save).toBeEnabled();
  await expect.element(field).toHaveValue("https://draft.example/");
  await expect.element(screen.getByRole("alert")).toBeVisible();
});

test.each([507, 360])("the full description wraps above the URL at %s px", async (width) => {
  await page.viewport(width, 300);
  const screen = await render(NewTabPageRow);
  screen.container.style.cssText = "padding:24px;background:var(--color-surface)";
  const input = screen.getByRole("textbox");
  await input.fill("https://example.com/start");
  const description = screen.container.querySelector(".copy p")!;
  const row = description.getBoundingClientRect();
  const field = input.element().getBoundingClientRect();
  const save = screen.getByRole("button", { name: "Save" }).element().getBoundingClientRect();
  expect(description.textContent).toBe(
    "Website for regular new tabs. Leave empty for Zephium’s page. Private tabs stay unchanged.",
  );
  expect(description.scrollWidth).toBeLessThanOrEqual(description.clientWidth);
  expect(description.scrollHeight).toBeLessThanOrEqual(description.clientHeight);
  expect(field.top).toBeGreaterThanOrEqual(row.bottom);
  expect(Math.abs(field.left - row.left)).toBeLessThan(1);
  expect(Math.abs(save.right - row.right)).toBeLessThan(1);
  expect(field.right).toBeLessThan(save.left);
});
