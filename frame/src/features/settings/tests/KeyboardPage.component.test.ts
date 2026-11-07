import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import "$styles/global.css";
import type { KeymapEntry, LauncherTrigger } from "$shared/ipc/bindings";
import { keymap } from "$domain/keymap";
import KeyboardPage from "../components/sections/KeyboardPage.svelte";

const entry = (id: string, title: string, accelerator: string | null, customized = false) =>
  ({
    id,
    title,
    group: "file",
    accelerator,
    default_accelerator: accelerator,
    customizable: true,
    customized,
  }) satisfies KeymapEntry;

const trigger: LauncherTrigger = {
  shortcut: "CmdOrCtrl+Shift+Space",
  default_shortcut: "CmdOrCtrl+Shift+Space",
  registered: true,
  editable: true,
  double_tap: "off",
  double_tap_supported: false,
  accessibility: false,
};

const native = vi.hoisted(() => ({
  entries: vi.fn(),
  bind: vi.fn(),
  reset: vi.fn(),
  record: vi.fn(async (_active: boolean) => true),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    keymapEntries: native.entries,
    keymapBind: native.bind,
    keymapReset: native.reset,
    keymapRecord: native.record,
    launcherTrigger: async () => trigger,
    launcherRecordShortcut: async () => {},
  });
});

beforeEach(async () => {
  native.entries.mockResolvedValue([
    entry("tab.new", "New Tab", "Cmd+T"),
    entry("tab.close", "Close Tab", "Cmd+W"),
    entry("page.print", "Print…", "Cmd+Shift+P", true),
    { ...entry("profile.next", "Next Profile", "Ctrl+Cmd+P"), group: "keys" },
    { ...entry("profile.previous", "Previous Profile", "Ctrl+Cmd+Shift+P"), group: "keys" },
  ]);
  await keymap.init();
});
afterEach(() => {
  keymap.dispose();
  vi.clearAllMocks();
});

test("shows both profile shortcuts and rebinds the previous-profile command", async () => {
  native.bind.mockResolvedValue({ kind: "applied" });
  const screen = await render(KeyboardPage);
  await expect
    .element(screen.getByRole("heading", { name: "Profiles", exact: true }))
    .toBeVisible();
  await expect
    .element(screen.getByRole("button", { name: "Change the shortcut for Next profile" }))
    .toBeVisible();
  const previous = screen.getByRole("button", { name: "Change the shortcut for Previous profile" });
  await expect.element(previous).toBeVisible();
  await page.viewport(1000, 900);
  await page.screenshot({ path: "../../../../../target/settings-profile-shortcuts.png" });
  await previous.click();
  await userEvent.keyboard("{Control>}{Meta>}k{/Meta}{/Control}");
  await vi.waitFor(() =>
    expect(native.bind).toHaveBeenLastCalledWith("profile.previous", "Ctrl+Cmd+KeyK", false),
  );
});

test("records a shortcut, names a conflict, and takes the keys when asked", async () => {
  native.bind
    .mockResolvedValueOnce({ kind: "conflict", command: "tab.close" })
    .mockResolvedValueOnce({ kind: "applied" });
  const screen = await render(KeyboardPage);
  const newTab = screen.getByRole("button", { name: "Change the shortcut for New tab" });
  await expect.element(newTab).toHaveTextContent("⌘T");

  await newTab.click();
  expect(native.record).toHaveBeenLastCalledWith(true);
  await userEvent.keyboard("{Meta>}w{/Meta}");
  await vi.waitFor(() =>
    expect(native.bind).toHaveBeenLastCalledWith("tab.new", "Cmd+KeyW", false),
  );
  expect(native.record).toHaveBeenLastCalledWith(false);
  await expect.element(screen.getByText("Already used by Close tab.")).toBeVisible();

  await screen.getByRole("button", { name: "Use here" }).click();
  await vi.waitFor(() => expect(native.bind).toHaveBeenLastCalledWith("tab.new", "Cmd+KeyW", true));
  await expect.element(screen.getByText("Already used by Close tab.")).not.toBeInTheDocument();

  document.documentElement.dataset.theme = "dark";
  await page.viewport(720, 640);
  await page.screenshot({ path: "../../../../../target/settings-keyboard.png" });
  delete document.documentElement.dataset.theme;
});

test("Delete clears a shortcut, Escape keeps it, and Reset restores the default", async () => {
  native.bind.mockResolvedValue({ kind: "applied" });
  native.reset.mockResolvedValue(true);
  const screen = await render(KeyboardPage);
  const close = screen.getByRole("button", { name: "Change the shortcut for Close tab" });

  await close.click();
  await userEvent.keyboard("{Escape}");
  expect(native.bind).not.toHaveBeenCalled();
  await expect.element(close).toHaveAttribute("aria-pressed", "false");

  await close.click();
  await userEvent.keyboard("{Backspace}");
  await vi.waitFor(() => expect(native.bind).toHaveBeenLastCalledWith("tab.close", null, false));

  await screen.getByRole("button", { name: "Reset" }).click();
  await vi.waitFor(() => expect(native.reset).toHaveBeenLastCalledWith("page.print"));
});
