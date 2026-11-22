import type { SidebarNodeView, TabView } from "$shared/ipc/bindings";

/** The tabs a folder can take from `ids`: web pages among the space's own
 *  tabs, not its Essentials. Native checks each again. */
export function groupable(
  ids: readonly (string | null)[],
  tabs: readonly TabView[],
  nodes: readonly SidebarNodeView[],
): string[] {
  const kept = new Set(
    nodes.flatMap((node) =>
      node.section === "favorites" && node.kind.type === "tab" ? [node.kind.tab_id] : [],
    ),
  );
  const byId = new Map(tabs.map((tab) => [tab.id, tab] as const));
  return ids.filter((id): id is string => {
    const tab = id === null ? undefined : byId.get(id);
    return tab !== undefined && (tab.content ?? "web") === "web" && !kept.has(tab.id);
  });
}
