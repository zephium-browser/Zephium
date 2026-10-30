import { beforeEach, describe, expect, it, vi } from "vitest";
import type { KeymapEntry } from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({
  entries: vi.fn(),
  bind: vi.fn(),
  reset: vi.fn(),
  stop: vi.fn(),
  listener: null as null | (() => void),
  listen: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    keymapEntries: native.entries,
    keymapBind: native.bind,
    keymapReset: native.reset,
  });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: { keymapChanged: { listen: native.listen } },
}));

const entry = (id: string, accelerator: string | null, group = "file"): KeymapEntry => ({
  id,
  title: id,
  group: group as KeymapEntry["group"],
  accelerator,
  default_accelerator: accelerator,
  customizable: group !== "work" && group !== "global",
  customized: false,
});

const press = (init: Partial<KeyboardEvent>) =>
  ({ isComposing: false, repeat: false, ...init }) as KeyboardEvent;

beforeEach(() => {
  vi.resetModules();
  vi.resetAllMocks();
  native.listener = null;
  native.listen.mockImplementation((listener: () => void) => {
    native.listener = listener;
    return Promise.resolve(native.stop);
  });
  native.entries.mockResolvedValue([
    entry("tab.new", "Ctrl+T"),
    entry("tab.select.1", "Ctrl+1", "keys"),
    entry("work.pane.close", "Escape", "work"),
    entry("launcher.toggle", "Ctrl+Shift+Space", "global"),
  ]);
});

describe("keymap", () => {
  it.each([false, true])("accepts zoom plus and keypad aliases (mac: %s)", async (mac) => {
    native.entries.mockResolvedValue([
      entry("zoom.in", "CmdOrCtrl+=", "view"),
      entry("zoom.out", "CmdOrCtrl+-", "view"),
    ]);
    const keymap = await import("../keymap.svelte");
    await keymap.init();
    const modifier = mac ? "Cmd" : "Ctrl";
    expect(
      keymap.commandFor(
        press({ code: "Equal", key: "+", shiftKey: true }),
        mac,
        `${modifier}+Shift+Equal`,
      ),
    ).toBe("zoom.in");
    expect(
      keymap.commandFor(press({ code: "NumpadAdd", key: "+" }), mac, `${modifier}+NumpadAdd`),
    ).toBe("zoom.in");
    expect(
      keymap.commandFor(
        press({ code: "NumpadSubtract", key: "-" }),
        mac,
        `${modifier}+NumpadSubtract`,
      ),
    ).toBe("zoom.out");
    expect(
      keymap.commandFor(press({ code: "NumpadAdd", key: "+" }), mac, "Alt+NumpadAdd"),
    ).toBeNull();
  });

  it("preserves exact bindings and does not revive rebound or disabled zoom", async () => {
    native.entries.mockResolvedValue([
      entry("zoom.in", "Ctrl+=", "view"),
      entry("tab.new", "Ctrl+Shift+=", "file"),
      entry("zoom.out", null, "view"),
    ]);
    const keymap = await import("../keymap.svelte");
    await keymap.init();
    expect(
      keymap.commandFor(
        press({ code: "Equal", key: "+", shiftKey: true }),
        false,
        "Ctrl+Shift+Equal",
      ),
    ).toBe("tab.new");
    expect(
      keymap.commandFor(press({ code: "NumpadSubtract", key: "-" }), false, "Ctrl+NumpadSubtract"),
    ).toBeNull();
    native.entries.mockResolvedValue([entry("zoom.in", "Ctrl+J", "view")]);
    native.listener?.();
    await vi.waitFor(() => expect(keymap.all()[0]?.accelerator).toBe("Ctrl+J"));
    expect(
      keymap.commandFor(press({ code: "NumpadAdd", key: "+" }), false, "Ctrl+NumpadAdd"),
    ).toBeNull();
    native.entries.mockResolvedValue([entry("zoom.in", "Ctrl+Alt+=", "view")]);
    native.listener?.();
    await vi.waitFor(() => expect(keymap.all()[0]?.accelerator).toBe("Ctrl+Alt+="));
    expect(
      keymap.commandFor(
        press({ code: "NumpadAdd", key: "+", altKey: true }),
        false,
        "Ctrl+Alt+NumpadAdd",
      ),
    ).toBeNull();
  });

  it("matches chrome key presses against the resolved keymap only", async () => {
    const keymap = await import("../keymap.svelte");
    await keymap.init();
    expect(keymap.commandFor(press({}), false, "Ctrl+KeyT")).toBe("tab.new");
    expect(keymap.commandFor(press({}), false, "Ctrl+Digit1")).toBe("tab.select.1");
    expect(keymap.commandFor(press({}), false, "Escape")).toBeNull();
    expect(keymap.commandFor(press({}), false, "Ctrl+Shift+Space")).toBeNull();
    expect(keymap.commandFor(press({ repeat: true }), false, "Ctrl+KeyT")).toBeNull();
    expect(keymap.commandFor(press({ isComposing: true }), false, "Ctrl+KeyT")).toBeNull();
  });

  it("reads the keymap again when native says it changed", async () => {
    const keymap = await import("../keymap.svelte");
    await keymap.init();
    native.entries.mockResolvedValue([entry("tab.new", "Ctrl+N")]);
    native.listener?.();
    await vi.waitFor(() => expect(keymap.all()[0]?.accelerator).toBe("Ctrl+N"));
    expect(keymap.commandFor(press({}), false, "Ctrl+KeyT")).toBeNull();
  });

  it("reports a conflict without refreshing, and refreshes after a change", async () => {
    const keymap = await import("../keymap.svelte");
    await keymap.init();
    native.entries.mockClear();
    native.bind.mockResolvedValueOnce({ kind: "conflict", command: "tab.close" });
    expect(await keymap.bind("tab.new", "Ctrl+W")).toEqual({
      kind: "conflict",
      command: "tab.close",
    });
    expect(native.entries).not.toHaveBeenCalled();
    native.bind.mockResolvedValueOnce({ kind: "applied" });
    await keymap.bind("tab.new", "Ctrl+N");
    expect(native.entries).toHaveBeenCalledOnce();
    expect(native.bind).toHaveBeenLastCalledWith("tab.new", "Ctrl+N", false);
    native.bind.mockRejectedValueOnce(new Error("gone"));
    expect(await keymap.bind("tab.new", "Ctrl+N")).toEqual({ kind: "unavailable" });
  });

  it("stops listening when disposed", async () => {
    const keymap = await import("../keymap.svelte");
    await keymap.init();
    keymap.dispose();
    expect(native.stop).toHaveBeenCalledOnce();
    expect(keymap.all()).toEqual([]);
  });
});
