import { describe, expect, it } from "vitest";
import { heldFrom, heldParts, NOTHING_HELD } from "../lib/keys";

// Every event carries all four modifier flags, as a real one does.
const MODIFIERS = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
const press = (init: KeyboardEventInit, previous = NOTHING_HELD) =>
  heldFrom({ type: "keydown", ...MODIFIERS, ...init } as KeyboardEvent, previous);

describe("heldParts", () => {
  it("lights the platform's own modifier for CmdOrCtrl", () => {
    const held = press({ code: "MetaLeft", metaKey: true });
    expect(heldParts("CmdOrCtrl+Shift+Space", held, true)).toEqual([false, true, false]);
    expect(heldParts("CmdOrCtrl+Shift+Space", held, false)).toEqual([false, false, false]);
  });

  it("follows each key down and back up", () => {
    let held = press({ code: "ShiftLeft", shiftKey: true, ctrlKey: true });
    held = press({ code: "Space", shiftKey: true, ctrlKey: true }, held);
    expect(heldParts("CmdOrCtrl+Shift+Space", held, false)).toEqual([true, true, true]);
    held = heldFrom(
      {
        type: "keyup",
        ...MODIFIERS,
        code: "Space",
        shiftKey: true,
        ctrlKey: true,
      } as KeyboardEvent,
      held,
    );
    expect(heldParts("CmdOrCtrl+Shift+Space", held, false)).toEqual([true, true, false]);
  });

  it("matches physical letter, digit and arrow codes to their displayed keycaps", () => {
    for (const [key, code] of [
      ["L", "KeyL"],
      ["1", "Digit1"],
      ["Up", "ArrowUp"],
    ]) {
      const held = press({ code, metaKey: true });
      expect(heldParts(`CmdOrCtrl+${key}`, held, true)).toEqual([true, true]);
    }
  });

  it("lights modifiers in display order for remapped shortcuts", () => {
    const held = press({ code: "KeyL", metaKey: true });
    expect(heldParts("CmdOrCtrl+Option+L", held, true)).toEqual([false, true, true]);
    expect(heldParts("Shift+Ctrl+L", held, false)).toEqual([false, false, true]);
  });

  it("reads Alt and Option as one key", () => {
    const held = press({ code: "AltLeft", altKey: true });
    expect(heldParts("Option+KeyK", held, true)).toEqual([true, false]);
    expect(heldParts("Alt+KeyK", held, false)).toEqual([true, false]);
  });
});
