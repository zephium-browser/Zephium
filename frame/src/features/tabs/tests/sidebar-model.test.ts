import { describe, expect, it } from "vitest";
import type { SidebarNodeView, SplitGroupView, TabView } from "$shared/ipc/bindings";
import { sidebarDisplayUnits, sidebarTree } from "../lib/sidebar-model";

function tab(id: string): TabView {
  return {
    id,
    projection_revision: id.padStart(32, "0"),
    title: `Tab ${id}`,
    url: `https://${id}.example/`,
    loading: false,
    can_go_back: false,
    can_go_forward: false,
    icon: null,
  };
}

function tabNode(
  id: string,
  section: SidebarNodeView["section"] = "today",
  parent_id: string | null = null,
): SidebarNodeView {
  return { id, parent_id, section, kind: { type: "tab", tab_id: id } };
}

describe("authoritative sidebar model", () => {
  it("preserves native section, pre-order, and folder depth", () => {
    const nodes: SidebarNodeView[] = [
      {
        id: "f",
        parent_id: null,
        section: "pinned",
        kind: { type: "folder", name: "Work", color: null, collapsed: false },
      },
      tabNode("a", "pinned", "f"),
      tabNode("b"),
    ];
    const tree = sidebarTree(nodes, [tab("a"), tab("b")]);

    expect(tree.pinned.map((entry) => [entry.kind, entry.depth])).toEqual([
      ["folder", 0],
      ["tab", 1],
    ]);
    expect(tree.today.map((entry) => entry.key)).toEqual(["tab:b"]);
  });

  it("rejects malformed links while retaining one fallback row per authoritative tab", () => {
    const nodes: SidebarNodeView[] = [
      tabNode("a", "pinned", "missing"),
      tabNode("a"),
      tabNode("a"),
    ];
    const tree = sidebarTree(nodes, [tab("a"), tab("b")]);
    const ids = Object.values(tree)
      .flat()
      .filter((entry) => entry.kind === "tab")
      .map((entry) => (entry.kind === "tab" ? entry.tab.id : ""));

    expect(ids.sort()).toEqual(["a", "b"]);
  });

  it("groups a valid same-section split once in native member order", () => {
    const tree = sidebarTree(
      [tabNode("a"), tabNode("b"), tabNode("c")],
      [tab("a"), tab("b"), tab("c")],
    );
    const split: SplitGroupView = { members: ["b", "a"] };
    const units = sidebarDisplayUnits("today", tree.today, split);

    expect(units.map((unit) => unit.kind)).toEqual(["split", "tab"]);
    const group = units[0];
    expect(group?.kind === "split" ? group.tabs.map((entry) => entry.tab.id) : []).toEqual([
      "b",
      "a",
    ]);
  });

  it("fails a cross-section or unknown split back to independent rows", () => {
    const tree = sidebarTree([tabNode("a", "pinned"), tabNode("b")], [tab("a"), tab("b")]);
    expect(
      sidebarDisplayUnits("today", tree.today, { members: ["a", "b"] }).map((unit) => unit.kind),
    ).toEqual(["tab"]);
  });
});
