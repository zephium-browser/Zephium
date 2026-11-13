import { expect, test, vi } from "vitest";
import { MARKS } from "../lib/editor/marks";
import { BLOCKS } from "../lib/editor/blocks";
import { shortcutHint, deleteShortcut } from "../lib/platform";

const platform = vi.hoisted(() => ({ mac: false }));
vi.mock("../lib/editor/behaviour", () => ({ toggleChecklist: vi.fn() }));
vi.mock("@hugeicons/core-free-icons", () => ({ CheckListIcon: [], TextIcon: [] }));
vi.mock("$shared/platform", () => ({
  get IS_MAC() {
    return platform.mac;
  },
  IS_WINDOWS: true,
}));

test("editor hints name Windows control keys and macOS command keys", () => {
  expect(MARKS.map((mark) => mark.hint)).toEqual(["Ctrl+B", "Ctrl+I", "Ctrl+Shift+X", "Ctrl+E"]);
  expect(BLOCKS.find((block) => block.id === "title")?.hint).toBe("Ctrl+Alt+1");
  platform.mac = true;
  expect(shortcutHint("CmdOrCtrl+B")).toBe("⌘B");
  expect(shortcutHint("CmdOrCtrl+Alt+1")).toBe("⌥⌘1");
});

test("note deletion uses the platform modifier", () => {
  for (const key of ["Backspace", "Delete"]) {
    platform.mac = false;
    expect(deleteShortcut({ key, ctrlKey: true, metaKey: false })).toBe(true);
    expect(deleteShortcut({ key, ctrlKey: false, metaKey: true })).toBe(false);
    platform.mac = true;
    expect(deleteShortcut({ key, ctrlKey: false, metaKey: true })).toBe(true);
    expect(deleteShortcut({ key, ctrlKey: true, metaKey: false })).toBe(false);
  }
});
