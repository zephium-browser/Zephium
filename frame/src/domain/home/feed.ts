import type {
  HomeChangedV1,
  HomeMapChangedV1_Serialize,
  HomeSearchV1_Serialize,
  HomeSnapshotV1_Serialize,
} from "$shared/ipc/bindings";
import { fold, replay, type MapState } from "./fold";
import { fitId, MAX_SEARCHES } from "./model";

export type FeedPorts = {
  snapshot: () => Promise<HomeSnapshotV1_Serialize | null>;
  onMap: (listener: (change: HomeMapChangedV1_Serialize) => void) => Promise<() => void>;
  onChanged: (listener: (changed: HomeChangedV1) => void) => Promise<() => void>;
};

export type FeedState = {
  /** The scope read, once native has answered for it. */
  scope: { profile: string; space: string } | null;
  searches: readonly HomeSearchV1_Serialize[];
  maps: ReadonlyMap<string, MapState>;
  /** Native could not be reached, or Home is off for this scope. */
  unavailable: boolean;
};

const EMPTY: FeedState = { scope: null, searches: [], maps: new Map(), unavailable: false };

/**
 * Native Home state for one profile and space: a snapshot, then the stream
 * folded on top of it by `seq`. Listening starts before the first read; what
 * arrives while a read is out waits for it. A change for a search the
 * snapshot does not hold is dropped, and a scope change starts again.
 */
export class HomeFeed {
  state: FeedState = EMPTY;
  private generation = 0;
  private queue: HomeMapChangedV1_Serialize[] | null = null;
  private stops: (() => void)[] = [];
  private active = false;
  private want: { profile: string; space: string } | null = null;

  constructor(
    private readonly ports: FeedPorts,
    private readonly changed: (
      state: FeedState,
      touched: HomeMapChangedV1_Serialize | null,
    ) => void,
  ) {}

  async start() {
    if (this.active) return;
    this.active = true;
    const stops = await Promise.all([
      this.ports.onMap((change) => this.receive(change)),
      this.ports.onChanged((event) => {
        if (this.want && event.profile === this.want.profile && event.space === this.want.space)
          void this.read();
      }),
    ]);
    if (!this.active) {
      for (const stop of stops) stop();
      return;
    }
    this.stops = stops;
    if (this.want) void this.read();
  }

  stop() {
    this.active = false;
    this.generation++;
    for (const stop of this.stops) stop();
    this.stops = [];
    this.queue = null;
    this.want = null;
    this.publish(EMPTY, null);
  }

  /** The scope Home is drawn for. Another one drops what was held and reads anew. */
  follow(profile: string | null, space: string | null) {
    const next = profile && space ? { profile, space } : null;
    if (next?.profile === this.want?.profile && next?.space === this.want?.space) return;
    this.want = next;
    this.generation++;
    this.queue = null;
    this.publish(EMPTY, null);
    if (next && this.active) void this.read();
  }

  refresh() {
    if (this.want && this.active) void this.read();
  }

  private async read() {
    const want = this.want;
    if (!want) return;
    const generation = ++this.generation;
    this.queue ??= [];
    let snapshot: HomeSnapshotV1_Serialize | null;
    try {
      snapshot = await this.ports.snapshot();
    } catch {
      snapshot = null;
    }
    if (generation !== this.generation) return;
    const queued = this.queue ?? [];
    this.queue = null;
    if (!snapshot || snapshot.profile !== want.profile || snapshot.space !== want.space) {
      this.publish({ ...EMPTY, scope: want, unavailable: snapshot === null }, null);
      return;
    }
    const searches = snapshot.searches.filter((search) => fitId(search.id)).slice(0, MAX_SEARCHES);
    const maps = new Map<string, MapState>();
    for (const search of searches) {
      if (!search.map) continue;
      const map = replay(search.map.changes, search.map.live);
      maps.set(search.id, { ...map, seq: Math.max(map.seq, search.map.seq ?? -1) });
    }
    this.publish({ scope: want, searches, maps, unavailable: false }, null);
    for (const change of queued) this.receive(change);
  }

  private receive(change: HomeMapChangedV1_Serialize) {
    if (this.queue) {
      this.queue.push(change);
      return;
    }
    const search = this.state.searches.find((one) => one.id === change.search);
    // A late change for a search no longer held, or for a map it no longer has.
    if (!search?.map || search.map.id !== change.map) return;
    const before = this.state.maps.get(search.id);
    if (!before) return;
    const after = fold(before, change.change);
    if (after === before) return;
    const maps = new Map(this.state.maps);
    maps.set(search.id, after);
    this.publish({ ...this.state, maps }, change);
  }

  private publish(state: FeedState, touched: HomeMapChangedV1_Serialize | null) {
    this.state = state;
    this.changed(state, touched);
  }
}
