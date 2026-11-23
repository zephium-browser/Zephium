import {
  commands,
  type ExtensionActionView,
  type ExtensionButtonChoice,
  type ExtensionButtonMenuEntry,
  type WebExtensionView,
} from "$shared/ipc/bindings";
import { extensions } from "$domain/extensions";
import { webext } from "$domain/webext";
import { IS_MAC } from "$shared/platform";
import * as m from "$shared/i18n/messages";

/** How native names a choice made in the menu, followed by `<choice>:<install>`. */
const CHOSEN = "extension.menu=";
const CHOICES: readonly ExtensionButtonChoice[] = ["pin", "unpin", "options", "manage", "remove"];

/**
 * The browser's own entries for an extension's toolbar button, which follow
 * the extension's own items where the platform lists them. Every platform
 * draws this one list; native only draws it and relays the choice.
 */
function buttonMenuEntries(
  extension: WebExtensionView | null,
  roomToPin: boolean,
): ExtensionButtonMenuEntry[] {
  const manage = {
    kind: "item",
    choice: "manage",
    label: m.webext_manage(),
    enabled: extension !== null,
  } satisfies ExtensionButtonMenuEntry;
  // The action snapshot can reach the tray before the install catalog.
  if (!extension) return [manage];
  return [
    extension.pinned
      ? { kind: "item", choice: "unpin", label: m.webext_unpin(), enabled: true }
      : {
          kind: "item",
          choice: "pin",
          label: roomToPin ? m.webext_pin() : m.webext_pin_full(),
          enabled: roomToPin,
        },
    ...(extension.has_options && extension.enabled
      ? [
          {
            kind: "item",
            choice: "options",
            label: m.webext_options(),
            enabled: true,
          } satisfies ExtensionButtonMenuEntry,
        ]
      : []),
    { kind: "separator" },
    manage,
    { kind: "item", choice: "remove", label: `${m.webext_remove()}…`, enabled: true },
  ];
}

/** A right click on an extension's toolbar button, pinned or in the tray. */
export function openButtonMenu(
  profileId: string,
  action: ExtensionActionView,
  anchor: DOMRect,
): void {
  const install = action.runtime.install_id;
  const extension = webext.byInstall(install);
  if (!extension && !IS_MAC) return;
  const entries = buttonMenuEntries(extension, webext.pinnedCount() < webext.MAX_PINNED);
  // WebKit adds the extension's own `contexts: ["action"]` items.
  if (IS_MAC) {
    extensions.menu(profileId, action, anchor, entries);
    return;
  }
  void commands
    .extensionButtonMenuPopup(install, entries, anchor.left, anchor.bottom)
    .catch(() => {});
}

/** The choice a native menu relayed, or null when `command` is not one. */
export function buttonMenuChoice(
  command: string,
): { choice: ExtensionButtonChoice; install: string } | null {
  if (!command.startsWith(CHOSEN)) return null;
  const rest = command.slice(CHOSEN.length);
  const split = rest.indexOf(":");
  if (split <= 0) return null;
  const choice = CHOICES.find((known) => known === rest.slice(0, split));
  const install = rest.slice(split + 1);
  return choice && install ? { choice, install } : null;
}

/**
 * Acts on a choice from an extension's button menu. Returns whether
 * `command` was one; an extension gone since the menu opened is left alone.
 */
export function chooseFromButtonMenu(command: string, openExtensions: () => void): boolean {
  const chosen = buttonMenuChoice(command);
  if (!chosen) return false;
  const extension = webext.byInstall(chosen.install);
  if (!extension) return true;
  switch (chosen.choice) {
    case "pin":
    case "unpin":
      void webext.setPinned(extension.id, chosen.choice === "pin");
      break;
    case "options":
      void webext.openOptions(extension.id);
      break;
    case "manage":
      openExtensions();
      break;
    case "remove":
      // Its row on the Extensions page asks to confirm, as there.
      webext.requestRemoval(extension.id);
      openExtensions();
      break;
  }
  return true;
}
