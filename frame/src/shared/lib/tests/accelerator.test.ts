import { describe, expect, it } from "vitest";
import { acceleratorFrom, acceleratorKeys, heldModifiers, sameAccelerator } from "../accelerator";

describe("accelerators", () => {
  it("translates every modifier and names keys by their label", () => {
    expect(acceleratorKeys("CmdOrCtrl+Shift+T", true)).toEqual(["⇧", "⌘", "T"]);
    expect(acceleratorKeys("Cmd+Option+N", true)).toEqual(["⌥", "⌘", "N"]);
    expect(acceleratorKeys("Ctrl+Shift+Enter", false)).toEqual(["Ctrl", "Shift", "↵"]);
    expect(acceleratorKeys("Ctrl++", false)).toEqual(["Ctrl", "="]);
    expect(acceleratorKeys("Ctrl+Shift+Tab", true)).toEqual(["⌃", "⇧", "Tab"]);
    expect(acceleratorKeys("Ctrl+Alt+KeyK", true)).toEqual(["⌃", "⌥", "K"]);
    expect(acceleratorKeys("CmdOrCtrl+Shift+Space", false)).toEqual(["Ctrl", "Shift", "Space"]);
    expect(acceleratorKeys("", true)).toEqual([]);
  });

  it("spells a key press by physical key, and nothing for modifiers alone", () => {
    const press = (init: Partial<KeyboardEvent>) =>
      ({
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        metaKey: false,
        ...init,
      }) as KeyboardEvent;
    expect(acceleratorFrom(press({ code: "Space", altKey: true, metaKey: true }), true)).toBe(
      "Alt+Cmd+Space",
    );
    expect(acceleratorFrom(press({ code: "KeyK", ctrlKey: true, shiftKey: true }), false)).toBe(
      "Ctrl+Shift+KeyK",
    );
    expect(acceleratorFrom(press({ code: "MetaLeft", metaKey: true }), true)).toBeNull();
    expect(acceleratorFrom(press({ code: "KeyK", ctrlKey: true, keyCode: 229 }), false)).toBeNull();
    expect(
      acceleratorFrom(press({ code: "KeyK", metaKey: true, isComposing: true }), true),
    ).toBeNull();
    expect(heldModifiers(press({ metaKey: true, shiftKey: true }), true)).toEqual(["⇧", "⌘"]);
    expect(heldModifiers(press({ ctrlKey: true, altKey: true }), false)).toEqual(["Ctrl", "Alt"]);
    expect(heldModifiers(press({}), false)).toEqual([]);
  });

  it("compares spellings by the keys they name", () => {
    expect(sameAccelerator("Ctrl+Shift+KeyK", "CmdOrCtrl+Shift+K", false)).toBe(true);
    expect(sameAccelerator("Shift+Ctrl+Digit1", "Ctrl+Shift+1", false)).toBe(true);
    expect(sameAccelerator("Ctrl+BracketLeft", "Ctrl+[", false)).toBe(true);
    expect(sameAccelerator("CmdOrCtrl+T", "Cmd+T", true)).toBe(true);
    expect(sameAccelerator("CmdOrCtrl+T", "Ctrl+T", true)).toBe(false);
    expect(sameAccelerator("Ctrl+T", "Ctrl+Shift+T", false)).toBe(false);
    expect(sameAccelerator("", "Ctrl+T", false)).toBe(false);
  });
});
