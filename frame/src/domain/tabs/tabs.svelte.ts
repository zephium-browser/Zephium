import { listenAll } from "$shared/lib/lifecycle";
import { forgetDrafts } from "$shared/lib/drafts";
import { flushSync } from "svelte";
import type { ItemsState, SpaceLookInputV1, TabMenuContext, TabView } from "$shared/ipc/bindings";
import { settle } from "$domain/operations";
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { TabProjectionModel } from "./tabs-model";

const model = new TabProjectionModel();
let state = $state.raw<ItemsState>(model.value);

export const tabs = () => state.tabs;
export const activeId = () => state.active;
export const splitGroup = () => state.split_group;
export const profile = () => state.profile;
export const spaces = () => state.spaces;
export const activeSpaceId = () => state.active_space_id;
export const activeSpace = () => state.spaces.find((space) => space.id === state.active_space_id);
export const sidebarNodes = () => state.nodes;
export const activeTab = (): TabView | undefined =>
  state.tabs.find((tab) => tab.id === state.active);

type Unlisten = () => void;

let lifecycle = 0;
let initialized = false;
let initializing: Promise<void> | null = null;
let unlisten: Unlisten | null = null;

function publishModelState() {
  state = model.value;
}

async function initialize(generation: number) {
  // These calls synchronously install their DOM listeners. Keep every scoped
  // listener registration before the first await so no privileged projection
  // can arrive in a gap during startup.
  const listeners = await listenAll([
    events.itemsChanged.listen((event) => {
      if (generation !== lifecycle) return;
      if (model.applySnapshot(event.payload)) publishModelState();
    }),
    events.tabChanged.listen((event) => {
      if (generation !== lifecycle) return;
      if (model.applyTab(event.payload)) publishModelState();
    }),
    events.browserReturn.listen((event) => {
      if (generation !== lifecycle) return;
      flushSync(() => {
        if (model.applySnapshot(event.payload)) publishModelState();
      });
    }),
    events.presentationTab.listen((event) => {
      if (generation !== lifecycle) return;

      // Rust verifies the committed chrome in the same synchronous dispatch.
      // State admission and DOM publication therefore belong to one flush.
      flushSync(() => {
        if (model.applyPresentation(event.payload)) publishModelState();
      });
    }),
  ]);

  if (generation !== lifecycle) {
    for (const stop of listeners) stop();
    return;
  }

  unlisten = () => {
    for (const stop of listeners) stop();
  };
  initialized = true;

  // A cold first run can outrace Tauri setup. Preserve the bounded bootstrap
  // retry while allowing disposal to cancel further attempts.
  for (let attempt = 0; attempt < 20 && generation === lifecycle; attempt += 1) {
    try {
      await commands.tabsBootstrap();
      return;
    } catch {
      await new Promise<void>((resolve) => setTimeout(resolve, 250));
    }
  }
}

export function init(): Promise<void> {
  if (initializing !== null) return initializing;
  if (initialized) return Promise.resolve();

  const generation = ++lifecycle;
  const task = initialize(generation);
  initializing = task;
  void task.then(
    () => {
      if (initializing === task) initializing = null;
    },
    () => {
      if (initializing === task) initializing = null;
    },
  );
  return task;
}

export function dispose() {
  if (!initialized && initializing === null && unlisten === null) return;

  lifecycle += 1;
  initialized = false;
  initializing = null;
  unlisten?.();
  unlisten = null;
}

export const open = () => void commands.tabsOpen();
export const activate = (id: string) => void commands.tabsActivate(id);
export const close = (id: string) => void commands.tabsClose(id);
/** The window's one tab, with nothing open in it yet. Closing it would only
 *  put another empty tab in its place, so it offers no close. */
export const loneNewTab = (id: string) => {
  const all = state.tabs;
  const tab = all.length === 1 ? all[0] : undefined;
  return tab?.id === id && !tab.url && !tab.failure && (tab.content ?? "web") === "web";
};
export const navigate = (id: string, input: string) => void commands.tabsNavigate(id, input);

const onActive = (fn: (id: string) => void) => () => {
  const id = state.active;
  if (id !== null) fn(id);
};

/** Mutes or unmutes a tab; the projected tab says what native applied. */
export const setMuted = (id: string, muted: boolean) => settle(commands.tabSetMuted(id, muted));
export const reloadActive = onActive((id) => void commands.tabsReload(id));
export const backActive = onActive((id) => void commands.tabsBack(id));
export const forwardActive = onActive((id) => void commands.tabsForward(id));
export const split = (other: string) => void commands.tabsSplit(other);
export const unsplit = () => void commands.tabsUnsplit();
export const inSplit = (id: string) => state.split_group?.members.includes(id) ?? false;
export const dragOver = (x: number, y: number) => void commands.tabDragOver(x, y);
export const dropTab = (id: string, x: number, y: number) => void commands.tabDrop(id, x, y);
/** Keeps one of onboarding's catalog sites in Essentials, named by its id;
 *  native owns the address. Settles on native's answer, never assumed. */
export const keepSite = (site: string) => settle(commands.essentialsKeep(site));
/** Lets go of a site kept and never opened; closing an Essential keeps it. */
export const releaseSite = (site: string) => settle(commands.essentialsRelease(site));
/** Names the focused profile, which is who the browser greets. */
export const renameProfile = (name: string) => settle(commands.profileRename(name));

/** A tab pairs with the active one, so it must exist and not already be it. */
export const canSplitWith = (id: string) =>
  state.tabs.length > 1 && state.active !== null && state.active !== id;

/** A new tab has nothing to pair, so split stays unavailable until it loads. */
export const canSplitActive = () => state.tabs.length > 1 && (activeTab()?.url ?? null) !== null;

// Native holds the authoritative menu target; this mirror exists only so the
// clipboard action can resolve a URL privileged chrome already projects.
let menuTarget: string | null = null;

/** What the tab menu can offer for one tab; native checks each action again. */
function tabMenuContext(id: string, mediaOnly = false): TabMenuContext {
  const tab = state.tabs.find((candidate) => candidate.id === id);
  const tabOf = (node: ItemsState["nodes"][number]) =>
    node.kind.type === "tab" ? node.kind.tab_id : null;
  const open = state.nodes
    .filter((node) => node.section === "today" && node.parent_id === null)
    .map(tabOf)
    .filter((candidate) => candidate !== null);
  const at = open.indexOf(id);
  return {
    page: (tab?.content ?? "web") === "web" && (tab?.url ?? null) !== null,
    can_split: canSplitWith(id),
    essential: state.nodes.some((node) => tabOf(node) === id && node.section === "favorites"),
    others: open.some((candidate) => candidate !== id),
    below: at >= 0 && at < open.length - 1,
    muted: tab?.audio?.muted ?? false,
    capture: tab?.capture?.navigation_id ?? null,
    media_only: mediaOnly,
  };
}

export function openTabMenu(id: string, x: number, y: number) {
  menuTarget = id;
  void commands.tabMenuPopup(id, x, y, tabMenuContext(id));
}

/** What can be done about a tab's camera, microphone and sound, opened from
 *  its in-use indicator. */
export function openMediaMenu(id: string, x: number, y: number) {
  menuTarget = id;
  void commands.tabMenuPopup(id, x, y, tabMenuContext(id, true));
}

/** The browser interface's own menu; `page` says a web page is in front. */
export function openChromeMenu(x: number, y: number, page: boolean) {
  void commands.chromeMenuPopup(x, y, page, canSplitActive());
}

export function copyMenuTargetLink(): Promise<boolean> {
  const id = menuTarget;
  menuTarget = null;
  if (id === null) return Promise.resolve(false);
  return copyLink(state.tabs.find((tab) => tab.id === id)?.url);
}

/** The address of the tab the menu was opened on, taken once. */
export function takeMenuTargetUrl(): string | null {
  const id = menuTarget;
  menuTarget = null;
  return id === null ? null : (state.tabs.find((tab) => tab.id === id)?.url ?? null);
}

/** Copies the address of the page in front, for the Copy Link command. */
export function copyActiveLink(): Promise<boolean> {
  return copyLink(activeTab()?.url);
}

/** Whether the address reached the clipboard, so chrome can say so. */
function copyLink(url: string | null | undefined): Promise<boolean> {
  if (!url) return Promise.resolve(false);
  return navigator.clipboard.writeText(url).then(
    () => true,
    // A denied clipboard is a user-visible no-op, never a chrome failure.
    () => false,
  );
}

/** Space intents settle on native's answer; the projection carries the result. */
export const switchSpace = (id: string) => settle(commands.spacesSwitch(id));
export const stepSpace = (step: 1 | -1) => settle(commands.spacesStep(step));
export const createSpace = (name: string, look: SpaceLookInputV1) =>
  settle(commands.spacesCreate(name, look));
export const updateSpace = (id: string, name: string, look: SpaceLookInputV1) =>
  settle(commands.spacesUpdate(id, name, look));
export async function deleteSpace(id: string) {
  const owner = state.profile?.id;
  const result = await settle(commands.spacesDelete(id));
  if (owner && (result.outcome === "applied" || result.outcome === "deferred"))
    forgetDrafts(owner, id);
  return result;
}
export const reorderSpace = (id: string, before: string | null) =>
  settle(commands.spacesReorder(id, before));
export const moveTabToSpace = (tab: string, space: string) =>
  settle(commands.spacesMoveTab(tab, space));
