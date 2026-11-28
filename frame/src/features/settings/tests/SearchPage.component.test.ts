import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { preferences } from "$domain/preferences";
import SearchPage from "../components/sections/SearchPage.svelte";

const native = vi.hoisted(() => ({
  values: new Map<string, string>(),
  set: vi.fn(async (key: string, value: string) => {
    native.values.set(key, value);
    return { accepted: true, operation_id: null };
  }),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => native.values.get(key) ?? null,
    settingSet: native.set,
    searchShortcutDefaults: async () => [],
  });
});

vi.mock("$domain/operations", () => ({
  settle: async (admission: Promise<{ accepted: boolean }>) => ({
    outcome: (await admission).accepted ? "deferred" : "rejected",
  }),
}));

beforeEach(() => {
  native.values.clear();
  native.values.set("search.engine", "google");
  native.values.set("search.kagi-token", "");
  native.values.set("search.custom-url", "");
  native.values.set("search.suggestions", "true");
  native.values.set("search.shortcuts", "");
});

afterEach(() => {
  preferences.dispose();
  native.set.mockReset();
});

test("Kagi is offered beside the other search engines", async () => {
  await preferences.init();
  const screen = await render(SearchPage);
  await screen.getByRole("button", { name: "Default search engine" }).click();
  await expect
    .element(screen.getByRole("option", { name: "Kagi", exact: true }))
    .toBeVisible();
  await screen.getByRole("option", { name: "Kagi", exact: true }).click();
  await expect.poll(() => native.set).toHaveBeenCalledWith("search.engine", "kagi");
  await expect
    .element(screen.getByRole("heading", { name: "Kagi session token" }))
    .toBeVisible();
});

test("the Kagi session token is a private field saved under its own preference", async () => {
  native.values.set("search.engine", "kagi");
  await preferences.init();
  const screen = await render(SearchPage);
  const field = screen.getByLabelText("Kagi session token", { exact: true });
  await expect.element(field).toHaveAttribute("type", "password");
  await field.fill("kagi-session-123");
  const form = field.element().closest("form")!;
  const save = form.querySelector<HTMLButtonElement>("button")!;
  expect(save.disabled).toBe(false);
  save.click();
  await expect
    .poll(() => native.set)
    .toHaveBeenCalledWith("search.kagi-token", "kagi-session-123");
  await expect.poll(() => preferences.value("search.kagi-token")).toBe("kagi-session-123");
});

test("an invalid Kagi token cannot be saved", async () => {
  native.values.set("search.engine", "kagi");
  await preferences.init();
  const screen = await render(SearchPage);
  const field = screen.getByLabelText("Kagi session token", { exact: true });
  await field.fill("token with spaces");
  const save = field.element().closest("form")!.querySelector<HTMLButtonElement>("button")!;
  expect(save.disabled).toBe(true);
  await expect.element(field).toHaveAttribute("aria-invalid", "true");
  expect(native.set).not.toHaveBeenCalled();
});

test("the token field stays out of the way for other engines", async () => {
  await preferences.init();
  const screen = await render(SearchPage);
  await expect
    .element(screen.getByLabelText("Kagi session token", { exact: true }))
    .not.toBeInTheDocument();
});
