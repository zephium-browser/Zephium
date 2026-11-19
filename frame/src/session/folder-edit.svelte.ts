import { untrack } from "svelte";
import { SvelteSet } from "svelte/reactivity";
import { tabs } from "$domain/tabs";
import type { FolderColor } from "$shared/ipc/bindings";

/**
 * The sidebar's folder in hand: the one whose name is being typed, and a
 * fold asked for but not yet answered. A new folder is made by native under
 * a default name; its row opens for renaming once it is projected.
 */

let renaming = $state<string | null>(null);
// The folders there were before a create, in the space it was made in, so
// the new one is the one not among them; given up if the projection never
// brings one.
type Awaiting = { known: ReadonlySet<string>; scope: string; settled: () => void };
let awaiting = $state.raw<Awaiting | null>(null);
let awaitTimer: ReturnType<typeof setTimeout> | undefined;
const folds = $state<Record<string, boolean>>({});

const folderIds = () =>
  new Set(
    tabs
      .sidebarNodes()
      .filter((node) => node.kind.type === "folder")
      .map((node) => node.id),
  );

const scope = () => `${tabs.profile()?.id ?? ""}/${tabs.activeSpaceId() ?? ""}`;

function settle() {
  const was = awaiting;
  clearTimeout(awaitTimer);
  awaiting = null;
  was?.settled();
}

export const renamingId = () => renaming;

/** Whether this folder is the one a create just made, so it should open for renaming. */
export const fresh = (id: string) =>
  awaiting !== null && awaiting.scope === scope() && !awaiting.known.has(id);

export function rename(id: string) {
  settle();
  renaming = id;
}

/** The fresh folder's row has opened for renaming. */
export function claim(id: string) {
  if (!fresh(id)) return;
  rename(id);
}

export function stopRenaming(id: string) {
  if (renaming === id) renaming = null;
}

export async function commitName(id: string, name: string, current: string) {
  stopRenaming(id);
  const trimmed = name.trim();
  if (trimmed === "" || trimmed === current) return;
  await tabs.folder({ type: "rename", id, name: trimmed });
}

type CreateOptions = { tabs?: string[]; parent?: string | null; color?: FolderColor | null };

// One create at a time, each until its folder is found, so two never take
// each other's row.
let creating: Promise<void> = Promise.resolve();

/** A new folder, holding `tabs` or empty inside `parent`; it opens for its name. */
export function create(name: string, options: CreateOptions = {}): Promise<void> {
  // Native files it in the space in front when it is sent, so it is meant
  // for the space in front now.
  const where = scope();
  const run = creating.then(() => createNow(name, options, where));
  creating = run.catch(() => {});
  return run;
}

async function createNow(name: string, options: CreateOptions, where: string) {
  if (scope() !== where) return;
  const known = folderIds();
  const result = await tabs.folder({
    type: "create",
    name,
    color: options.color ?? null,
    tabs: options.tabs ?? [],
    parent: options.parent ?? null,
    before: null,
  });
  if (result.outcome !== "applied" && result.outcome !== "deferred") return;
  // Another space is in front now; its folders are not this one.
  if (scope() !== where) return;
  const made = [...folderIds()].find((id) => !known.has(id));
  if (made !== undefined) {
    rename(made);
    return;
  }
  await new Promise<void>((settled) => {
    settle();
    awaiting = { known, scope: where, settled };
    awaitTimer = setTimeout(settle, 2000);
  });
}

/** A fold as the sidebar draws it: the one asked for until native answers. */
export const folded = (id: string, projected: boolean) => folds[id] ?? projected;

// Asked folds not yet answered; an answered one gives way once the
// projection shows it, or at once if native refused it.
const asking = new SvelteSet<string>();

export async function setFolded(id: string, collapsed: boolean) {
  folds[id] = collapsed;
  asking.add(id);
  const result = await tabs.folder({ type: "set_collapsed", id, collapsed });
  if (folds[id] !== collapsed) return;
  asking.delete(id);
  if (result.outcome !== "applied" && result.outcome !== "deferred") delete folds[id];
  else settleFolds();
}

function settleFolds() {
  const nodes = tabs.sidebarNodes();
  for (const id of Object.keys(folds)) {
    if (asking.has(id)) continue;
    const node = nodes.find((candidate) => candidate.id === id);
    if (node?.kind.type !== "folder" || node.kind.collapsed === folds[id]) delete folds[id];
  }
}

$effect.root(() => {
  $effect(() => {
    void tabs.sidebarNodes();
    untrack(settleFolds);
  });
});

// Group into Folder asks the sidebar, which knows the selection. With no
// sidebar drawn the caller groups the tab in front itself.
let grouper: (() => void) | null = null;

/** The sidebar takes Group into Folder while it is drawn; returns its release. */
export function handleGroup(handler: () => void) {
  grouper = handler;
  return () => {
    if (grouper === handler) grouper = null;
  };
}

/** Whether a sidebar took the ask. */
export function askGroup(): boolean {
  if (grouper === null) return false;
  grouper();
  return true;
}
