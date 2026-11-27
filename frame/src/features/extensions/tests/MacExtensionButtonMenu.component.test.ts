import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import type { ExtensionActionView } from "$shared/ipc/bindings";
import ExtensionActions from "../components/ExtensionActions.svelte";
import PinnedExtensions from "../components/PinnedExtensions.svelte";
import { chooseFromButtonMenu } from "../lib/button-menu";

const native = vi.hoisted(() => ({
  actionMenu: vi.fn(async (..._args: unknown[]) => ({ accepted: true })),
  popup: vi.fn(),
  requestRemoval: vi.fn(),
  catalogReady: true,
}));
vi.mock("$shared/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));
vi.mock("$shared/ipc/bindings", () => ({
  commands: {
    extensionActionMenu: native.actionMenu,
    extensionButtonMenuPopup: native.popup,
  },
}));
vi.mock("$domain/extensions", async (importOriginal) => {
  const { extensions } = await importOriginal<typeof import("$domain/extensions")>();
  return { extensions: { ...extensions, activeActions: () => [action] } };
});
vi.mock("$domain/tabs", () => ({
  ZERO_PROJECTION_REVISION: "0".repeat(32),
  tabs: { profile: () => ({ id: "profile" }), activeId: () => "tab" },
}));
vi.mock("$domain/webext", () => ({
  webext: {
    byInstall: (install: string) =>
      native.catalogReady && install === action.runtime.install_id ? extension : null,
    pinnedCount: () => 1,
    MAX_PINNED: 3,
    requestRemoval: native.requestRemoval,
  },
}));

const action: ExtensionActionView = {
  runtime: { install_id: "install-a", generation: "g" },
  revision: "r1",
  label: "Dark Reader",
  badge: "",
  unread_badge: false,
  enabled: true,
  presents_popup: true,
  icon_rgba_base64: null,
};
const extension = {
  id: "extension-install-a",
  name: "Dark Reader",
  enabled: true,
  pinned: true,
  has_options: true,
};

afterEach(() => {
  vi.clearAllMocks();
  native.catalogReady = true;
});

test("a macOS right click sends the browser entries and a relayed Remove opens confirmation", async () => {
  const screen = await render(PinnedExtensions);
  const button = screen.getByRole("button", { name: "Dark Reader" });
  await userEvent.click(button, { button: "right" });

  const { left, top, width, height } = button.element().getBoundingClientRect();
  const [profile, install, generation, revision, anchor, entries] =
    native.actionMenu.mock.calls[0] ?? [];
  expect(native.actionMenu).toHaveBeenCalledOnce();
  expect(native.popup).not.toHaveBeenCalled();
  expect([profile, install, generation, revision]).toEqual(["profile", "install-a", "g", "r1"]);
  expect(anchor).toEqual({ x: left, y: top, width, height });
  expect(entries).toEqual([
    { kind: "item", choice: "unpin", label: "Unpin from sidebar", enabled: true },
    { kind: "item", choice: "options", label: "Options", enabled: true },
    { kind: "separator" },
    { kind: "item", choice: "manage", label: "Manage extensions", enabled: true },
    { kind: "item", choice: "remove", label: "Remove…", enabled: true },
  ]);

  const openExtensions = vi.fn();
  expect(chooseFromButtonMenu("extension.menu=remove:install-a", openExtensions)).toBe(true);
  expect(native.requestRemoval).toHaveBeenCalledExactlyOnceWith(extension.id);
  expect(openExtensions).toHaveBeenCalledOnce();
});

test("a macOS tray action still requests WebKit's items while the install catalog loads", async () => {
  native.catalogReady = false;
  const screen = await render(ExtensionActions);
  await userEvent.click(screen.getByRole("button", { name: "Dark Reader" }), { button: "right" });

  expect(native.actionMenu).toHaveBeenCalledOnce();
  expect(native.actionMenu.mock.calls[0]?.[5]).toEqual([
    { kind: "item", choice: "manage", label: "Manage extensions", enabled: false },
  ]);
});
