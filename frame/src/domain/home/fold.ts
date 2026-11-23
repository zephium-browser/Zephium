import type {
  MapAlso,
  MapChange_Serialize,
  MapFailure,
  MapNetwork,
  MapOfficial,
  MapOutcome,
  MapPlace_Serialize,
  MapRow_Serialize,
} from "$shared/ipc/bindings";

/** A map's place in one slot: read so far, or failed. */
type PlaceState =
  | { kind: "place"; place: MapPlace_Serialize; complete: boolean }
  | { kind: "failed"; failure: MapFailure };

/** What a map's changes have said so far, latest of each kind. */
export type MapState = {
  seq: number;
  live: boolean;
  first: { official: MapOfficial | null; social: MapNetwork[]; also: MapAlso[] } | null;
  places: readonly [PlaceState | null, PlaceState | null];
  compare: MapRow_Serialize[] | null;
  end: MapOutcome | null;
  /** Why an inside read was refused, by `slot/entry`. */
  refused: ReadonlyMap<string, MapFailure>;
};

export const EMPTY_MAP: MapState = {
  seq: -1,
  live: true,
  first: null,
  places: [null, null],
  compare: null,
  end: null,
  refused: new Map(),
};

const slotOf = (slot: number) => (slot === 0 || slot === 1 ? slot : null);

/**
 * One change folded into a map. A change at or below what is held is already
 * in it and leaves the map as it was; so does one naming a slot a map does not have.
 */
export function fold(map: MapState, change: MapChange_Serialize): MapState {
  if (!Number.isSafeInteger(change.seq) || change.seq <= map.seq) return map;
  const next = { ...map, seq: change.seq };
  switch (change.type) {
    case "first_layer":
      return {
        ...next,
        first: { official: change.official, social: change.social, also: change.also },
      };
    case "place":
    case "place_failed": {
      const slot = slotOf(change.slot);
      if (slot === null) return next;
      const places: [PlaceState | null, PlaceState | null] = [...map.places];
      places[slot] =
        change.type === "place"
          ? { kind: "place", place: change.place, complete: change.complete }
          : { kind: "failed", failure: change.failure };
      return { ...next, places };
    }
    case "compare":
      return { ...next, compare: change.rows };
    case "end":
      return { ...next, end: change.outcome };
    case "opened": {
      if (change.result.kind !== "refused") return next;
      const refused = new Map(map.refused);
      refused.set(`${change.slot}/${change.id}`, change.result.failure);
      return { ...next, refused };
    }
    default:
      return next;
  }
}

/** A snapshot's map: its changes replayed in order, held at the seq it names. */
export function replay(changes: readonly MapChange_Serialize[], live: boolean): MapState {
  let map: MapState = { ...EMPTY_MAP, live };
  for (const change of changes) map = fold(map, change);
  return map;
}
