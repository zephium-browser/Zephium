import { describe, expect, it } from "vitest";
import type { SidebarNodeView, SplitGroupView, TabView } from "$shared/ipc/bindings";
import {
  homeDisplayUnits,
  homeRailTabs,
  sidebarDisplayUnits,
  sidebarTree,
} from "../lib/sidebar-model";

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
      { id: "f", parent_id: null, section: "pinned", kind: { type: "folder", name: "Work" } },
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

describe("the left side while Home is on", () => {
  const found = (id: string, search: string, results = false): TabView => ({
    ...tab(id),
    search: { id: search, text: `words ${search}`, results },
  });
  const all = [
    { ...tab("home"), url: null, home: true },
    tab("plain"),
    found("a1", "q-a"),
    found("b1", "q-b"),
    found("ar", "q-a", true),
    found("a2", "q-a"),
    tab("later"),
  ];
  const units = (active: string | null) => {
    const tree = sidebarTree(
      all.map((one) => tabNode(one.id)),
      all,
    );
    return homeDisplayUnits(sidebarDisplayUnits("today", tree.today, null), active);
  };
  const keys = (list: ReturnType<typeof units>) => list.map((unit) => unit.key);

  it("leaves Home out and gathers a search's pages under one head where its first page stood", () => {
    expect(keys(units("plain"))).toEqual([
      "tab:plain",
      "search:q-a",
      "tab:a1",
      "tab:a2",
      "search:q-b",
      "tab:b1",
      "tab:later",
    ]);
    expect(units("plain")[1]).toMatchObject({ kind: "search", text: "words q-a", depth: 0 });
  });

  it("lists a results page only while it is in front", () => {
    expect(keys(units("ar"))).toEqual([
      "tab:plain",
      "search:q-a",
      "tab:a1",
      "tab:ar",
      "tab:a2",
      "search:q-b",
      "tab:b1",
      "tab:later",
    ]);
  });

  it("keeps a page inside a folder where it is, ungathered", () => {
    const nodes: SidebarNodeView[] = [
      { id: "f", parent_id: null, section: "today", kind: { type: "folder", name: "Kept" } },
      tabNode("a1", "today", "f"),
      tabNode("a2"),
    ];
    const tree = sidebarTree(
      nodes,
      all.filter((one) => one.id === "a1" || one.id === "a2"),
    );
    const shown = homeDisplayUnits(sidebarDisplayUnits("today", tree.today, null), null);
    expect(shown.map((unit) => unit.key)).toContain("tab:a1");
    expect(shown.filter((unit) => unit.kind === "search").map((unit) => unit.key)).toEqual([
      "search:q-a",
    ]);
  });

  it("puts Home first in the rail and the groups' pages together", () => {
    expect(homeRailTabs(all, null).map((one) => one.id)).toEqual([
      "home",
      "plain",
      "a1",
      "a2",
      "b1",
      "later",
    ]);
    expect(homeRailTabs(all, "ar").map((one) => one.id)).toContain("ar");
  });
});
