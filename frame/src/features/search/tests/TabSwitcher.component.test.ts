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
