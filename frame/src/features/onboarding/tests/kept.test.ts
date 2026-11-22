import { describe, expect, it } from "vitest";
import type { SidebarNodeView, TabView } from "$shared/ipc/bindings";
import { keptSites } from "../lib/kept";

const tab = (id: string, url: string | null): TabView => ({
  id,
  projection_revision: "0",
  title: id,
  url,
  loading: false,
  can_go_back: false,
  can_go_forward: false,
  icon: null,
});
const node = (id: string, section: SidebarNodeView["section"]): SidebarNodeView => ({
  id,
  parent_id: null,
  section,
  kind: { type: "tab", tab_id: id },
});
const URLS = { slack: "https://app.slack.com/client", github: "https://github.com/" };

describe("keptSites", () => {
  it("finds a catalog site only in Essentials and only at its exact address", () => {
    const kept = keptSites(
      [node("a", "favorites"), node("b", "today"), node("c", "favorites")],
      [
        tab("a", "https://app.slack.com/client"),
        tab("b", "https://github.com/"),
        tab("c", "https://github.com/enigma"),
      ],
      URLS,
    );
    expect([...kept]).toEqual([["slack", "a"]]);
  });

  it("names the first tab when a site is somehow kept twice", () => {
    const kept = keptSites(
      [node("a", "favorites"), node("b", "favorites")],
      [tab("a", "https://github.com/"), tab("b", "https://github.com/")],
      URLS,
    );
    expect(kept.get("github")).toBe("a");
  });

  it("ignores folders and tabs with no page", () => {
    const folder: SidebarNodeView = {
      id: "f",
      parent_id: null,
      section: "favorites",
      kind: { type: "folder", name: "Work", color: null, collapsed: false },
    };
    expect(keptSites([folder, node("n", "favorites")], [tab("n", null)], URLS).size).toBe(0);
  });
});
