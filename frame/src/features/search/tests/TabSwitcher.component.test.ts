import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { commands, page, userEvent } from "vitest/browser";
import type { TabSwitcherState } from "$shared/ipc/bindings";
import "$styles/panel.css";
import TabSwitcher from "../components/TabSwitcher.svelte";

declare module "vitest/browser" {
  interface BrowserCommands {
    setForcedColors: (forcedColors: "active" | null) => Promise<void>;
  }
}

const native = vi.hoisted(() => ({ action: vi.fn(async (..._args: unknown[]) => true) }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ tabSwitcherAction: native.action });
});
afterEach(() => {
  vi.clearAllMocks();
  delete document.documentElement.dataset.platform;
  delete document.documentElement.dataset.material;
});
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

test("Enter commits, Escape cancels, Ctrl W closes and a click activates its own tab", async () => {
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
    expect(native.action.mock.calls.slice(-1)).toEqual([["7", { type: "activate", tab_id: "2" }]]),
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

test("a refused pointer choice says so and sends nothing else", async () => {
  const screen = await render(TabSwitcher, { snapshot: state });
  native.action.mockResolvedValueOnce(false);
  await screen.getByRole("option", { name: "Tab 2 example.test" }).click();
  await expect.element(screen.getByRole("alert")).toBeVisible();
  expect(native.action).toHaveBeenCalledExactlyOnceWith("7", { type: "activate", tab_id: "2" });
});

test("leaving, the last state stays on screen and answers nothing", async () => {
  const screen = await render(TabSwitcher, { snapshot: state, frozen: true });
  await userEvent.keyboard("{Tab}{Enter}{Escape}{Control>}{/Control}");
  await screen.getByRole("option", { name: "Tab 2 example.test" }).click();
  expect(native.action).not.toHaveBeenCalled();
  expect(screen.container.querySelectorAll('[role="option"]')).toHaveLength(12);
});

test("a card's close control names its own tab, whatever is chosen meanwhile", async () => {
  let reply: (value: boolean) => void = () => {};
  native.action.mockImplementationOnce(() => new Promise<boolean>((done) => (reply = done)));
  const screen = await render(TabSwitcher, { snapshot: state });
  await screen.getByRole("button", { name: "Close tab" }).nth(3).click();
  await screen.getByRole("option", { name: "Tab 1 example.test" }).click();
  reply(true);
  await vi.waitFor(() =>
    expect(native.action.mock.calls).toEqual([
      ["7", { type: "close_tab", tab_id: "3" }],
      ["7", { type: "activate", tab_id: "1" }],
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

const across = (screen: Awaited<ReturnType<typeof render>>) => {
  const tops = [...screen.container.querySelectorAll<HTMLElement>(".card")].map(
    (card) => card.getBoundingClientRect().top,
  );
  return tops.filter((top) => top === tops[0]).length;
};

test("a window too narrow for four stands as many cards across as fit", async () => {
  await page.viewport(470, 700);
  const screen = await render(TabSwitcher, { snapshot: state });
  await vi.waitFor(() => expect(across(screen)).toBe(2));
  await userEvent.keyboard("{ArrowDown}");
  await vi.waitFor(() =>
    expect(native.action.mock.calls).toEqual([["7", { type: "select", index: 3 }]]),
  );
});

test("cards stand four across, and up and down move a whole row", async () => {
  await page.viewport(900, 700);
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

const transparent = "rgba(0, 0, 0, 0)";

test("the Linux switcher rounds its plate and fades the scrolling cards", async ({
  onTestFinished,
}) => {
  const { documentElement, body } = document;
  const root = document.createElement("div");
  root.id = "root";
  body.append(root);
  onTestFinished(() => root.remove());
  documentElement.dataset.platform = "linux";
  const canvas = [documentElement, body, root];
  for (const layer of canvas) expect(getComputedStyle(layer).backgroundColor).not.toBe(transparent);

  documentElement.dataset.material = "none";
  await page.viewport(680, 200);
  const screen = await render(TabSwitcher, { snapshot: state });
  const plate = screen.getByRole("region").element();
  const cards = screen.getByRole("listbox").element();
  for (const layer of canvas) expect(getComputedStyle(layer).backgroundColor).toBe(transparent);
  expect(getComputedStyle(plate).borderTopLeftRadius).toBe("26px");
  expect(getComputedStyle(plate).backgroundColor).not.toBe(transparent);
  expect(getComputedStyle(plate).boxShadow).not.toBe("none");
  expect(getComputedStyle(plate).maskImage).toBe("none");
  expect(getComputedStyle(cards).backgroundColor).toBe(transparent);
  expect(getComputedStyle(cards).boxShadow).toBe("none");
  await expect.poll(() => getComputedStyle(cards).maskImage).toContain("linear-gradient");

  delete documentElement.dataset.platform;
  expect(getComputedStyle(plate).borderTopLeftRadius).toBe("0px");
  expect(getComputedStyle(cards).backgroundColor).not.toBe(transparent);
});

test("forced colors retain a solid Linux panel canvas", async ({ onTestFinished }) => {
  document.documentElement.dataset.platform = "linux";
  document.documentElement.dataset.material = "none";
  onTestFinished(() => commands.setForcedColors(null));
  await commands.setForcedColors("active");
  expect(matchMedia("(forced-colors: active)").matches).toBe(true);

  const screen = await render(TabSwitcher, { snapshot: state });
  const plate = screen.getByRole("region").element();
  expect(getComputedStyle(plate).borderTopLeftRadius).toBe("0px");
  expect(getComputedStyle(document.body).backgroundColor).not.toBe(transparent);
});
