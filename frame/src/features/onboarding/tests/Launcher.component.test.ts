import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { acceleratorKeys } from "$shared/lib/accelerator";
import { IS_MAC } from "$shared/platform";
import Launcher from "../components/steps/Launcher.svelte";

const native = vi.hoisted(() => ({ location: "CmdOrCtrl+L" as string | null, registered: true }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    launcherTrigger: async () => ({
      shortcut: "CmdOrCtrl+Shift+Space",
      default_shortcut: "CmdOrCtrl+Shift+Space",
      registered: native.registered,
      editable: true,
      double_tap: "off",
      double_tap_supported: true,
      accessibility: false,
    }),
    keymapEntries: async () => [
      {
        id: "url.focus",
        title: "Open Location",
        group: "view",
        accelerator: native.location,
        default_accelerator: "CmdOrCtrl+L",
        customizable: true,
        customized: native.location !== "CmdOrCtrl+L",
      },
    ],
  });
});

beforeEach(() => {
  native.location = "CmdOrCtrl+L";
  native.registered = true;
});

test("distinguishes the global launcher shortcut from the current page-address shortcut", async () => {
  const screen = await render(Launcher);
  await expect.element(screen.getByText("Try it now, from any app.")).toBeVisible();
  await expect
    .element(
      screen.getByLabelText(acceleratorKeys("CmdOrCtrl+Shift+Space", IS_MAC).join(" "), {
        exact: true,
      }),
    )
    .toBeVisible();
  await expect.element(screen.getByText("For a page’s address in Zephium.")).toBeVisible();
  await expect
    .element(
      screen.getByLabelText(acceleratorKeys("CmdOrCtrl+L", IS_MAC).join(" "), { exact: true }),
    )
    .toBeVisible();
});

test("shows a remapped page shortcut even when the global shortcut is unavailable", async () => {
  native.location = "CmdOrCtrl+Alt+L";
  native.registered = false;
  const screen = await render(Launcher);
  await expect
    .element(
      screen.getByLabelText(acceleratorKeys(native.location, IS_MAC).join(" "), { exact: true }),
    )
    .toBeVisible();
  await expect.element(screen.getByText("Try it now, from any app.")).not.toBeInTheDocument();
});

test("does not advertise an unbound page shortcut", async () => {
  native.location = null;
  const screen = await render(Launcher);
  await expect.element(screen.getByText("Try it now, from any app.")).toBeVisible();
  await expect
    .element(screen.getByText("For a page’s address in Zephium."))
    .not.toBeInTheDocument();
});

test("highlights the pressed page-address keys in their displayed order", async () => {
  native.location = "CmdOrCtrl+Alt+L";
  const screen = await render(Launcher);
  const keys = screen.getByLabelText(acceleratorKeys(native.location, IS_MAC).join(" "), {
    exact: true,
  });
  await expect.element(keys).toBeVisible();
  window.dispatchEvent(
    new KeyboardEvent("keydown", {
      code: "KeyL",
      metaKey: IS_MAC,
      ctrlKey: !IS_MAC,
    }),
  );
  await expect.element(keys.getByText("L", { exact: true })).toHaveAttribute("data-down", "true");
  await expect
    .element(keys.getByText(IS_MAC ? "⌘" : "Ctrl", { exact: true }))
    .toHaveAttribute("data-down", "true");
  await expect
    .element(keys.getByText(IS_MAC ? "⌥" : "Alt", { exact: true }))
    .toHaveAttribute("data-down", "false");
  window.dispatchEvent(new KeyboardEvent("keyup", { code: "KeyL" }));
  await expect.element(keys.getByText("L", { exact: true })).toHaveAttribute("data-down", "false");
});
