import { SvelteMap, SvelteSet } from "svelte/reactivity";
import {
  commands,
  type HomeInsideRefusalV1,
  type HomeMapChangedV1_Serialize,
  type MapEntry_Serialize,
  type MapTarget,
  type OperationReason,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { settle } from "$domain/operations";
import { tabs } from "$domain/tabs";
import { model as project, type Local } from "./admit";
import { HomeFeed, type FeedState } from "./feed";
import type { HomeModel, Refusal } from "./model";

/** Why a request from Home did not go through, said to the person. */
export type Refused = "gone" | "busy" | "unavailable" | "failed";

let feed: FeedState = $state.raw({
  scope: null,
  searches: [],
  maps: new Map(),
  unavailable: false,
});
let now = $state(Date.now());
const unfolded = new SvelteSet<string>();
const waiting = new SvelteSet<string>();
const refused = new SvelteMap<string, Refusal>();
let users = 0;
let stopScope: (() => void) | null = null;

/** A waiting mark yields to the first change that answers for its entry, or to the map's end. */
function settleWaits(touched: HomeMapChangedV1_Serialize | null) {
  if (!touched) return;
  const prefix = `${touched.search}/`;
  const change = touched.change;
  for (const key of [...waiting]) {
    if (!key.startsWith(prefix)) continue;
    const entry = key.slice(prefix.length);
    const answered =
      change.type === "end" ||
      (change.type === "opened" && change.id === entry) ||
      (change.type === "place" &&
        change.place.sections.some((one) => one.id === entry && !!one.read));
    if (answered) waiting.delete(key);
  }
}

const homeFeed = new HomeFeed(
  {
    snapshot: () => commands.homeSnapshot() as ReturnType<typeof commands.homeSnapshot>,
    onMap: (listener) =>
      events.homeMapChanged.listen(({ payload }) =>
        listener(payload as HomeMapChangedV1_Serialize),
      ),
    onChanged: (listener) => events.homeChanged.listen(({ payload }) => listener(payload)),
  },
  (state, touched) => {
    if (state.scope === null || state.searches !== feed.searches) {
      const held = new Set(state.searches.map((search) => search.id));
      for (const set of [unfolded, waiting])
        for (const key of [...set]) if (!held.has(key.split("/")[0]!)) set.delete(key);
      for (const key of [...refused.keys()]) if (!held.has(key.split("/")[0]!)) refused.delete(key);
    }
    settleWaits(touched);
    now = Date.now();
    feed = state;
  },
);

/** Home is drawn somewhere: native state is followed until the last one lets go. */
export function retain(): () => void {
  users++;
  if (users === 1) {
    void homeFeed.start();
    stopScope = $effect.root(() => {
      $effect(() => homeFeed.follow(tabs.profile()?.id ?? null, tabs.activeSpaceId()));
    });
  }
  let held = true;
  return () => {
    if (!held) return;
    held = false;
    users--;
    if (users > 0) return;
    stopScope?.();
    stopScope = null;
    homeFeed.stop();
    unfolded.clear();
    waiting.clear();
    refused.clear();
  };
}

const tabMap = $derived(new Map(tabs.tabs().map((tab) => [tab.id, tab] as const)));
const local: Local = {
  get unfolded() {
    return unfolded;
  },
  get waiting() {
    return waiting;
  },
  get refused() {
    return refused;
  },
  get tabs() {
    return tabMap;
  },
};
const drawn: HomeModel = $derived(project(feed.searches, feed.maps, local, now));

export const model = () => drawn;
/** The first answer for this scope has come back. */
export const ready = () => feed.scope !== null;

const reasonOf = (reason: OperationReason | undefined): Refused =>
  reason === "item_limit_reached" ? "busy" : reason === "home_search_unknown" ? "gone" : "failed";

async function run(
  admission: ReturnType<typeof commands.homeOpenResults>,
): Promise<Refused | null> {
  const result = await settle(admission);
  if (result.outcome === "applied" || result.outcome === "no_op" || result.outcome === "deferred")
    return null;
  return result.outcome === "rejected" ? reasonOf(result.disposition?.reason) : "failed";
}

/** Where `entry` stands in `search`: its place's slot, and the row itself when it is one. */
function find(
  search: string,
  entry: string,
): { slot: number; row: MapEntry_Serialize | null } | null {
  const map = feed.maps.get(search);
  if (!map) return null;
  for (const slot of [0, 1] as const) {
    const state = map.places[slot];
    if (state?.kind !== "place") continue;
    for (const row of state.place.sections) {
      if (row.id === entry) return { slot, row };
      if (row.children.some((child) => child.id === entry)) return { slot, row: null };
    }
  }
  return null;
}

/** Opens a page of a search full: a row, its site, a profile or another result. */
export function open(search: string, target: MapTarget, slot?: number) {
  const at = slot ?? (target.kind === "entry" ? (find(search, target.id)?.slot ?? null) : 0);
  if (at === null) return Promise.resolve<Refused | null>("unavailable");
  return run(commands.homeOpenTarget(search, at, target));
}

const insideRefusal = (refusal: HomeInsideRefusalV1 | null): Refusal =>
  refusal === "map_gone" ? "gone" : refusal === "too_many_waiting" ? "busy" : "unavailable";

/** Shows a row's inside: what is held at once, else it is read, the only time it is. */
export async function openInside(search: string, entry: string) {
  const key = `${search}/${entry}`;
  unfolded.add(key);
  refused.delete(key);
  const found = find(search, entry);
  const read = found?.row?.read;
  if (read === "read" || read === "queued" || waiting.has(key)) return;
  if (!found) {
    refused.set(key, "unavailable");
    return;
  }
  try {
    const answer = await commands.homeOpenInside(search, found.slot, entry);
    if (!unfolded.has(key)) return;
    if (answer.accepted) waiting.add(key);
    else refused.set(key, insideRefusal(answer.refusal));
  } catch {
    if (unfolded.has(key)) refused.set(key, "unavailable");
  }
}

export function foldInside(search: string, entry: string) {
  unfolded.delete(`${search}/${entry}`);
}

export const openResults = (search: string) => run(commands.homeOpenResults(search));
export const focus = (search: string) => run(commands.homeFocusSearch(search));
export const forget = (search: string) => run(commands.homeForgetSearch(search));
