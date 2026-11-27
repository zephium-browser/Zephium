import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import ProxyRows from "../components/ProxyRows.svelte";
import SearchPage from "../components/sections/SearchPage.svelte";

const native = vi.hoisted(() => ({
  values: new Map<string, string>(),
  set: vi.fn<(key: string, value: string) => Promise<{ accepted: boolean; operation_id: null }>>(),
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

afterEach(() => {
  preferences.dispose();
  native.values.clear();
  native.set.mockReset();
});

const proxy = {
  component: ProxyRows,
  label: "Proxy address",
  key: "network.proxy-address",
  first: "first.example:3128",
  second: "second.example:3128",
};
const search = {
  component: SearchPage,
  label: "Custom search engine",
  key: "search.custom-url",
  first: "https://first.example/?q={searchTerms}",
  second: "https://second.example/?q={searchTerms}",
};

test.each([
  { ...proxy, accepted: true },
  { ...proxy, accepted: false },
  { ...search, accepted: true },
])("the $label keeps newer edits after a save (accepted: $accepted)", async (scenario) => {
  const { component, label, key, first, second, accepted } = scenario;
  native.values.set("network.proxy", "http");
  let finish!: () => void;
  native.set.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => {
          if (accepted) native.values.set(key, first);
          resolve({ accepted, operation_id: null });
        };
      }),
  );
  await preferences.init();
  const screen = await render(component);
  const field = screen.getByRole("textbox", { name: label, exact: true });
  const save = screen.getByRole("button", { name: "Save", exact: true });
  await field.fill(first);
  await save.click();
  try {
    expect(native.set).toHaveBeenCalledExactlyOnceWith(key, first);
    await field.fill(second);
  } finally {
    finish?.();
  }
  await expect.element(save).toBeEnabled();
  await expect.element(field).toHaveValue(second);
  await expect.element(field).not.toHaveAttribute("aria-invalid", "true");
});
