import type {
  SidebarNodeView,
  SidebarSectionView,
  SplitGroupView,
  TabView,
} from "$shared/ipc/bindings";

export type SidebarFolderEntry = {
  kind: "folder";
  key: string;
  node: SidebarNodeView & { kind: { type: "folder"; name: string } };
  depth: number;
};

export type SidebarTabEntry = {
  kind: "tab";
  key: string;
  node: SidebarNodeView & { kind: { type: "tab"; tab_id: string } };
  tab: TabView;
  depth: number;
};

export type SidebarEntry = SidebarFolderEntry | SidebarTabEntry;

export type SidebarDisplayUnit =
  | SidebarFolderEntry
  | SidebarTabEntry
  | {
      kind: "split";
      key: string;
      tabs: SidebarTabEntry[];
      depth: number;
    };

export type SidebarTree = Record<SidebarSectionView, SidebarEntry[]>;

const EMPTY_TREE = (): SidebarTree => ({
  favorites: [],
  pinned: [],
  today: [],
});

/**
 * Admits Rust's bounded pre-order sidebar records into a renderable tree.
 *
 * Broken parent links and duplicate references are ignored. Any authoritative
 * TabView left without a valid node is appended to Today so the native
 * presentation barrier can still find exactly one row and fail independently
 * on its exact revision checks.
 */
// The last tree's tab entries, so a row whose tab, node and depth are all as
// they were keeps its entry object and the list leaves it alone.
let previousTabEntries = new Map<string, SidebarTabEntry>();

function tabEntry(
  node: SidebarTabEntry["node"],
  tab: TabView,
  depth: number,
  next: Map<string, SidebarTabEntry>,
): SidebarTabEntry {
  const key = `tab:${tab.id}`;
  const previous = previousTabEntries.get(key);
  const entry =
    previous !== undefined &&
    previous.tab === tab &&
    previous.depth === depth &&
    previous.node.id === node.id &&
    previous.node.parent_id === node.parent_id &&
    previous.node.section === node.section
      ? previous
      : { kind: "tab" as const, key, node, tab, depth };
  next.set(key, entry);
  return entry;
}

export function sidebarTree(
  nodes: readonly SidebarNodeView[],
  tabs: readonly TabView[],
): SidebarTree {
  const tree = EMPTY_TREE();
  const entries = new Map<string, SidebarTabEntry>();
  const tabsById = new Map(tabs.map((tab) => [tab.id, tab] as const));
  const admittedTabs = new Set<string>();
  const admittedNodes = new Map<
    string,
    { depth: number; section: SidebarSectionView; folder: boolean }
  >();

  for (const node of nodes) {
    if (admittedNodes.has(node.id)) continue;

    let depth = 0;
    if (node.parent_id !== null) {
      const parent = admittedNodes.get(node.parent_id);
      if (parent === undefined || !parent.folder || parent.section !== node.section) continue;
      depth = parent.depth + 1;
      if (depth > 64) continue;
    }

    if (node.kind.type === "folder") {
      const entry: SidebarFolderEntry = {
        kind: "folder",
        key: `folder:${node.id}`,
        node: node as SidebarFolderEntry["node"],
        depth,
      };
      admittedNodes.set(node.id, { depth, section: node.section, folder: true });
      tree[node.section].push(entry);
      continue;
    }

    const tab = tabsById.get(node.kind.tab_id);
    if (tab === undefined || admittedTabs.has(tab.id)) continue;
    const entry = tabEntry(node as SidebarTabEntry["node"], tab, depth, entries);
    admittedNodes.set(node.id, { depth, section: node.section, folder: false });
    admittedTabs.add(tab.id);
    tree[node.section].push(entry);
  }

  for (const tab of tabs) {
    if (admittedTabs.has(tab.id)) continue;
    const node: SidebarTabEntry["node"] = {
      id: tab.id,
      parent_id: null,
      section: "today",
      kind: { type: "tab", tab_id: tab.id },
    };
    tree.today.push(tabEntry(node, tab, 0, entries));
  }

  previousTabEntries = entries;
  return tree;
}

/**
 * Groups one section only when every authoritative split member is present.
 * Cross-section and malformed groups stay as independent rows; this never
 * clones or removes a presentation sentinel.
 */
export function sidebarDisplayUnits(
  section: SidebarSectionView,
  entries: readonly SidebarEntry[],
  splitGroup: SplitGroupView | null,
): SidebarDisplayUnit[] {
  if (splitGroup === null || splitGroup.members.length < 2) return [...entries];

  const positions = new Map<string, { index: number; entry: SidebarTabEntry }>();
  entries.forEach((entry, index) => {
    if (entry.kind === "tab") positions.set(entry.tab.id, { index, entry });
  });

  const ids = new Set<string>();
  const members: SidebarTabEntry[] = [];
  let insertionIndex = entries.length;
  for (const id of splitGroup.members) {
    const member = positions.get(id);
    if (member === undefined || ids.has(id)) return [...entries];
    ids.add(id);
    members.push(member.entry);
    insertionIndex = Math.min(insertionIndex, member.index);
  }

  const output: SidebarDisplayUnit[] = [];
  entries.forEach((entry, index) => {
    if (index === insertionIndex) {
      output.push({
        kind: "split",
        key: `split:${section}`,
        tabs: members,
        depth: Math.min(...members.map((member) => member.depth)),
      });
    }
    if (entry.kind !== "tab" || !ids.has(entry.tab.id)) output.push(entry);
  });
  return output;
}

/** Keep the real active tab visible even when its parent was folded. */
export function collapsedSidebarUnits(
  units: readonly SidebarDisplayUnit[],
  folded: ReadonlySet<string>,
  activeId: string | null,
): Set<string> {
  const hidden = new Set<string>();
  const ancestors: number[] = [];
  for (let index = 0; index < units.length; index++) {
    const unit = units[index]!;
    while (ancestors.length && ancestors[ancestors.length - 1]! >= unit.depth) ancestors.pop();
    if (ancestors.length) hidden.add(unit.key);
    if (unit.kind !== "folder" || !folded.has(unit.key)) continue;
    let hasActive = false;
    for (let childIndex = index + 1; childIndex < units.length; childIndex++) {
      const child = units[childIndex]!;
      if (child.depth <= unit.depth) break;
      if (
        (child.kind === "tab" && child.tab.id === activeId) ||
        (child.kind === "split" && child.tabs.some((entry) => entry.tab.id === activeId))
      ) {
        hasActive = true;
        break;
      }
    }
    if (!hasActive) ancestors.push(unit.depth);
  }
  return hidden;
}
