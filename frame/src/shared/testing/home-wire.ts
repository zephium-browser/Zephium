import type {
  HomePageV1,
  HomeSearchV1_Serialize,
  HomeSnapshotV1_Serialize,
  MapAlso,
  MapChange_Serialize,
  MapEntry_Serialize,
  MapFailure,
  MapNetwork,
  MapPlace_Serialize,
  MapRole,
  MapRow_Serialize,
  SearchEngine,
  TabView,
} from "$shared/ipc/bindings";

/** Native's Home shapes, as the snapshot and the stream carry them. */
export type Wire = HomeSearchV1_Serialize;

type EntryShape = Partial<Omit<MapEntry_Serialize, "id" | "label">>;

export const entry = (id: string, label: string, shape: EntryShape = {}): MapEntry_Serialize => ({
  id,
  tier: "primary",
  label,
  role: null,
  off_site: false,
  menu_only: false,
  open: false,
  host: null,
  path: "",
  read: null,
  children: [],
  more: false,
  ...shape,
});

export const roled = (id: string, label: string, role: MapRole, shape: EntryShape = {}) =>
  entry(id, label, { role, ...shape });
export const opens = (id: string, label: string, shape: EntryShape = {}) =>
  entry(id, label, { open: true, ...shape });
export const off = (host: string): EntryShape => ({ off_site: true, host });
const tier = (t: MapEntry_Serialize["tier"], prefix: string, labels: readonly string[]) =>
  labels.map((label, index) => entry(`${prefix}${index}`, label, { tier: t }));
export const children = (prefix: string, labels: readonly string[]) =>
  labels.map((label, index) => entry(`${prefix}${index}`, label));
/** A site's menu with no page of its own, and what it already holds. */
export const menu = (id: string, label: string, held: readonly string[]) =>
  entry(id, label, { menu_only: true, children: children(id, held) });

type PlaceShape = {
  title: string;
  host: string;
  focus?: string | null;
  rows: MapEntry_Serialize[];
  more?: readonly string[];
  actions?: readonly string[];
  social?: MapNetwork[];
  thin?: boolean;
};

export function wirePlace(shape: PlaceShape): MapPlace_Serialize {
  return {
    title: shape.title,
    site: `https://${shape.host}/`,
    host: shape.host,
    sections: [
      ...shape.rows,
      ...tier("secondary", "m", shape.more ?? []),
      ...tier("action", "act", shape.actions ?? []),
    ],
    focus: shape.focus ?? null,
    social: shape.social ?? [],
    notes: {
      thin: shape.thin ?? false,
      shell: false,
      wall: false,
      robots_disallowed: false,
      truncated: false,
    },
  };
}

export const also = (title: string, host: string, kind: MapAlso["kind"] = "page"): MapAlso => ({
  title,
  host,
  kind,
});

/** The stream as a map tells it: the results page, then each place, then its end. */
export const first = (
  seq: number,
  place: { title: string; host: string } | null,
  more: MapAlso[] = [],
  social: MapNetwork[] = [],
): MapChange_Serialize => ({
  type: "first_layer",
  seq,
  official: place ? { title: place.title, site: `https://${place.host}/` } : null,
  social,
  organic: more.length,
  also: more,
});

export const placed = (
  seq: number,
  place: MapPlace_Serialize,
  slot = 0,
  complete = true,
): MapChange_Serialize => ({
  type: "place",
  seq,
  slot,
  place,
  stale: false,
  confidence: "high",
  complete,
});

export const ended = (seq: number, failure: MapFailure | null = null): MapChange_Serialize => ({
  type: "end",
  seq,
  outcome: failure ? { kind: "failed", failure } : { kind: "done" },
  elapsed_ms: 2400,
});

export const failedAt = (seq: number, failure: MapFailure, slot = 0): MapChange_Serialize => ({
  type: "place_failed",
  seq,
  slot,
  failure,
});

export const refusedInside = (
  seq: number,
  id: string,
  failure: MapFailure,
): MapChange_Serialize => ({
  type: "opened",
  seq,
  slot: 0,
  id,
  result: { kind: "refused", failure },
});

export const comparison = (seq: number, rows: MapRow_Serialize[]): MapChange_Serialize => ({
  type: "compare",
  seq,
  rows,
});

type SearchShape = {
  id: string;
  text: string;
  at: number;
  kind?: Wire["kind"];
  engine?: SearchEngine;
  changes?: MapChange_Serialize[];
  live?: boolean;
  pages?: HomePageV1[];
};

export function wireSearch(shape: SearchShape): Wire {
  const changes = shape.changes ?? [];
  return {
    id: shape.id,
    text: shape.text,
    at_ms: shape.at,
    kind: shape.kind ?? "place",
    engine: shape.engine ?? "google",
    results_open: false,
    map:
      shape.kind === "other" || shape.kind === "address"
        ? null
        : {
            id: `map-${shape.id}`,
            seq: changes.at(-1)?.seq ?? null,
            live: shape.live ?? false,
            changes,
          },
    pages: shape.pages ?? [],
  };
}

export const opened = (
  entry: string | null,
  title: string,
  host: string,
  tab: string | null = null,
  slot: number | null = 0,
): HomePageV1 => ({ tab, slot: entry ? slot : null, entry, open: tab !== null, title, host });

export const snapshot = (searches: Wire[]): HomeSnapshotV1_Serialize => ({
  profile: "profile-a",
  space: "space-a",
  searches,
});

/** A tab as the tabs projection draws one, for what Home joins to it. */
export const tab = (id: string, title: string, shape: Partial<TabView> = {}): TabView =>
  ({
    id,
    title,
    url: `https://example.org/${id}`,
    loading: false,
    icon: null,
    projection_revision: "1",
    ...shape,
  }) as TabView;

/** A search whose places say each section differently, as a later read of the same site would. */
export function mapEntries(
  search: Wire,
  change: (entry: MapEntry_Serialize) => MapEntry_Serialize,
): Wire {
  if (!search.map) return search;
  return {
    ...search,
    map: {
      ...search.map,
      changes: search.map.changes.map((one) =>
        one.type === "place"
          ? { ...one, place: { ...one.place, sections: one.place.sections.map(change) } }
          : one,
      ),
    },
  };
}
