import { describe, expect, it } from "vitest";
import type { KeymapEntry } from "$shared/ipc/bindings";
import { commandTitle, keymapSections } from "../lib/keymap-model";

const entry = (id: string, group: KeymapEntry["group"], customizable = true): KeymapEntry => ({
  id,
  title: `native ${id}`,
  group,
  accelerator: null,
  default_accelerator: null,
  customizable,
  customized: false,
});

describe("keymap model", () => {
  it("titles commands in the interface language, numbering tab selection", () => {
    expect(commandTitle(entry("tab.new", "file"))).toBe("New tab");
    expect(commandTitle(entry("tab.select.3", "keys"))).toBe("Select tab 3");
    expect(commandTitle(entry("future.command", "file"))).toBe("native future.command");
  });

  it("lists rebindable commands by section and drops empty sections", () => {
    const sections = keymapSections([
      entry("browser.settings", "app"),
      entry("tab.new", "file"),
      entry("tab.select.1", "keys"),
      entry("tab.next", "window"),
      entry("tab.below", "window"),
      entry("space.next", "keys"),
      entry("tool.downloads", "window"),
      entry("launcher.toggle", "global", false),
      entry("work.pane.close", "work", false),
      entry("work.dictate", "keys"),
    ]);
    expect(sections.map((section) => section.id)).toEqual(["browser", "tabs", "spaces", "work"]);
    expect(sections[0]!.entries.map((e) => e.id)).toEqual([
      "browser.settings",
      "tab.new",
      "tool.downloads",
    ]);
    expect(sections[1]!.entries.map((e) => e.id)).toEqual([
      "tab.select.1",
      "tab.next",
      "tab.below",
    ]);
    expect(sections[2]!.entries.map((e) => e.id)).toEqual(["space.next"]);
    expect(commandTitle(sections[2]!.entries[0]!)).toBe("Next space");
    expect(sections[3]!.entries.map((e) => e.id)).toEqual(["work.dictate"]);
    expect(commandTitle(sections[3]!.entries[0]!)).toBe("Dictate in Work");
    expect(
      keymapSections(
        [entry("tab.new", "file"), entry("work.dictate", "keys")],
        (id) => id !== "work.dictate",
      ).map((section) => section.id),
    ).toEqual(["browser"]);
  });
  it("files the new browser keys with their commands and hides keys a platform lacks", () => {
    const keyed = (id: string, group: KeymapEntry["group"], accelerator: string | null) => ({
      ...entry(id, group),
      accelerator,
      default_accelerator: accelerator,
    });
    const sections = keymapSections([
      keyed("settings.clearData", "keys", "Ctrl+Shift+Delete"),
      keyed("nav.reload", "view", "Ctrl+R"),
      keyed("nav.reload.alt", "keys", "F5"),
      keyed("nav.hardReload", "view", "Ctrl+Shift+R"),
      keyed("window.fullscreen", "keys", "F11"),
      keyed("nav.back", "history", "Ctrl+["),
      keyed("nav.back.alt", "keys", "Alt+Left"),
      keyed("tab.next.alt", "keys", "Ctrl+PageDown"),
    ]);
    const ids = (id: string) => sections.find((s) => s.id === id)?.entries.map((e) => e.id);
    expect(ids("browser")).toEqual(["settings.clearData"]);
    expect(ids("view")).toEqual([
      "nav.reload",
      "nav.reload.alt",
      "nav.hardReload",
      "window.fullscreen",
    ]);
    expect(ids("history")).toEqual(["nav.back", "nav.back.alt"]);
    expect(ids("tabs")).toEqual(["tab.next.alt"]);
    expect(commandTitle(sections.find((s) => s.id === "view")!.entries[1]!)).toBe(
      "Reload page (alternate)",
    );

    const mac = keymapSections([
      entry("nav.reload.alt", "keys"),
      entry("window.fullscreen", "keys"),
    ]);
    expect(mac).toEqual([]);
  });
});
