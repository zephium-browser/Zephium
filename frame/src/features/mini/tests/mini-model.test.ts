import { describe, expect, it } from "vitest";
import {
  acceptMiniTab,
  acceptMiniView,
  hostOf,
  miniShortcut,
  windowOfLabel,
} from "../lib/mini-model";
import type { MiniView, TabView } from "$shared/ipc/bindings";

const page = (id: string, revision: number): TabView => ({
  id,
  projection_revision: revision.toString(16).padStart(32, "0"),
  title: "Example",
  url: "https://example.com/a",
  loading: false,
  can_go_back: false,
  can_go_forward: false,
  icon: null,
});

const view = (window: number, tab: TabView | null): MiniView => ({
  window,
  tab,
  spaces: [],
  space: null,
  pinned: false,
});

describe("mini window toolbar", () => {
  it("names its window from canonical labels only", () => {
    expect(windowOfLabel("mini-2")).toBe(2);
    for (const label of ["mini-", "mini-0", "mini-02", "mini-2a", "main", "panel"])
      expect(windowOfLabel(label)).toBeNull();
    expect(windowOfLabel("mini-99999999999999999999")).toBeNull();
  });

  it("takes views for its own window and never an older revision of its page", () => {
    expect(acceptMiniView(2, null, view(3, page("a", 1)))).toBe(false);
    expect(acceptMiniView(2, null, view(2, page("a", 1)))).toBe(true);
    expect(acceptMiniView(2, page("a", 1), view(2, null))).toBe(true);
    expect(acceptMiniTab(page("a", 2), page("a", 1))).toBe(false);
    expect(acceptMiniTab(page("a", 2), page("a", 2))).toBe(true);
    expect(acceptMiniTab(page("a", 2), page("b", 1))).toBe(true);
    expect(acceptMiniTab(null, { ...page("a", 1), projection_revision: "bad" })).toBe(false);
  });

  it("shows the host native checks, and nothing for a URL it cannot parse", () => {
    expect(hostOf("https://example.com:8443/path")).toBe("example.com:8443");
    expect(hostOf("not a url")).toBe("");
    expect(hostOf(null)).toBe("");
  });
});

describe("miniShortcut", () => {
  const key = (
    k: string,
    mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>>,
  ) => ({
    key: k,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...mods,
  });

  it("answers the platform's command modifier only", () => {
    expect(miniShortcut(key("o", { metaKey: true }), true)).toBe("open_in_main");
    expect(miniShortcut(key("O", { ctrlKey: true }), false)).toBe("open_in_main");
    expect(miniShortcut(key("l", { metaKey: true }), true)).toBe("edit_address");
    expect(miniShortcut(key("w", { ctrlKey: true }), false)).toBe("close");
    expect(miniShortcut(key("r", { metaKey: true }), true)).toBe("reload");
    expect(miniShortcut(key("[", { metaKey: true }), true)).toBe("back");
    expect(miniShortcut(key("]", { ctrlKey: true }), false)).toBe("forward");
  });

  it("ignores the other platform's modifier, extra modifiers and other keys", () => {
    expect(miniShortcut(key("o", { ctrlKey: true }), true)).toBeNull();
    expect(miniShortcut(key("o", { metaKey: true }), false)).toBeNull();
    expect(miniShortcut(key("o", { metaKey: true, ctrlKey: true }), true)).toBeNull();
    expect(miniShortcut(key("o", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(miniShortcut(key("o", { ctrlKey: true, altKey: true }), false)).toBeNull();
    expect(miniShortcut(key("o", {}), true)).toBeNull();
    expect(miniShortcut(key("t", { metaKey: true }), true)).toBeNull();
    expect(miniShortcut(key("", { ctrlKey: true }), false)).toBeNull();
  });
});
