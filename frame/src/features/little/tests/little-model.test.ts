import { describe, expect, it } from "vitest";
import { acceptLittleTab, acceptLittleView, hostOf, windowOfLabel } from "../lib/little-model";
import type { LittleView, TabView } from "$shared/ipc/bindings";

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

const view = (window: number, tab: TabView | null): LittleView => ({
  window,
  tab,
  spaces: [],
  space: null,
});

describe("Little window toolbar", () => {
  it("names its window from canonical labels only", () => {
    expect(windowOfLabel("little-2")).toBe(2);
    for (const label of ["little-", "little-0", "little-02", "little-2a", "main", "panel"])
      expect(windowOfLabel(label)).toBeNull();
    expect(windowOfLabel("little-99999999999999999999")).toBeNull();
  });

  it("takes views for its own window and never an older revision of its page", () => {
    expect(acceptLittleView(2, null, view(3, page("a", 1)))).toBe(false);
    expect(acceptLittleView(2, null, view(2, page("a", 1)))).toBe(true);
    expect(acceptLittleView(2, page("a", 1), view(2, null))).toBe(true);
    expect(acceptLittleTab(page("a", 2), page("a", 1))).toBe(false);
    expect(acceptLittleTab(page("a", 2), page("a", 2))).toBe(true);
    expect(acceptLittleTab(page("a", 2), page("b", 1))).toBe(true);
    expect(acceptLittleTab(null, { ...page("a", 1), projection_revision: "bad" })).toBe(false);
  });

  it("shows the host native checks, and nothing for a URL it cannot parse", () => {
    expect(hostOf("https://example.com:8443/path")).toBe("example.com:8443");
    expect(hostOf("not a url")).toBe("");
    expect(hostOf(null)).toBe("");
  });
});
