import { describe, expect, it, vi } from "vitest";
import type {
  HomeChangedV1,
  HomeMapChangedV1_Serialize,
  HomeSnapshotV1_Serialize,
} from "$shared/ipc/bindings";
import { entry, first, placed, snapshot, wirePlace, wireSearch } from "$shared/testing/home-wire";
import { HomeFeed, type FeedState } from "../feed";

const place = (rows: number) =>
  wirePlace({
    title: "Vercel",
    host: "vercel.com",
    rows: Array.from({ length: rows }, (_, i) => entry(`r${i}`, `Row ${i}`)),
  });
const vercel = (
  changes = [first(1, { title: "Vercel", host: "vercel.com" }), placed(2, place(1), 0, false)],
) => wireSearch({ id: "q-vercel", text: "vercel", at: 1, live: true, changes });
const change = (seq: number, rows: number, search = "q-vercel", map = "map-q-vercel") =>
  ({
    map,
    search,
    change: placed(seq, place(rows), 0, false),
  }) satisfies HomeMapChangedV1_Serialize;

function harness() {
  let onMap: (change: HomeMapChangedV1_Serialize) => void = () => {};
  let onChanged: (changed: HomeChangedV1) => void = () => {};
  const reads: { resolve: (value: HomeSnapshotV1_Serialize | null) => void }[] = [];
  const snapshot = vi.fn(
    () => new Promise<HomeSnapshotV1_Serialize | null>((resolve) => reads.push({ resolve })),
  );
  const states: FeedState[] = [];
  const feed = new HomeFeed(
    {
      snapshot,
      onMap: async (listener) => {
        onMap = listener;
        return () => {};
      },
      onChanged: async (listener) => {
        onChanged = listener;
        return () => {};
      },
    },
    (state) => states.push(state),
  );
  const rows = () => {
    const held = feed.state.maps.get("q-vercel")?.places[0];
    return held?.kind === "place" ? held.place.sections.length : null;
  };
  return {
    feed,
    snapshot,
    reads,
    states,
    rows,
    map: (value: HomeMapChangedV1_Serialize) => onMap(value),
    changed: (value: HomeChangedV1) => onChanged(value),
  };
}

const flush = () => new Promise((done) => setTimeout(done, 0));

describe("Home's feed", () => {
  it("listens before it reads, then folds what came during the read onto the snapshot", async () => {
    const h = harness();
    h.feed.follow("profile-a", "space-a");
    await h.feed.start();
    expect(h.snapshot).toHaveBeenCalledTimes(1);
    h.map(change(2, 5));
    h.map(change(3, 2));
    h.reads[0]!.resolve(snapshot([vercel()]));
    await flush();
    // Seq 2 is already in the snapshot; seq 3 folds on top.
    expect(h.rows()).toBe(2);
    h.map(change(3, 9));
    expect(h.rows()).toBe(2);
    h.map(change(4, 3));
    expect(h.rows()).toBe(3);
  });

  it("reads again when native says this scope changed, and not for another scope", async () => {
    const h = harness();
    h.feed.follow("profile-a", "space-a");
    await h.feed.start();
    h.reads[0]!.resolve(snapshot([vercel()]));
    await flush();
    h.changed({ profile: "profile-a", space: "space-b" });
    expect(h.snapshot).toHaveBeenCalledTimes(1);
    h.changed({ profile: "profile-a", space: "space-a" });
    expect(h.snapshot).toHaveBeenCalledTimes(2);
    h.reads[1]!.resolve(snapshot([]));
    await flush();
    expect(h.feed.state.searches).toEqual([]);
  });

  it("drops what it held when the profile or space changes, and a late read for the old scope", async () => {
    const h = harness();
    h.feed.follow("profile-a", "space-a");
    await h.feed.start();
    h.feed.follow("profile-b", "space-a");
    expect(h.feed.state.scope).toBeNull();
    h.reads[0]!.resolve(snapshot([vercel()]));
    await flush();
    expect(h.feed.state.searches).toEqual([]);
    h.reads[1]!.resolve({ ...snapshot([vercel()]), profile: "profile-b" });
    await flush();
    expect(h.feed.state.scope).toEqual({ profile: "profile-b", space: "space-a" });
    expect(h.feed.state.searches).toHaveLength(1);
  });

  it("refuses a snapshot for another scope, and says when native could not answer", async () => {
    const h = harness();
    h.feed.follow("profile-a", "space-a");
    await h.feed.start();
    h.reads[0]!.resolve({ ...snapshot([vercel()]), space: "space-z" });
    await flush();
    expect(h.feed.state).toMatchObject({ searches: [], unavailable: false });
    h.feed.refresh();
    h.reads[1]!.resolve(null);
    await flush();
    expect(h.feed.state.unavailable).toBe(true);
  });

  it("drops a late change for a forgotten search, or for a map it no longer has", async () => {
    const h = harness();
    h.feed.follow("profile-a", "space-a");
    await h.feed.start();
    h.reads[0]!.resolve(snapshot([vercel()]));
    await flush();
    const before = h.feed.state;
    h.map(change(3, 4, "q-gone", "map-q-gone"));
    h.map(change(3, 4, "q-vercel", "map-other"));
    expect(h.feed.state).toBe(before);
    h.feed.refresh();
    h.reads[1]!.resolve(snapshot([]));
    await flush();
    h.map(change(5, 4));
    expect(h.feed.state.maps.size).toBe(0);
  });

  it("stops listening, and holds nothing, once stopped", async () => {
    const h = harness();
    h.feed.follow("profile-a", "space-a");
    await h.feed.start();
    h.reads[0]!.resolve(snapshot([vercel()]));
    await flush();
    h.feed.stop();
    expect(h.feed.state.searches).toEqual([]);
    h.map(change(3, 4));
    expect(h.feed.state.maps.size).toBe(0);
  });
});
