import type {
  FolderColor,
  SidebarNodeView,
  SidebarSectionView,
  SplitGroupView,
  TabView,
} from "$shared/ipc/bindings";

export type SidebarFolderEntry = {
  kind: "folder";
  key: string;
  node: SidebarNodeView & {
    kind: { type: "folder"; name: string; color: FolderColor | null; collapsed: boolean };
  };
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

const holds = (unit: SidebarDisplayUnit, id: string | null) =>
  (unit.kind === "tab" && unit.tab.id === id) ||
  (unit.kind === "split" && unit.tabs.some((entry) => entry.tab.id === id));

/** A folded folder hides what it holds, except the tab in front, which
 *  stays beneath it with the folders on its way. */
export function collapsedSidebarUnits(
  units: readonly SidebarDisplayUnit[],
  folded: ReadonlySet<string>,
  activeId: string | null,
): Set<string> {
  const hidden = new Set<string>();
  for (let index = 0; index < units.length; index++) {
    const unit = units[index]!;
    if (unit.kind !== "folder" || !folded.has(unit.key)) continue;
    let end = index + 1;
    while (end < units.length && units[end]!.depth > unit.depth) end += 1;
    const kept = new Set<number>();
    for (let at = index + 1; at < end; at++) {
      if (!holds(units[at]!, activeId)) continue;
      kept.add(at);
      let depth = units[at]!.depth;
      for (let up = at - 1; up > index; up--)
        if (units[up]!.depth < depth) {
          kept.add(up);
          depth = units[up]!.depth;
        }
      break;
    }
    for (let at = index + 1; at < end; at++) if (!kept.has(at)) hidden.add(units[at]!.key);
  }
  return hidden;
}

/** What a folded folder says about what it holds. */
export type FolderSummary = {
  /** Every tab under it, nested folders' included. */
  count: number;
  /** The first few that have an icon to show, in sidebar order. */
  faces: TabView[];
  audible: boolean;
  loading: boolean;
  /** Its own tabs that are not asleep. */
  awake: number;
  /** The deepest folder level inside it, itself counted. */
  levels: number;
};

export type FolderContext = {
  summaries: Map<string, FolderSummary>;
  /** For each row inside a folder, the colour of every folder around it, outermost first. */
  spines: Map<string, (FolderColor | null)[]>;
};

const FACES = 3;

/** One pass over the rows: each folder's summary and each row's spines. */
export function folderContext(units: readonly SidebarDisplayUnit[]): FolderContext {
  const summaries = new Map<string, FolderSummary>();
  const spines = new Map<string, (FolderColor | null)[]>();
  const open: { depth: number; color: FolderColor | null; summary: FolderSummary }[] = [];
  for (const unit of units) {
    while (open.length && open[open.length - 1]!.depth >= unit.depth) open.pop();
    if (open.length)
      spines.set(
        unit.key,
        open.map((folder) => folder.color),
      );
    const tabs =
      unit.kind === "folder"
        ? []
        : unit.kind === "split"
          ? unit.tabs.map((e) => e.tab)
          : [unit.tab];
    for (const folder of open) {
      const summary = folder.summary;
      for (const tab of tabs) {
        summary.count += 1;
        if (summary.faces.length < FACES && tab.url) summary.faces.push(tab);
        if (tab.audio && !tab.audio.muted) summary.audible = true;
        if (tab.loading) summary.loading = true;
        if (!tab.asleep) summary.awake += 1;
      }
    }
    if (unit.kind !== "folder") continue;
    const summary: FolderSummary = {
      count: 0,
      faces: [],
      audible: false,
      loading: false,
      awake: 0,
      levels: 1,
    };
    open.forEach((folder, at) => {
      folder.summary.levels = Math.max(folder.summary.levels, open.length - at + 1);
    });
    summaries.set(unit.key, summary);
    open.push({ depth: unit.depth, color: unit.node.kind.color, summary });
  }
  return { summaries, spines };
}
