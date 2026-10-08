import { describe, expect, it } from "vitest";
import type { ItemsState, TabView } from "$shared/ipc/bindings";
import { initialItemsState, TabProjectionModel, ZERO_PROJECTION_REVISION } from "../tabs-model";

function revision(value: number): string {
  return value.toString(16).padStart(32, "0");
}

function tab(id: string, value: number, overrides: Partial<TabView> = {}): TabView {
  return {
    id,
    projection_revision: revision(value),
    title: `Tab ${id}`,
    url: `https://${id}.example/`,
    loading: false,
    can_go_back: false,
    can_go_forward: false,
    icon: null,
    ...overrides,
  };
}

function snapshot(value: number, tabs: TabView[], active: string | null): ItemsState {
  return {
    projection_revision: revision(value),
    profile: {
      id: "profile-a",
      name: "Personal",
      kind: "default",
    },
    spaces: [{ id: "space-a", name: "Main" }],
    active_space_id: "space-a",
    nodes: [],
    tabs,
    active,
    split_group: null,
  };
}

describe("tab projection admission", () => {
  it("starts from the exact zero revision", () => {
    const state = initialItemsState();
    expect(state.projection_revision).toBe(ZERO_PROJECTION_REVISION);
    expect(state.profile).toBeNull();
    expect(state.spaces).toEqual([]);
    expect(state.active_space_id).toBeNull();
    expect(state.nodes).toEqual([]);
    expect(state.tabs).toEqual([]);
    expect(state.active).toBeNull();
    expect(state.split_group).toBeNull();
  });

  it("keeps the object of a tab whose revision did not change", () => {
    const model = new TabProjectionModel();
    model.applySnapshot(snapshot(3, [tab("a", 1), tab("b", 2)], "a"));
    const [a, b] = model.value.tabs;
    model.applySnapshot(snapshot(4, [tab("a", 1), tab("b", 4, { title: "Changed" })], "b"));
    expect(model.value.tabs[0]).toBe(a);
    expect(model.value.tabs[1]).not.toBe(b);
    expect(model.value.tabs[1]?.title).toBe("Changed");
    expect(model.value.active).toBe("b");
  });

  it("accepts only strictly newer full snapshots", () => {
    const model = new TabProjectionModel();
    const current = snapshot(3, [tab("a", 1)], "a");

    expect(model.applySnapshot(current)).toBe(true);
    expect(model.value).toStrictEqual(current);
    expect(model.applySnapshot({ ...current, active: null })).toBe(false);
    expect(model.applySnapshot(snapshot(2, [tab("b", 2)], "b"))).toBe(false);
    expect(model.value).toStrictEqual(current);
  });

  it("patches one row while preserving unrelated tab identity", () => {
    const first = tab("a", 1);
    const second = tab("b", 2);
    const initial = {
      ...snapshot(3, [first, second], "a"),
      nodes: [
        {
          id: "a",
          parent_id: null,
          section: "favorites" as const,
          kind: { type: "tab" as const, tab_id: "a" },
        },
        {
          id: "b",
          parent_id: null,
          section: "today" as const,
          kind: { type: "tab" as const, tab_id: "b" },
        },
      ],
    };
    const model = new TabProjectionModel(initial);
    const changed = tab("a", 4, { title: "Changed" });

    expect(model.applyTab(changed)).toBe(true);
    expect(model.value.tabs[0]).toBe(changed);
    expect(model.value.tabs[1]).toBe(second);
    expect(model.value.profile).toBe(initial.profile);
    expect(model.value.spaces).toBe(initial.spaces);
    expect(model.value.nodes).toBe(initial.nodes);
    expect(model.applyTab({ ...changed, title: "Duplicate" })).toBe(false);
    expect(model.applyTab(tab("a", 2, { title: "Stale" }))).toBe(false);
    expect(model.value.tabs[0]).toBe(changed);
  });

  it("replaces profile, spaces, nodes, tabs, and focus as one structural revision", () => {
    const model = new TabProjectionModel(snapshot(3, [tab("a", 1)], "a"));
    const replacement: ItemsState = {
      ...snapshot(6, [tab("b", 5)], "b"),
      profile: { id: "profile-b", name: "Work", kind: "named" },
      spaces: [
        { id: "space-b", name: "Build" },
        { id: "space-c", name: "Research" },
      ],
      active_space_id: "space-b",
      nodes: [
        {
          id: "folder",
          parent_id: null,
          section: "pinned",
          kind: { type: "folder", name: "Project" },
        },
        {
          id: "b",
          parent_id: "folder",
          section: "pinned",
          kind: { type: "tab", tab_id: "b" },
        },
      ],
    };

    expect(model.applySnapshot(replacement)).toBe(true);
    expect(model.value).toStrictEqual(replacement);
    expect(
      model.applySnapshot({
        ...snapshot(5, [tab("a", 4)], "a"),
        profile: { id: "stale", name: "Stale", kind: "incognito" },
      }),
    ).toBe(false);
    expect(model.value).toStrictEqual(replacement);
  });

  it("lets an exact presentation update its tab and active id", () => {
    const group = { members: ["a", "b"] };
    const model = new TabProjectionModel({
      ...snapshot(3, [tab("a", 1), tab("b", 2)], "a"),
      split_group: group,
    });
    const presented = tab("b", 5, { title: "Committed", url: "https://committed.example/" });

    expect(model.applyPresentation({ tab: presented, active: "b" })).toBe(true);
    expect(model.value.active).toBe("b");
    expect(model.value.split_group).toBe(group);
    expect(model.value.tabs.find((candidate) => candidate.id === "b")).toBe(presented);
  });

  it("replaces split membership only through a newer structural snapshot", () => {
    const original = { members: ["a", "b"] };
    const replacement = { members: ["b", "c"] };
    const model = new TabProjectionModel({
      ...snapshot(4, [tab("a", 1), tab("b", 2), tab("c", 3)], "a"),
      split_group: original,
    });

    expect(
      model.applySnapshot({
        ...snapshot(6, [tab("a", 5), tab("b", 5), tab("c", 5)], "b"),
        split_group: replacement,
      }),
    ).toBe(true);
    expect(model.value.split_group).toBe(replacement);
    expect(
      model.applySnapshot({
        ...snapshot(5, [tab("a", 4), tab("b", 4), tab("c", 4)], "a"),
        split_group: original,
      }),
    ).toBe(false);
    expect(model.value.split_group).toBe(replacement);
  });

  it("rejects a presentation older than a previously applied delta", () => {
    const model = new TabProjectionModel(snapshot(3, [tab("a", 1)], "a"));
    const newest = tab("a", 7, { title: "Newest" });

    expect(model.applyTab(newest)).toBe(true);
    expect(
      model.applyPresentation({
        tab: tab("a", 6, { title: "Stale presentation" }),
        active: null,
      }),
    ).toBe(false);
    expect(model.value.active).toBe("a");
    expect(model.value.tabs[0]).toBe(newest);
  });

  it("rejects an older snapshot after a newer tab delta advances the global floor", () => {
    const model = new TabProjectionModel(snapshot(3, [tab("a", 1)], "a"));
    expect(model.applyTab(tab("a", 8))).toBe(true);
    expect(model.applySnapshot(snapshot(7, [tab("a", 6)], null))).toBe(false);
    expect(model.value.active).toBe("a");
    expect(model.value.tabs[0]?.projection_revision).toBe(revision(8));
  });
});
