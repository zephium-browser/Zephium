import type {
  HomePageV1,
  HomeSearchV1_Serialize,
  HomeSnapshotV1_Serialize,
  IconRef,
  MapEntry_Serialize,
  MapFailure,
  MapPlace_Serialize,
  TabView,
} from "$shared/ipc/bindings";
import { replay, type MapState } from "./fold";
import {
  clean,
  fitId,
  MAX_ACTIONS,
  MAX_ALSO,
  MAX_CHILDREN,
  MAX_KNOWN,
  MAX_LABEL,
  MAX_MORE,
  MAX_PAGES,
  MAX_ROWS,
  MAX_SEARCHES,
  MAX_SOCIAL,
  type Address,
  type Day,
  type Entry,
  type HomeModel,
  type Inside,
  type Opened,
  type Place,
  type Refusal,
  type Row,
  type Section,
} from "./model";

/** What the frame itself holds about a search beside native state. */
export type Local = {
  /** Insides the person opened, by `search/entry`. */
  unfolded: ReadonlySet<string>;
  /** Insides asked for and admitted, not yet answered on the stream, by `search/entry`. */
  waiting: ReadonlySet<string>;
  /** Insides refused at admission, by `search/entry`. */
  refused: ReadonlyMap<string, Refusal>;
  tabs: ReadonlyMap<string, TabView>;
};

const label = (text: string) => clean(text, MAX_LABEL);

/** A local date key and the minutes after its midnight. */
function when(atMs: number | null, now: number): { day: string; at: number } {
  const date = new Date(atMs !== null && Number.isFinite(atMs) && atMs > 0 ? atMs : now);
  const pad = (value: number) => String(value).padStart(2, "0");
  return {
    day: `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
    at: date.getHours() * 60 + date.getMinutes(),
  };
}

function section(entry: MapEntry_Serialize): Section | null {
  const id = fitId(entry.id);
  const text = label(entry.label);
  return id && text ? { id, label: text } : null;
}

const sections = (entries: readonly MapEntry_Serialize[], limit: number) =>
  entries.flatMap((entry) => section(entry) ?? []).slice(0, limit);

function child(entry: MapEntry_Serialize, opened: ReadonlySet<string>): Row | null {
  const head = section(entry);
  if (!head) return null;
  return {
    ...head,
    host: entry.off_site && entry.host ? label(entry.host) || null : null,
    known: [],
    opens: false,
    menu: false,
    inside: null,
    opened: opened.has(head.id),
  };
}

function row(
  entry: MapEntry_Serialize,
  key: string,
  slot: number,
  local: Local,
  opened: ReadonlySet<string>,
  why: ReadonlyMap<string, MapFailure>,
): Row | null {
  const head = child(entry, opened);
  if (!head) return null;
  const menu = entry.menu_only;
  const children = entry.children.flatMap((next) => child(next, opened) ?? []);
  const held = children.slice(0, MAX_CHILDREN);
  const local_ = `${key}/${head.id}`;
  let inside: Inside | null = null;
  if (menu) inside = { state: "open", rows: held, more: children.length > MAX_CHILDREN };
  else if (entry.open && local.unfolded.has(local_)) {
    const refused = local.refused.get(local_) ?? why.get(`${slot}/${head.id}`);
    if (entry.read === "read" && held.length)
      inside = { state: "open", rows: held, more: entry.more || children.length > MAX_CHILDREN };
    else if (entry.read === "refused" || refused)
      inside = { state: "refused", reason: refused ?? "unreadable" };
    else if (entry.read === "queued" || local.waiting.has(local_)) inside = { state: "opening" };
  }
  return {
    ...head,
    host: menu ? null : head.host,
    known: menu ? [] : held.slice(0, MAX_KNOWN).map(({ id, label }) => ({ id, label })),
    opens: !menu && entry.open,
    menu,
    inside,
    opened: !menu && head.opened,
  };
}

/** A menu with nothing held under it leads nowhere, and is not drawn. */
const leads = (row: Row) =>
  !row.menu || (row.inside?.state === "open" && row.inside.rows.length > 0);

function place(
  search: HomeSearchV1_Serialize,
  map: MapState,
  slot: 0 | 1,
  local: Local,
  pages: readonly HomePageV1[],
): Place {
  const opened = new Set(
    pages.flatMap((one) => (one.slot === slot && one.entry ? [one.entry] : [])),
  );
  const state = map.places[slot];
  const official = slot === 0 ? map.first?.official : null;
  const base: Place = {
    id: String(slot),
    title: label(official?.title ?? ""),
    host: "",
    state: "arriving",
    thin: false,
    failure: null,
    rows: [],
    more: [],
    actions: [],
    social: [],
    asked: null,
    icon: icon(
      pages.filter((one) => one.slot === slot),
      local.tabs,
    ),
  };
  const ended = map.end !== null;
  if (state?.kind === "failed") return { ...base, state: "failed", failure: state.failure };
  if (!state) {
    if (ended || (map.first && !official))
      return {
        ...base,
        state: "failed",
        failure: map.end?.kind === "failed" ? map.end.failure : "missing",
      };
    return base;
  }
  const source: MapPlace_Serialize = state.place;
  const key = search.id;
  const of = (tier: MapEntry_Serialize["tier"]) =>
    source.sections.filter((entry) => entry.tier === tier);
  const rows = of("primary")
    .flatMap((entry) => row(entry, key, slot, local, opened, map.refused) ?? [])
    .filter(leads)
    .slice(0, MAX_ROWS);
  const asked = fitId(source.focus);
  return {
    ...base,
    title: label(source.title) || label(source.host),
    host: label(source.host),
    state: state.complete || ended || !map.live ? "settled" : "arriving",
    thin: source.notes.thin === true,
    rows,
    more: sections(of("secondary"), MAX_MORE),
    actions: sections(of("action"), MAX_ACTIONS),
    social: [...new Set(source.social)].slice(0, MAX_SOCIAL),
    asked: rows.some((one) => one.id === asked) ? asked : null,
  };
}

/** The page of a search as Home names it: the tab's own title while it is open. */
function page(entry: HomePageV1, index: number, tabs: ReadonlyMap<string, TabView>): Opened {
  const tab = entry.tab ? tabs.get(entry.tab) : undefined;
  return {
    id: `${index}`,
    title: clean(entry.open && tab ? tab.title : entry.title) || label(entry.host),
    host: label(entry.host),
    tab: entry.open && tab ? tab.id : null,
  };
}

function icon(pages: readonly HomePageV1[], tabs: ReadonlyMap<string, TabView>): IconRef | null {
  for (const entry of pages) {
    const tab = entry.open && entry.tab ? tabs.get(entry.tab) : undefined;
    if (tab?.icon) return tab.icon;
  }
  return null;
}

/** One search as Home draws it, or null when there is nothing to draw for it. */
function entry(
  search: HomeSearchV1_Serialize,
  map: MapState | null,
  local: Local,
  now: number,
): Entry | null {
  const id = fitId(search.id);
  if (!id) return null;
  const { at } = when(search.at_ms, now);
  const pages = search.pages.slice(0, MAX_PAGES);
  const mark = icon(pages, local.tabs);
  if (search.kind === "address") {
    const first = pages[0];
    const host = label(first?.host || search.text);
    if (!host) return null;
    const shown = first ? page(first, 0, local.tabs) : null;
    const title = shown?.title ?? "";
    return {
      kind: "address",
      id,
      at,
      host,
      title: title === host ? "" : title,
      icon: mark,
      tab: shown?.tab ?? null,
    } satisfies Address;
  }
  const asked = {
    id,
    text: clean(search.text),
    at,
    engine: search.engine ?? null,
    icon: mark,
    also: (map?.first?.also ?? []).slice(0, MAX_ALSO).map((also, index) => ({
      id: `${index}`,
      title: clean(also.title),
      host: label(also.host),
      kind: also.kind,
    })),
  };
  if (!asked.text) return null;
  if (search.kind === "place" && map)
    return { ...asked, kind: "place", place: place(search, map, 0, local, pages) };
  if (search.kind === "compare" && map) {
    const places = [
      place(search, map, 0, local, pages),
      place(search, map, 1, local, pages),
    ] as const;
    const left = new Set(places[0].rows.map((one) => one.id));
    const right = new Set(places[1].rows.map((one) => one.id));
    const pairs = (map.compare ?? [])
      .map((pair) => ({
        left: pair.left && left.has(pair.left) ? pair.left : null,
        right: pair.right && right.has(pair.right) ? pair.right : null,
      }))
      .filter((pair) => pair.left !== null || pair.right !== null)
      .slice(0, MAX_ROWS);
    return { ...asked, kind: "compare", places, pairs };
  }
  return {
    ...asked,
    kind: "pages",
    pages: pages.map((one, index) => page(one, index, local.tabs)),
  };
}

/** Home's model from native searches, newest first, grouped by the day each was asked. */
export function model(
  searches: readonly HomeSearchV1_Serialize[],
  maps: ReadonlyMap<string, MapState>,
  local: Local,
  now: number,
): HomeModel {
  const today = when(now, now).day;
  const days = new Map<string, Entry[]>();
  for (const search of searches.slice(0, MAX_SEARCHES)) {
    const drawn = entry(search, search.map ? (maps.get(search.id) ?? null) : null, local, now);
    if (!drawn) continue;
    const day = when(search.at_ms, now).day;
    days.set(day, [...(days.get(day) ?? []), drawn]);
  }
  const keys = [...days.keys()]
    .filter((key) => key !== today)
    .sort()
    .reverse();
  return {
    today: { key: today, entries: days.get(today) ?? [] },
    earlier: keys.map((key): Day => ({ key, entries: days.get(key)! })),
  };
}

/** Home's model straight from a snapshot, with what the frame holds beside it. */
export function fromSnapshot(
  snapshot: HomeSnapshotV1_Serialize,
  local: Partial<Local> = {},
  now = Date.now(),
): HomeModel {
  const maps = new Map<string, MapState>();
  for (const search of snapshot.searches)
    if (search.map) maps.set(search.id, replay(search.map.changes, search.map.live));
  return model(
    snapshot.searches,
    maps,
    {
      unfolded: local.unfolded ?? new Set(),
      waiting: local.waiting ?? new Set(),
      refused: local.refused ?? new Map(),
      tabs: local.tabs ?? new Map(),
    },
    now,
  );
}
