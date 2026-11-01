/**
 * Open files arranged in up to four panes, each with its own tab strip, the
 * way Obsidian splits a workspace. Saved per profile so the layout returns,
 * and named layouts can be saved and restored ("study ITF").
 */
export type Layout = "single" | "columns" | "rows" | "grid";

export type Pane = { id: number; tabs: string[]; active: string | null };

export type Snapshot = { layout: Layout; panes: Pane[]; focused: number };

const MAX_PANES = 4;
const STORE = "zephium.vault.workspace.";
const LAYOUTS = "zephium.vault.layouts.";

function paneCount(layout: Layout): number {
  return layout === "single" ? 1 : layout === "grid" ? 4 : 2;
}

export class Workspace {
  layout = $state<Layout>("single");
  panes = $state<Pane[]>([{ id: 0, tabs: [], active: null }]);
  focused = $state(0);
  #key: string;
  #next = 1;

  constructor(profile: string, storage: Pick<Storage, "getItem" | "setItem"> | null = null) {
    this.#key = profile;
    this.#storage = storage;
    const saved = this.#read(STORE + profile);
    if (saved) this.restore(saved);
  }

  #storage: Pick<Storage, "getItem" | "setItem"> | null;

  #read(key: string): Snapshot | null {
    try {
      const text = this.#storage?.getItem(key);
      if (!text) return null;
      const value = JSON.parse(text) as Snapshot;
      return Array.isArray(value.panes) ? value : null;
    } catch {
      return null;
    }
  }

  snapshot(): Snapshot {
    return {
      layout: this.layout,
      focused: this.focused,
      panes: this.panes.map((pane) => ({ id: pane.id, tabs: [...pane.tabs], active: pane.active })),
    };
  }

  restore(snapshot: Snapshot) {
    const panes = snapshot.panes.slice(0, MAX_PANES).map((pane, index) => ({
      id: index,
      tabs: pane.tabs.filter((tab) => typeof tab === "string").slice(0, 50),
      active: pane.active,
    }));
    if (panes.length === 0) panes.push({ id: 0, tabs: [], active: null });
    this.panes = panes;
    this.#next = panes.length;
    this.layout = snapshot.layout;
    this.#fit();
    this.focused = Math.min(Math.max(snapshot.focused, 0), this.panes.length - 1);
  }

  save() {
    try {
      this.#storage?.setItem(STORE + this.#key, JSON.stringify(this.snapshot()));
    } catch {
      // A full or unavailable store only loses the layout, never a file.
    }
  }

  get pane(): Pane {
    const pane = this.panes[this.focused] ?? this.panes[0];
    if (!pane) throw new Error("a workspace always has a pane");
    return pane;
  }

  /** The file in front of the focused pane. */
  get current(): string | null {
    return this.pane.active;
  }

  /** Opens a file in the focused pane, or brings its tab forward. */
  open(path: string, options: { pane?: number } = {}) {
    const index = options.pane ?? this.focused;
    const pane = this.panes[index] ?? this.pane;
    if (!pane.tabs.includes(path)) {
      const at = pane.active ? pane.tabs.indexOf(pane.active) + 1 : pane.tabs.length;
      pane.tabs.splice(at, 0, path);
    }
    pane.active = path;
    this.focused = this.panes.indexOf(pane);
    this.save();
  }

  /** Opens beside: in the next pane, splitting when there is only one. */
  openBeside(path: string) {
    if (this.panes.length === 1) this.setLayout("columns");
    this.open(path, { pane: (this.focused + 1) % this.panes.length });
  }

  close(paneIndex: number, path: string) {
    const pane = this.panes[paneIndex];
    if (!pane) return;
    const at = pane.tabs.indexOf(path);
    if (at < 0) return;
    pane.tabs.splice(at, 1);
    if (pane.active === path) pane.active = pane.tabs[Math.min(at, pane.tabs.length - 1)] ?? null;
    this.save();
  }

  closeCurrent() {
    if (this.pane.active) this.close(this.focused, this.pane.active);
  }

  activate(paneIndex: number, path: string) {
    const pane = this.panes[paneIndex];
    if (!pane?.tabs.includes(path)) return;
    pane.active = path;
    this.focused = paneIndex;
    this.save();
  }

  /** Ctrl+PageDown / Ctrl+PageUp: the next or previous tab in the focused pane. */
  cycle(step: 1 | -1) {
    const pane = this.pane;
    if (pane.tabs.length === 0) return;
    const at = pane.active ? pane.tabs.indexOf(pane.active) : -1;
    pane.active = pane.tabs[(at + step + pane.tabs.length) % pane.tabs.length] ?? null;
    this.save();
  }

  focusPane(step: 1 | -1) {
    this.focused = (this.focused + step + this.panes.length) % this.panes.length;
    this.save();
  }

  setLayout(layout: Layout) {
    this.layout = layout;
    this.#fit();
    this.save();
  }

  /** Grows or merges panes to match the layout; merged tabs join the first pane. */
  #fit() {
    const wanted = paneCount(this.layout);
    while (this.panes.length < wanted) {
      this.panes.push({ id: this.#next++, tabs: [], active: null });
    }
    while (this.panes.length > wanted) {
      const removed = this.panes.pop();
      if (!removed) break;
      const keep = this.panes[0];
      if (!keep) break;
      for (const tab of removed.tabs) if (!keep.tabs.includes(tab)) keep.tabs.push(tab);
    }
    this.focused = Math.min(this.focused, this.panes.length - 1);
  }

  /** Follows a rename or move so open tabs keep their file. */
  renamed(from: string, to: string) {
    for (const pane of this.panes) {
      pane.tabs = pane.tabs.map((tab) =>
        tab === from ? to : tab.startsWith(`${from}/`) ? to + tab.slice(from.length) : tab,
      );
      if (pane.active === from) pane.active = to;
      else if (pane.active?.startsWith(`${from}/`))
        pane.active = to + pane.active.slice(from.length);
    }
    this.save();
  }

  /** Drops tabs for a file or folder that was trashed. */
  removed(path: string) {
    for (const [index, pane] of this.panes.entries()) {
      for (const tab of [...pane.tabs]) {
        if (tab === path || tab.startsWith(`${path}/`)) this.close(index, tab);
      }
    }
  }

  layouts(): Record<string, Snapshot> {
    try {
      const text = this.#storage?.getItem(LAYOUTS + this.#key);
      return text ? (JSON.parse(text) as Record<string, Snapshot>) : {};
    } catch {
      return {};
    }
  }

  saveLayout(name: string) {
    const all = this.layouts();
    all[name] = this.snapshot();
    try {
      this.#storage?.setItem(LAYOUTS + this.#key, JSON.stringify(all));
    } catch {
      // Unavailable storage keeps the current layout only.
    }
  }

  loadLayout(name: string): boolean {
    const layout = this.layouts()[name];
    if (!layout) return false;
    this.restore(layout);
    this.save();
    return true;
  }

  deleteLayout(name: string) {
    const all = this.layouts();
    delete all[name];
    try {
      this.#storage?.setItem(LAYOUTS + this.#key, JSON.stringify(all));
    } catch {
      // Nothing else depends on the stored list.
    }
  }
}
