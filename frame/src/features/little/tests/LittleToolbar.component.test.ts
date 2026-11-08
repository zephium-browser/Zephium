import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/little.css";
import type { TabView } from "$shared/ipc/bindings";
import LittleToolbar from "../components/LittleToolbar.svelte";

const tab: TabView = {
  id: "0190a5f2-0000-7000-8000-000000000001",
  projection_revision: "0000000000000000000000000000002a",
  title: "<b>Example</b>",
  url: "https://example.com/path",
  loading: false,
  can_go_back: true,
  can_go_forward: false,
  icon: null,
};

const props = () => ({
  tab: null as TabView | null,
  spaces: 1,
  onAction: vi.fn(),
  onSpaceMenu: vi.fn(),
});

/** The same reads native's presentation check makes before the page shows. */
function nativeCheck(root: ParentNode, expected: TabView): boolean {
  const shell = root.querySelector<HTMLElement>("[data-zephium-little-tab]");
  if (!shell || shell.dataset.zephiumLittleTab !== expected.id) return false;
  if (
    shell.dataset.zephiumTabUrl !== (expected.url ?? "") ||
    shell.dataset.zephiumProjectionRevision !== expected.projection_revision
  )
    return false;
  const label = shell.querySelector("[data-zephium-tab-label]");
  if (!label || label.textContent !== expected.title) return false;
  const address = shell.querySelector("[data-zephium-address]");
  return address?.textContent === new URL(expected.url!).host;
}

test("the toolbar shows the page so native's presentation check passes", async () => {
  const screen = await render(LittleToolbar, { ...props(), tab });
  expect(nativeCheck(screen.container, tab)).toBe(true);
  // An untrusted title is text, never markup.
  expect(screen.container.querySelector("b")).toBeNull();
});

test("the buttons ask for their page actions", async () => {
  const onAction = vi.fn();
  const screen = await render(LittleToolbar, { ...props(), tab, onAction });
  await expect.element(screen.getByRole("button", { name: "Forward" })).toBeDisabled();
  await screen.getByRole("button", { name: "Back" }).click();
  await screen.getByRole("button", { name: "Reload" }).click();
  await screen.getByRole("button", { name: "Open in Zephium" }).click();
  expect(onAction.mock.calls).toEqual([["back"], ["reload"], ["open_in_main"]]);
});

test("clicking the page's name asks to edit its address", async () => {
  const onAction = vi.fn();
  const screen = await render(LittleToolbar, { ...props(), tab, onAction });
  await screen.getByText("example.com").click();
  expect(onAction).toHaveBeenCalledWith("edit_address");
});

test("a page that can go to another space offers the choice", async () => {
  const onSpaceMenu = vi.fn();
  const one = await render(LittleToolbar, { ...props(), tab });
  expect(one.container.querySelector('[aria-haspopup="menu"]')).toBeNull();
  one.unmount();
  const screen = await render(LittleToolbar, { ...props(), tab, spaces: 2, onSpaceMenu });
  await screen.getByRole("button", { name: "Move to Space" }).click();
  expect(onSpaceMenu).toHaveBeenCalledOnce();
});
