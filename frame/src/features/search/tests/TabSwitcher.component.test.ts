import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import type { TabSwitcherState } from "$shared/ipc/bindings";
import TabSwitcher from "../components/TabSwitcher.svelte";

const native = vi.hoisted(() => ({ action: vi.fn(async (..._args: unknown[]) => true) }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ tabSwitcherAction: native.action });
});
afterEach(() => vi.clearAllMocks());
const state: TabSwitcherState = {
  session_id: "7",
  window_id: "1",
  selected: 1,
  keyboard: true,
  entries: Array.from({ length: 12 }, (_, index) => ({
    tab_id: String(index),
    title: `Tab ${index}`,
    url_host: "example.test",
    favicon: null,
    preview_url: null,
    active: index === 0,
  })),
};

test("keyboard selection sends typed intents and waits for native selection", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  await userEvent.keyboard("{Tab}{Shift>}{Tab}{/Shift}{ArrowLeft}{ArrowRight}");
  await vi.waitFor(() =>
    expect(native.action.mock.calls).toEqual([
      ["7", { type: "step", reverse: false }],
      ["7", { type: "step", reverse: true }],
      ["7", { type: "step", reverse: true }],
      ["7", { type: "step", reverse: false }],
    ]),
  );
  expect(
    screen
      .getByRole("option", { name: "Tab 1 example.test" })
      .element()
      .getAttribute("aria-selected"),
  ).toBe("true");
  await screen.rerender({ snapshot: { ...state, selected: 10 } });
  await expect.element(screen.getByRole("option", { name: "Tab 10 example.test" })).toBeVisible();
  expect(screen.container.querySelectorAll('[role="option"]')).toHaveLength(12);
});

test("Enter commits, Escape cancels, Ctrl W closes and a click selects before committing", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  await userEvent.keyboard("{Enter}{Escape}{Control>}w{/Control}");
  await vi.waitFor(() =>
    expect(native.action.mock.calls.slice(0, 3)).toEqual([
      ["7", { type: "commit" }],
      ["7", { type: "cancel" }],
      ["7", { type: "close" }],
    ]),
  );
  await screen.getByRole("option", { name: "Tab 2 example.test" }).click();
  await vi.waitFor(() =>
    expect(native.action.mock.calls.slice(-2)).toEqual([
      ["7", { type: "select", index: 2 }],
      ["7", { type: "commit" }],
    ]),
  );
});

test("Ctrl release commits only keyboard sessions and failures are visible", async () => {
  const screen = await render(TabSwitcher, { snapshot: { ...state, keyboard: false } });
  await userEvent.keyboard("{Control>}{/Control}");
  expect(native.action).not.toHaveBeenCalled();
  await screen.rerender({ snapshot: state });
  await userEvent.keyboard("{Control>}{/Control}");
  await vi.waitFor(() => expect(native.action).toHaveBeenCalledWith("7", { type: "commit" }));
  native.action.mockResolvedValueOnce(false);
  await userEvent.keyboard("{ArrowRight}");
  await expect.element(screen.getByRole("alert")).toBeVisible();
});

test("unmount removes previews and keyboard listeners", async () => {
  const screen = await render(TabSwitcher, {
    snapshot: {
      ...state,
      entries: state.entries.map((entry) => ({
        ...entry,
        preview_url:
          "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
      })),
    },
  });
  await vi.waitFor(() => {
    const count = screen.container.querySelectorAll("img").length;
    expect(count).toBeGreaterThan(0);
    expect(count).toBeLessThanOrEqual(7);
  });
  screen.unmount();
  await userEvent.keyboard("{Tab}{Enter}{Escape}");
  expect(native.action).not.toHaveBeenCalled();
  expect(screen.container.querySelectorAll("img")).toHaveLength(0);
});

test("a refused pointer selection does not commit the previously selected tab", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  native.action.mockResolvedValueOnce(false);
  await screen.getByRole("option", { name: "Tab 2 example.test" }).click();
  await expect.element(screen.getByRole("alert")).toBeVisible();
  expect(native.action).toHaveBeenCalledExactlyOnceWith("7", { type: "select", index: 2 });
});

test("leaving, the last state stays on screen and answers nothing", async () => {
  const screen = await render(TabSwitcher, { snapshot: state, frozen: true });
  await userEvent.keyboard("{Tab}{Enter}{Escape}{Control>}{/Control}");
  await screen.getByRole("option", { name: "Tab 2 example.test" }).click();
  expect(native.action).not.toHaveBeenCalled();
  expect(screen.container.querySelectorAll('[role="option"]')).toHaveLength(12);
});

test("a card's close control closes that tab and leaves the switcher open", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  await screen.getByRole("button", { name: "Close tab" }).nth(3).click();
  await vi.waitFor(() =>
    expect(native.action.mock.calls).toEqual([
      ["7", { type: "select", index: 3 }],
      ["7", { type: "close" }],
    ]),
  );
});

test("every card keeps its width, however long its title", async () => {
  const screen = await render(TabSwitcher, {
    snapshot: {
      ...state,
      entries: state.entries.map((entry, index) => ({
        ...entry,
        title: index % 2 ? "A very long title ".repeat(12) : "Short",
      })),
    },
  });
  const widths = [...screen.container.querySelectorAll<HTMLElement>(".card")].map(
    (card) => card.getBoundingClientRect().width,
  );
  expect(new Set(widths)).toEqual(new Set([204]));
});

test("cards stand four across, and up and down move a whole row", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  const tops = [...screen.container.querySelectorAll<HTMLElement>(".card")].map(
    (card) => card.getBoundingClientRect().top,
  );
  expect(new Set(tops.slice(0, 4)).size).toBe(1);
  expect(tops[4]).toBeGreaterThan(tops[0]!);
  await userEvent.keyboard("{ArrowDown}");
  await vi.waitFor(() =>
    expect(native.action.mock.calls).toEqual([["7", { type: "select", index: 5 }]]),
  );
  await screen.rerender({ snapshot: { ...state, selected: 0 } });
  await userEvent.keyboard("{ArrowUp}");
  await vi.waitFor(() =>
    expect(native.action.mock.calls.at(-1)).toEqual(["7", { type: "step", reverse: true }]),
  );
});

test("the new tab card after the tabs leaves for a new tab", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  await screen.getByRole("button", { name: "New Tab" }).click();
  await vi.waitFor(() => expect(native.action.mock.calls).toEqual([["7", { type: "new_tab" }]]));
});
