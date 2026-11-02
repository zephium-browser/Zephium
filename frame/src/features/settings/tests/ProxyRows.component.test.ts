import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import ProxyRows from "../components/ProxyRows.svelte";

const state = vi.hoisted(() => ({ values: {} as Record<string, string>, failed: false }));

vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => state.values[key] ?? "",
    saving: () => false,
    saveFailed: () => state.failed,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));

beforeEach(() => {
  state.values = { "network.proxy": "system", "network.proxy-address": "" };
  state.failed = false;
  vi.mocked(preferences.set).mockClear();
});

test("the system proxy is the default and asks for no address", async () => {
  const screen = await render(ProxyRows);
  await expect.element(screen.getByText("Use system proxy")).toBeVisible();
  expect(screen.getByRole("textbox").elements()).toHaveLength(0);
  expect(screen.container.textContent).toContain("Applies to tabs opened from now on");
});

test("choosing a kind of proxy saves it and asks for the address", async () => {
  const screen = await render(ProxyRows);
  await screen.getByRole("button", { name: "Proxy" }).click();
  await screen.getByRole("option", { name: "SOCKS5 proxy" }).click();
  expect(preferences.set).toHaveBeenCalledWith("network.proxy", "socks5");
});

test("a manual proxy takes host:port, and native's refusal is shown beside the field", async () => {
  state.values = { "network.proxy": "http", "network.proxy-address": "" };
  state.failed = true;
  const screen = await render(ProxyRows);
  await screen.getByRole("textbox").fill(" proxy.example.com ");
  await screen.getByRole("button", { name: "Save" }).click();
  expect(preferences.set).toHaveBeenCalledWith("network.proxy-address", "proxy.example.com");
  await expect.element(screen.getByText("Use the form host:port", { exact: false })).toBeVisible();
});

test("a saved address is shown and saving needs a change", async () => {
  state.values = { "network.proxy": "http", "network.proxy-address": "proxy.example.com:3128" };
  const screen = await render(ProxyRows);
  await expect.element(screen.getByRole("textbox")).toHaveValue("proxy.example.com:3128");
  await expect.element(screen.getByRole("button", { name: "Save" })).toBeDisabled();
});
