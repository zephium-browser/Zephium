import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/mini.css";
import type { TabView } from "$shared/ipc/bindings";
import MiniToolbar from "../components/MiniToolbar.svelte";

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
  pinned: false,
  frame: "lights" as "lights" | "controls",
  onAction: vi.fn(),
  onSpaceMenu: vi.fn(),
});

/** The same reads native's presentation check makes before the page shows. */
function nativeCheck(root: ParentNode, expected: TabView): boolean {
  const shell = root.querySelector<HTMLElement>("[data-zephium-mini-tab]");
  if (!shell || shell.dataset.zephiumMiniTab !== expected.id) return false;
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
  const screen = await render(MiniToolbar, { ...props(), tab });
  expect(nativeCheck(screen.container, tab)).toBe(true);
  // An untrusted title is text, never markup.
  expect(screen.container.querySelector("b")).toBeNull();
});

test("the buttons ask for their page actions", async () => {
  const onAction = vi.fn();
  const screen = await render(MiniToolbar, { ...props(), tab, onAction });
  await expect.element(screen.getByRole("button", { name: "Forward" })).toBeDisabled();
  await screen.getByRole("button", { name: "Back" }).click();
  await screen.getByRole("button", { name: "Reload" }).click();
  await screen.getByRole("button", { name: "Open in Zephium" }).click();
  expect(onAction.mock.calls).toEqual([["back"], ["reload"], ["open_in_main"]]);
});

test("clicking the page's name asks to edit its address", async () => {
  const onAction = vi.fn();
  const screen = await render(MiniToolbar, { ...props(), tab, onAction });
  await screen.getByText("example.com").click();
  expect(onAction).toHaveBeenCalledWith("edit_address");
});

test("a page that can go to another space offers the choice", async () => {
  const onSpaceMenu = vi.fn();
  const one = await render(MiniToolbar, { ...props(), tab });
  expect(one.container.querySelector('[aria-haspopup="menu"]')).toBeNull();
  one.unmount();
  const screen = await render(MiniToolbar, { ...props(), tab, spaces: 2, onSpaceMenu });
  await screen.getByRole("button", { name: "Choose a space" }).click();
  expect(onSpaceMenu).toHaveBeenCalledOnce();
});

test("keep on top shows whether it is on and asks to toggle", async () => {
  const onAction = vi.fn();
  const off = await render(MiniToolbar, { ...props(), tab, onAction });
  const pin = off.getByRole("button", { name: "Keep on top" });
  await expect.element(pin).toHaveAttribute("aria-pressed", "false");
  await pin.click();
  expect(onAction).toHaveBeenCalledWith("keep_on_top");
  off.unmount();
  const on = await render(MiniToolbar, { ...props(), tab, pinned: true });
  await expect
    .element(on.getByRole("button", { name: "Keep on top" }))
    .toHaveAttribute("aria-pressed", "true");
});

test("only a frameless window draws its own minimize and close", async () => {
  const lights = await render(MiniToolbar, { ...props(), tab });
  expect(lights.container.querySelector(".window-button")).toBeNull();
  lights.unmount();
  const onAction = vi.fn();
  const screen = await render(MiniToolbar, { ...props(), tab, frame: "controls", onAction });
  await screen.getByRole("button", { name: "Minimize" }).click();
  await screen.getByRole("button", { name: "Close" }).click();
  expect(onAction.mock.calls).toEqual([["minimize"], ["close"]]);
});

test("a loading page shows a progress line, and nothing once it loaded", async () => {
  const loading = await render(MiniToolbar, { ...props(), tab: { ...tab, loading: true } });
  await expect.element(loading.getByRole("progressbar")).toBeInTheDocument();
  loading.unmount();
  const loaded = await render(MiniToolbar, { ...props(), tab });
  expect(loaded.container.querySelector('[role="progressbar"]')).toBeNull();
});

test("without a page, only reload's neighbours and the window stay usable", async () => {
  const screen = await render(MiniToolbar, { ...props(), frame: "controls" });
  await expect.element(screen.getByRole("button", { name: "Open in Zephium" })).toBeDisabled();
  await expect.element(screen.getByRole("button", { name: "Reload" })).toBeDisabled();
  await expect.element(screen.getByRole("button", { name: "Close" })).toBeEnabled();
  expect(screen.container.querySelector("[data-zephium-mini-tab]")).toBeNull();
});
