import { acceleratorKeys } from "$shared/lib/accelerator";

/** What the keyboard is holding right now, as far as a key event reports. */
export type Held = {
  meta: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  codes: ReadonlySet<string>;
};

export const NOTHING_HELD: Held = {
  meta: false,
  ctrl: false,
  alt: false,
  shift: false,
  codes: new Set(),
};

/** For each part of an accelerator such as `CmdOrCtrl+Shift+Space`, whether
 *  that key is down, so each keycap can light as it is pressed. */
export function heldParts(accelerator: string, held: Held, mac: boolean): boolean[] {
  const codes = new Set([...held.codes].flatMap((code) => acceleratorKeys(code, mac)));
  return acceleratorKeys(accelerator, mac).map((part) => {
    switch (part) {
      case "⌘":
      case "Win":
        return held.meta;
      case "⌃":
      case "Ctrl":
        return held.ctrl;
      case "⌥":
      case "Alt":
        return held.alt;
      case "⇧":
      case "Shift":
        return held.shift;
      default:
        return codes.has(part);
    }
  });
}

export function heldFrom(event: KeyboardEvent, previous: Held): Held {
  const codes = new Set(previous.codes);
  if (event.type === "keydown") codes.add(event.code);
  else codes.delete(event.code);
  return {
    meta: event.metaKey,
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    codes,
  };
}
