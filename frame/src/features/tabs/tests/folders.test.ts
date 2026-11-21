import { describe, expect, it } from "vitest";
import type { SidebarNodeView, TabView } from "$shared/ipc/bindings";
import {
  collapsedSidebarUnits,
  folderContext,
  sidebarDisplayUnits,
  sidebarTree,
} from "../lib/sidebar-model";
import { groupable } from "../lib/folder-group";
import { spineLayers } from "../lib/folder-look";

function tab(id: string, extra: Partial<TabView> = {}): TabView {
  return {
    id,
    projection_revision: id.padStart(32, "0"),
    title: `Tab ${id}`,
    url: `https://${id}.example/`,
    loading: false,
    can_go_back: false,
    can_go_forward: false,
    icon: null,
    asleep: false,
    ...extra,
  };
}

const node = (id: string, parent_id: string | null = null): SidebarNodeView => ({
  id,
  parent_id,
  section: "today",
  kind: { type: "tab", tab_id: id },
});

const folder = (
  id: string,
  parent_id: string | null = null,
  color: "sky" | null = null,
): SidebarNodeView => ({
  id,
  parent_id,
  section: "today",
  kind: { type: "folder", name: id, color, collapsed: false },
});

describe("folder context", () => {
  it("sums every tab under a folder and draws each folder's spine", () => {
    const nodes = [
      folder("F", null, "sky"),
      node("a", "F"),
      folder("G", "F"),
      node("b", "G"),
      node("c", "G"),
      node("d", "F"),
      node("e"),
    ];
    const tabs = [
      tab("a", { asleep: true }),
      tab("b", { audio: { muted: false, can_mute: true } }),
      tab("c", { loading: true, url: null }),
      tab("d"),
      tab("e"),
    ];
    const units = sidebarDisplayUnits("today", sidebarTree(nodes, tabs).today, null);
    const { summaries, spines } = folderContext(units);

    const outer = summaries.get("folder:F")!;
    expect(outer.count).toBe(4);
    expect(outer.faces.map((face) => face.id)).toEqual(["a", "b", "d"]);
    expect(outer.audible).toBe(true);
    expect(outer.loading).toBe(true);
    expect(outer.awake).toBe(3);
    expect(outer.levels).toBe(2);
    expect(summaries.get("folder:G")).toMatchObject({ count: 2, levels: 1, audible: true });

    expect(spines.get("tab:a")).toEqual(["sky"]);
    expect(spines.get("tab:b")).toEqual(["sky", null]);
    expect(spines.get("tab:d")).toEqual(["sky"]);
    expect(spines.has("tab:e")).toBe(false);
    expect(spines.has("folder:F")).toBe(false);
  });

  it("draws one hairline per folder, outermost first", () => {
    const layers = spineLayers(["sky", null]).split("no-repeat, ");
    expect(layers).toHaveLength(2);
    expect(layers[0]).toContain("--color-soft-sky");
    expect(layers[1]).toContain("--color-border");
  });
});

describe("grouping", () => {
  it("files web pages of the space and leaves Essentials and browser pages out", () => {
    const nodes: SidebarNodeView[] = [
      { ...node("kept"), section: "favorites" },
      node("a"),
      node("settings"),
    ];
    const tabs = [tab("kept"), tab("a"), tab("settings", { content: "settings" })];
    expect(groupable(["kept", "a", "settings", "missing", null], tabs, nodes)).toEqual(["a"]);
  });
});

describe("folding", () => {
  it("hides a folded folder's rows but the tab in front and the folders on its way", () => {
    const nodes = [
      folder("F"),
      node("a", "F"),
      folder("G", "F"),
      node("b", "G"),
      node("c", "G"),
      node("d", "F"),
      node("e"),
    ];
    const tabs = ["a", "b", "c", "d", "e"].map((id) => tab(id));
    const units = sidebarDisplayUnits("today", sidebarTree(nodes, tabs).today, null);
    const keys = (set: Set<string>) => [...set].sort();
    expect(keys(collapsedSidebarUnits(units, new Set(["folder:F"]), "e"))).toEqual(
      ["folder:G", "tab:a", "tab:b", "tab:c", "tab:d"].sort(),
    );
    expect(keys(collapsedSidebarUnits(units, new Set(["folder:F"]), "c"))).toEqual(
      ["tab:a", "tab:b", "tab:d"].sort(),
    );
    expect(keys(collapsedSidebarUnits(units, new Set(["folder:F", "folder:G"]), "c"))).toEqual(
      ["tab:a", "tab:b", "tab:d"].sort(),
    );
    expect(keys(collapsedSidebarUnits(units, new Set(["folder:G"]), "a"))).toEqual(
      ["tab:b", "tab:c"].sort(),
    );
  });
});
