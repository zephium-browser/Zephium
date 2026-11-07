import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import * as find from "../lib/find.svelte";
import AddressField from "../components/AddressField.svelte";

const native = vi.hoisted(() => ({ find: vi.fn(async () => true) }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    pageFind: native.find,
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
  });
});

afterEach(() => {
  find.dispose();
  tabs.dispose();
  vi.clearAllMocks();
});

async function field() {
  await tabs.init();
  const tab = tabFixture({ url: "https://www.wikipedia.org/wiki/Premium" });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(Date.now()),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  });
  const screen = await render(AddressField);
  const address = screen.container.querySelector<HTMLInputElement>("[data-zephium-address]")!;
  return { screen, address, tab };
}

test("the field becomes the search and keeps the page's address underneath", async () => {
  const { screen, address, tab } = await field();
  find.show(tab.id);
  const search = screen.getByRole("searchbox", { name: "Find on page" });
  await expect.element(search).toHaveFocus();
  await userEvent.type(search, "premium");
  await vi.waitFor(() => expect(native.find).toHaveBeenLastCalledWith("premium", true));
  // Native still reads the authoritative host through the address input.
  expect(address.value).toBe("www.wikipedia.org");

  emitNativeEvent("find", { query: "premium", matches: 4, active: 1 });
  await expect.element(screen.getByText("1 of 4")).toBeVisible();
  emitNativeEvent("find", { query: "prem", matches: 9, active: 2 });
  await expect.element(screen.getByText("1 of 4")).toBeVisible();

  await userEvent.keyboard("{Shift>}{Enter}{/Shift}");
  expect(native.find).toHaveBeenLastCalledWith("premium", false);
  await userEvent.keyboard("{Enter}");
  expect(native.find).toHaveBeenLastCalledWith("premium", true);

  document.documentElement.dataset.theme = "dark";
  await page.viewport(300, 60);
  await page.screenshot({ path: "../../../../../target/address-find.png" });
  delete document.documentElement.dataset.theme;

  await userEvent.keyboard("{Escape}");
  expect(native.find).toHaveBeenLastCalledWith(null, true);
  await expect.element(screen.getByRole("searchbox")).not.toBeInTheDocument();
});

test("find next with the field closed picks up the last search", async () => {
  const { screen, tab } = await field();
  find.show(tab.id);
  await userEvent.type(screen.getByRole("searchbox"), "wiki");
  await vi.waitFor(() => expect(native.find).toHaveBeenLastCalledWith("wiki", true));
  find.hide();
  find.step(true, tab.id);
  expect(native.find).toHaveBeenLastCalledWith("wiki", true);
  await expect.element(screen.getByRole("searchbox")).toHaveValue("wiki");
  emitNativeEvent("find", { query: "wiki", matches: 0, active: null });
  await expect.element(screen.getByText("No results")).toBeVisible();
});
