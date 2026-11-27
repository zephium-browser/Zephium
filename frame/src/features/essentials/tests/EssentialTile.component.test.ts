import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import { tabFixture } from "$shared/testing/fixtures";
import type { TabView } from "$shared/ipc/bindings";
import EssentialTile from "../components/EssentialTile.svelte";

const native = vi.hoisted(() => ({
  close: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ tabsClose: native.close });
});

afterEach(() => native.close.mockClear());

const SLEEPING: TabView["availability"] = { state: "sleeping" };

function tile(tab: TabView, active = false) {
  const props = {
    tab,
    active,
    splitCandidate: false,
    onSelect: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  };
  const screen = render(EssentialTile, props).then((view) => {
    // The dock gives a tile a share of its row, never its own content's width.
    view.container.querySelector<HTMLElement>(".essential")!.style.width = "52px";
    return view;
  });
  return { props, screen };
}

const closeOf = (container: Element) =>
  container.querySelector<HTMLButtonElement>(".essential-close");

test("an open Essential offers to end its page, and selecting it is not that", async () => {
  const { props, screen } = tile(tabFixture({ id: "mail", title: "Mail" }));
  const view = await screen;
  const close = closeOf(view.container)!;
  const shown = () => getComputedStyle(close).opacity;
  await expect.poll(shown).toBe("0");

  await userEvent.hover(view.getByRole("button", { name: "Mail", exact: true }));
  await expect.poll(shown).toBe("1");

  await userEvent.click(view.getByRole("button", { name: "Close Mail", exact: true }));
  expect(native.close).toHaveBeenCalledExactlyOnceWith("mail");
  expect(props.onSelect).not.toHaveBeenCalled();
});

test("a closed Essential only keeps its site: nothing to close", async () => {
  const { screen } = tile(tabFixture({ id: "mail", title: "Mail", availability: SLEEPING }));
  const view = await screen;
  expect(closeOf(view.container)).toBeNull();
  expect(view.container.querySelector(".essential")?.getAttribute("data-open")).toBe("false");
});

test("the close is reachable and shown from the keyboard", async () => {
  const { screen } = tile(tabFixture({ id: "mail", title: "Mail" }));
  const view = await screen;
  const close = closeOf(view.container)!;
  await expect.poll(() => getComputedStyle(close).opacity).toBe("0");
  close.focus();
  expect(document.activeElement).toBe(close);
  await expect.poll(() => getComputedStyle(close).opacity).toBe("1");

  await userEvent.keyboard("{Enter}");
  expect(native.close).toHaveBeenCalledExactlyOnceWith("mail");
});

test("a tile reads closed, open or in front, and the dot is only for a page", async () => {
  const state = async (tab: TabView, active: boolean) => {
    const view = await tile(tab, active).screen;
    const host = view.container.querySelector<HTMLElement>(".essential")!;
    const dot = getComputedStyle(host, "::after");
    const result = {
      open: host.dataset.open,
      active: host.dataset.active,
      dot: dot.content !== "none" && dot.content !== "normal",
    };
    view.unmount();
    return result;
  };
  expect(await state(tabFixture({ availability: SLEEPING }), false)).toEqual({
    open: "false",
    active: "false",
    dot: false,
  });
  expect(await state(tabFixture(), false)).toEqual({ open: "true", active: "false", dot: true });
  expect(await state(tabFixture(), true)).toEqual({ open: "true", active: "true", dot: true });
});

test("the close adds nothing to what native reads from the row", async () => {
  const tab = tabFixture({ id: "mail", title: "Mail" });
  const view = await tile(tab).screen;
  const row = view.container.querySelector("[data-zephium-tab-id]")!;
  expect(view.container.querySelectorAll("[data-zephium-tab-id]")).toHaveLength(1);
  expect(row.getAttribute("data-zephium-tab-url")).toBe(tab.url);
  const label = row.querySelector("[data-zephium-tab-label]")!;
  expect(label.textContent).toBe("Mail");
  expect(label.childElementCount).toBe(0);
  expect(row.querySelector(".essential-close [data-zephium-tab-label]")).toBeNull();
});
