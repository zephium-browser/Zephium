import { expect, test, vi } from "vitest";
import { flushSync } from "svelte";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import { tabFixture, revision } from "$shared/testing/fixtures";
import { emitNativeEvent } from "$shared/testing/native-events";
import { events } from "$shared/ipc/native-events";
import TabRow from "../components/TabRow.svelte";

test("presentation sentinels expose the exact title and revision synchronously", async () => {
  const tab = tabFixture({ title: "  <img onerror=unsafe()> & title  " });
  const screen = await render(TabRow, {
    tab,
    active: true,
    splitCandidate: false,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  });
  const row = screen.container.querySelector("[data-zephium-tab-id]")!;
  expect(row.getAttribute("data-zephium-tab-id")).toBe(tab.id);
  expect(row.getAttribute("data-zephium-tab-url")).toBe(tab.url);
  expect(row.getAttribute("data-zephium-projection-revision")).toBe(tab.projection_revision);
  const label = row.querySelector("[data-zephium-tab-label]")!;
  expect(label.textContent).toBe(tab.title);
  expect(label.childElementCount).toBe(0);
  // Validate the production scoped transport, including synchronous delivery.
  const received = vi.fn();
  const stop = await events.presentationTab.listen(received);
  const next = {
    tab: { ...tab, title: "Changed", projection_revision: revision(2) },
    active: tab.id,
  };
  flushSync(() => emitNativeEvent("presentationTab", next));
  expect(received).toHaveBeenCalledExactlyOnceWith({ payload: next });
  stop();
  emitNativeEvent("presentationTab", next);
  expect(received).toHaveBeenCalledTimes(1);
});

function renderRow(closable: boolean) {
  const tab = tabFixture({ title: "Docs" });
  const props = {
    tab,
    active: false,
    closable,
    splitCandidate: false,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  };
  return { tab, props, screen: render(TabRow, props) };
}

test("a middle-click closes a row that has a close button, and only that", async () => {
  const { tab, props, screen } = renderRow(true);
  const row = (await screen).getByRole("button", { name: "Docs", exact: true });
  await userEvent.click(row, { button: "middle" });
  expect(props.onClose).toHaveBeenCalledExactlyOnceWith(tab.id);
  expect(props.onSelect).not.toHaveBeenCalled();
  await userEvent.click(row, { button: "right" });
  expect(props.onClose).toHaveBeenCalledTimes(1);
});

test("a middle press does not start autoscroll", async () => {
  const { screen } = renderRow(true);
  const row = (await screen).getByRole("button", { name: "Docs", exact: true }).element();
  const press = (button: number) => {
    const event = new MouseEvent("mousedown", { button, bubbles: true, cancelable: true });
    row.dispatchEvent(event);
    return event.defaultPrevented;
  };
  expect(press(1)).toBe(true);
  expect(press(0)).toBe(false);
});

test("a middle-click leaves a pinned row open", async () => {
  const { props, screen } = renderRow(false);
  await userEvent.click((await screen).getByRole("button", { name: "Docs", exact: true }), {
    button: "middle",
  });
  expect(props.onClose).not.toHaveBeenCalled();
});
