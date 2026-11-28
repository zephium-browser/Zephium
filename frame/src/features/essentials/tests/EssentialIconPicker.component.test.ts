import { afterEach, expect, test, vi } from "vitest";
import { userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import EssentialIconPicker from "../components/EssentialIconPicker.svelte";

const native = vi.hoisted(() => ({
  setIcon: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ tabsSetTabIcon: native.setIcon });
});

afterEach(() => native.setIcon.mockClear());

test("the picker sends the chosen symbol and closes", async () => {
  const onclose = vi.fn();
  const screen = await render(EssentialIconPicker, {
    id: "mail",
    title: "Work mail",
    current: null,
    onclose,
  });
  const dialog = screen.getByRole("dialog", { name: "Change icon" });
  await expect.element(dialog).toBeVisible();
  // The page title is context, not a new name for the tile.
  expect(screen.container.textContent).toContain("Work mail");

  const symbol = screen.getByRole("radio", { name: "Symbol 4" });
  await userEvent.click(symbol);
  expect(symbol.element().getAttribute("aria-checked")).toBe("true");

  await userEvent.click(screen.getByRole("button", { name: "Done" }));
  expect(native.setIcon).toHaveBeenCalledExactlyOnceWith("mail", { kind: "symbol", index: 3 });
  expect(onclose).toHaveBeenCalledOnce();
});

test("typing one emoji offers it, and No icon clears the mark", async () => {
  const onclose = vi.fn();
  const screen = await render(EssentialIconPicker, {
    id: "mail",
    title: "Work mail",
    current: { kind: "emoji", text: "🚀" },
    onclose,
  });
  const field = screen.getByRole("textbox");
  await userEvent.fill(field, "🎨 more text");
  await userEvent.click(screen.getByRole("button", { name: "Done" }));
  expect(native.setIcon).toHaveBeenCalledExactlyOnceWith("mail", {
    kind: "emoji",
    text: "🎨",
  });

  native.setIcon.mockClear();
  await screen.unmount();
  const again = await render(EssentialIconPicker, {
    id: "mail",
    title: "Work mail",
    current: { kind: "emoji", text: "🚀" },
    onclose,
  });
  await userEvent.click(again.getByRole("radio", { name: "No icon" }));
  await userEvent.click(again.getByRole("button", { name: "Done" }));
  expect(native.setIcon).toHaveBeenCalledExactlyOnceWith("mail", null);
});

test("Cancel leaves the mark untouched", async () => {
  const onclose = vi.fn();
  const screen = await render(EssentialIconPicker, {
    id: "mail",
    title: "Work mail",
    current: { kind: "emoji", text: "🚀" },
    onclose,
  });
  await userEvent.click(screen.getByRole("radio", { name: "Symbol 2", exact: true }));
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(native.setIcon).not.toHaveBeenCalled();
  expect(onclose).toHaveBeenCalledOnce();
});
