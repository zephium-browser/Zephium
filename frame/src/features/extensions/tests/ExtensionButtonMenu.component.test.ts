import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import PinnedExtensions from "../components/PinnedExtensions.svelte";
import { buttonMenuChoice, chooseFromButtonMenu } from "../lib/button-menu";

// Windows (#532): WebView2 lists none of an extension's own action items, so a
// right click asked the engine for a menu it refused, and nothing opened.
const native = vi.hoisted(() => ({
  popup: vi.fn(async (..._args: unknown[]) => true),
  engineMenu: vi.fn(),
  extensions: new Map<string, Record<string, unknown>>(),
  pinnedCount: 1,
  setPinned: vi.fn(async () => {}),
  openOptions: vi.fn(async () => {}),
  requestRemoval: vi.fn(),
}));
vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));
vi.mock("$shared/ipc/bindings", () => ({
  commands: { extensionButtonMenuPopup: native.popup },
}));
vi.mock("$domain/webext", () => ({
  webext: {
    byInstall: (install: string) => native.extensions.get(install) ?? null,
    pinnedCount: () => native.pinnedCount,
    MAX_PINNED: 3,
    setPinned: native.setPinned,
    openOptions: native.openOptions,
    requestRemoval: native.requestRemoval,
  },
}));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "profile" }), activeId: () => "tab" },
}));
vi.mock("$domain/extensions", () => ({
  extensions: {
    activeActions: () => [action("install-a")],
    isInvoking: () => false,
    invoke: vi.fn(),
    menu: native.engineMenu,
  },
}));

function action(install: string) {
  return {
    runtime: { install_id: install, generation: "g" },
    revision: "r1",
    label: "Dark Reader",
    badge: "",
    unread_badge: false,
    enabled: true,
    presents_popup: true,
    icon_rgba_base64: null,
  };
}

function installed(install: string, overrides: Record<string, unknown> = {}) {
  native.extensions.set(install, {
    id: `extension-${install}`,
    install_id: install,
    name: "Dark Reader",
    enabled: true,
    pinned: true,
    has_options: true,
    ...overrides,
  });
}

afterEach(() => {
  vi.clearAllMocks();
  native.extensions.clear();
  native.pinnedCount = 1;
});

test("right-clicking a pinned extension opens the browser's own entries under it", async () => {
  installed("install-a");
  const screen = await render(PinnedExtensions);
  const button = screen.getByRole("button", { name: "Dark Reader" });
  await userEvent.click(button, { button: "right" });

  expect(native.engineMenu).not.toHaveBeenCalled();
  expect(native.popup).toHaveBeenCalledOnce();
  const [install, entries, x, y] = native.popup.mock.calls[0] ?? [];
  const rect = button.element().getBoundingClientRect();
  expect(install).toBe("install-a");
  expect([x, y]).toEqual([rect.left, rect.bottom]);
  expect(entries).toEqual([
    { kind: "item", choice: "unpin", label: "Unpin from sidebar", enabled: true },
    { kind: "item", choice: "options", label: "Options", enabled: true },
    { kind: "separator" },
    { kind: "item", choice: "manage", label: "Manage extensions", enabled: true },
    { kind: "item", choice: "remove", label: "Remove…", enabled: true },
  ]);
});

test("an unpinned extension offers to pin while there is room, and no options it lacks", async () => {
  installed("install-a", { pinned: false, has_options: false });
  // The tray's rows render the same openMenu; drive the helper directly.
  const { openButtonMenu } = await import("../lib/button-menu");
  const anchor = new DOMRect(10, 20, 30, 30);

  native.pinnedCount = 2;
  openButtonMenu("profile", action("install-a") as never, anchor);
  native.pinnedCount = 3;
  openButtonMenu("profile", action("install-a") as never, anchor);

  const [room, full] = native.popup.mock.calls.map((call) => call[1]);
  expect(room).toEqual([
    { kind: "item", choice: "pin", label: "Pin to sidebar", enabled: true },
    { kind: "separator" },
    { kind: "item", choice: "manage", label: "Manage extensions", enabled: true },
    { kind: "item", choice: "remove", label: "Remove…", enabled: true },
  ]);
  expect((full as unknown[])[0]).toEqual({
    kind: "item",
    choice: "pin",
    label: "Unpin one to pin another",
    enabled: false,
  });
  expect(native.popup.mock.calls[0]?.slice(2)).toEqual([10, 50]);
});

test("an extension gone before the right click opens nothing", async () => {
  const { openButtonMenu } = await import("../lib/button-menu");
  openButtonMenu("profile", action("install-gone") as never, new DOMRect());
  expect(native.popup).not.toHaveBeenCalled();
});

test("each relayed choice acts on that extension", () => {
  installed("install-a");
  const openExtensions = vi.fn();

  expect(chooseFromButtonMenu("extension.menu=unpin:install-a", openExtensions)).toBe(true);
  expect(native.setPinned).toHaveBeenLastCalledWith("extension-install-a", false);
  expect(chooseFromButtonMenu("extension.menu=pin:install-a", openExtensions)).toBe(true);
  expect(native.setPinned).toHaveBeenLastCalledWith("extension-install-a", true);
  chooseFromButtonMenu("extension.menu=options:install-a", openExtensions);
  expect(native.openOptions).toHaveBeenCalledExactlyOnceWith("extension-install-a");
  expect(openExtensions).not.toHaveBeenCalled();

  chooseFromButtonMenu("extension.menu=manage:install-a", openExtensions);
  expect(openExtensions).toHaveBeenCalledOnce();
  expect(native.requestRemoval).not.toHaveBeenCalled();

  // Remove… asks on the Extensions page, never removes from the menu.
  chooseFromButtonMenu("extension.menu=remove:install-a", openExtensions);
  expect(native.requestRemoval).toHaveBeenCalledExactlyOnceWith("extension-install-a");
  expect(openExtensions).toHaveBeenCalledTimes(2);
});

test("a malformed or stale choice does nothing", () => {
  installed("install-a");
  const openExtensions = vi.fn();
  for (const command of [
    "extensions.manage",
    "extension.action=install-a",
    "extension.menu=",
    "extension.menu=remove",
    "extension.menu=remove:",
    "extension.menu=:install-a",
    "extension.menu=uninstall:install-a",
    "extension.menu=Remove:install-a",
  ]) {
    expect(buttonMenuChoice(command), command).toBeNull();
    expect(chooseFromButtonMenu(command, openExtensions), command).toBe(false);
  }
  // A choice for an extension removed since the menu opened is spent quietly.
  expect(chooseFromButtonMenu("extension.menu=remove:install-gone", openExtensions)).toBe(true);
  expect(native.setPinned).not.toHaveBeenCalled();
  expect(native.openOptions).not.toHaveBeenCalled();
  expect(native.requestRemoval).not.toHaveBeenCalled();
  expect(openExtensions).not.toHaveBeenCalled();
});
