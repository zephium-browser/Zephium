import * as m from "$shared/i18n/messages";
import { IS_MAC, IS_WINDOWS } from "$shared/platform";
import { acceleratorKeys } from "$shared/lib/accelerator";

export function shortcutHint(accelerator: string): string {
  return acceleratorKeys(accelerator, IS_MAC)
    .map((key) => (IS_MAC && key === "Backspace" ? "⌫" : key))
    .join(IS_MAC ? "" : "+");
}

export function deleteShortcut(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey">): boolean {
  return (
    (event.key === "Backspace" || event.key === "Delete") &&
    (IS_MAC ? event.metaKey : event.ctrlKey)
  );
}

/** The file manager's own name for showing a file in its folder. */
export function revealLabel(): string {
  if (IS_MAC) return m.note_reveal();
  if (IS_WINDOWS) return m.note_reveal_explorer();
  return m.note_reveal_folder_generic();
}
