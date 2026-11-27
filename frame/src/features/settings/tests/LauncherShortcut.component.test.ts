import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { tick } from "svelte";
import "$styles/global.css";
import type { LauncherTrigger, TriggerChange } from "$shared/ipc/bindings";
import LauncherShortcut from "../components/LauncherShortcut.svelte";

// These tests read the macOS keycaps (⌘, ⌥, ⇧); Windows spells its shortcuts out.
vi.mock("$shared/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));

const trigger = (shortcut: string): LauncherTrigger => ({
  shortcut,
  default_shortcut: "CmdOrCtrl+Shift+Space",
  registered: true,
  editable: true,
  ownership_available: true,
  double_tap: "off",
  double_tap_supported: true,
  accessibility: false,
});
const native = vi.hoisted(() => ({
  state: vi.fn(),
  set: vi.fn(),
  record: vi.fn(async (_active: boolean) => {}),
  doubleTap: vi.fn(),
  accessibility: vi.fn(async () => {}),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    launcherTrigger: native.state,
    launcherSetShortcut: native.set,
    launcherRecordShortcut: native.record,
    launcherSetDoubleTap: native.doubleTap,
    launcherOpenAccessibility: native.accessibility,
  });
});
afterEach(() => vi.clearAllMocks());

test("records a shortcut, keeps listening after a refusal, and applies the next", async () => {
  native.state.mockResolvedValue(trigger("CmdOrCtrl+Shift+Space"));
  native.set
    .mockResolvedValueOnce({
      type: "rejected",
      reason: "app_command",
      trigger: trigger("CmdOrCtrl+Shift+Space"),
    })
    .mockResolvedValueOnce({ type: "applied", trigger: trigger("Ctrl+Alt+Space") });
  const screen = await render(LauncherShortcut);
  const recorder = screen.getByRole("button", { name: "Record a shortcut" });
  await expect.element(recorder).toHaveTextContent("⇧⌘Space");

  await recorder.click();
  expect(native.record).toHaveBeenLastCalledWith(true);
  await userEvent.keyboard("{Meta>}{Shift>}z{/Shift}{/Meta}");
  await expect.element(screen.getByText(/like Redo/u)).toBeVisible();
  // Still listening: the next press is taken without another click.
  await userEvent.keyboard("{Control>}{Alt>}{Space}{/Alt}{/Control}");
  await vi.waitFor(() => expect(native.set).toHaveBeenLastCalledWith("Ctrl+Alt+Space"));
  await expect
    .element(screen.getByRole("button", { name: "Record a shortcut" }))
    .toHaveTextContent("⌃⌥Space");
  await expect.element(screen.getByRole("button", { name: "Reset" })).toBeVisible();
  document.documentElement.dataset.theme = "dark";
  await page.viewport(720, 220);
  await page.screenshot({ path: "../../../../../target/settings-launcher-shortcut.png" });
  delete document.documentElement.dataset.theme;
});

test("refreshes the recorder when this instance takes shortcut ownership", async () => {
  native.state
    .mockResolvedValueOnce({ ...trigger("CmdOrCtrl+Shift+Space"), editable: false, ownership_available: false })
    .mockResolvedValue({ ...trigger("CmdOrCtrl+Shift+Space") });
  const screen = await render(LauncherShortcut);
  const recorder = screen.getByRole("button", { name: "Record a shortcut" });
  await expect.element(recorder).toBeDisabled();
  await expect
    .element(screen.getByText("Only one Zephium instance can change the launcher shortcut at a time."))
    .toBeVisible();
  await vi.waitFor(() => expect(recorder).toBeEnabled(), { timeout: 2500 });
  expect(native.state).toHaveBeenCalledTimes(2);
});

test("keeps refreshing until the owned shortcut is registered", async () => {
  native.state
    .mockResolvedValueOnce({ ...trigger("CmdOrCtrl+Shift+Space"), registered: false })
    .mockResolvedValue({ ...trigger("CmdOrCtrl+Shift+Space"), registered: true });
  const screen = await render(LauncherShortcut);
  await expect
    .element(screen.getByText("Not active: another app may be using it. Record a different one."))
    .toBeVisible();
  await vi.waitFor(() => expect(native.state).toHaveBeenCalledTimes(2), { timeout: 2500 });
  await expect.element(screen.getByText("Works from any app.")).toBeVisible();
});

test("closing settings during a save releases the launcher even if the shortcut is refused", async () => {
  const original = trigger("CmdOrCtrl+Shift+Space");
  native.state.mockResolvedValue(original);
  let finish!: (change: TriggerChange) => void;
  const result = new Promise<TriggerChange>((resolve) => (finish = resolve));
  native.set.mockReturnValueOnce(result);

  const screen = await render(LauncherShortcut);
  await screen.getByRole("button", { name: "Record a shortcut" }).click();
  await userEvent.keyboard("{Meta>}{Shift>}z{/Shift}{/Meta}");
  expect(native.set).toHaveBeenCalledOnce();
  await screen.unmount();
  finish({ type: "rejected", reason: "app_command", trigger: original });
  await result;
  await tick();

  expect(native.record.mock.calls).toEqual([[true], [false]]);
});

test("composition keys leave launcher shortcut recording active", async () => {
  native.state.mockResolvedValue(trigger("CmdOrCtrl+Shift+Space"));
  native.set.mockResolvedValue({ type: "applied", trigger: trigger("Ctrl+Alt+Space") });
  const screen = await render(LauncherShortcut);
  await screen.getByRole("button", { name: "Record a shortcut" }).click();
  const recorder = screen.getByRole("button", { name: "Press the new shortcut. Esc cancels." });

  for (const event of [
    { key: "Escape", isComposing: true },
    { key: "k", code: "KeyK", ctrlKey: true, keyCode: 229 },
  ]) {
    expect(
      recorder
        .element()
        .dispatchEvent(new KeyboardEvent("keydown", { ...event, bubbles: true, cancelable: true })),
    ).toBe(true);
    await expect.element(recorder).toHaveAttribute("aria-pressed", "true");
  }
  expect(native.set).not.toHaveBeenCalled();
  expect(native.record.mock.calls).toEqual([[true]]);

  await userEvent.keyboard("{Control>}{Alt>}{Space}{/Alt}{/Control}");
  expect(native.set).toHaveBeenLastCalledWith("Ctrl+Alt+Space");
});

test("double tap says what it is waiting for, and where to grant it", async () => {
  native.state.mockResolvedValue({ ...trigger("CmdOrCtrl+Shift+Space"), double_tap: "command" });
  const screen = await render(LauncherShortcut);
  await expect.element(screen.getByText(/Needs Accessibility permission/u)).toBeVisible();
  await screen.getByRole("button", { name: "Open System Settings" }).click();
  expect(native.accessibility).toHaveBeenCalledOnce();

  // Granted in System Settings: the page notices on its own.
  native.state.mockResolvedValue({
    ...trigger("CmdOrCtrl+Shift+Space"),
    double_tap: "command",
    accessibility: true,
  });
  await expect
    .element(screen.getByText(/never clashes with a shortcut/u), { timeout: 4000 })
    .toBeVisible();
});
