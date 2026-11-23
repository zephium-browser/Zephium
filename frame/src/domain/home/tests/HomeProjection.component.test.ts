import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync } from "svelte";
import type { HomeInsideRefusalV1, OperationReason } from "$shared/ipc/bindings";
import { entry, first, placed, snapshot, wirePlace, wireSearch } from "$shared/testing/home-wire";

const harness = vi.hoisted(() => ({
  settled: { outcome: "applied", disposition: null } as unknown,
  inside: { accepted: true, refusal: null } as { accepted: boolean; refusal: unknown },
  snapshot: null as unknown,
  opened: [] as unknown[][],
  map: null as null | ((event: { payload: unknown }) => void),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    homeSnapshot: vi.fn(async () => harness.snapshot as never),
    homeOpenTarget: vi.fn(async (...args: unknown[]) => {
      harness.opened.push(args);
      return { accepted: true } as never;
    }),
    homeOpenInside: vi.fn(async () => harness.inside as never),
    homeOpenResults: vi.fn(async () => ({ accepted: true }) as never),
    homeFocusSearch: vi.fn(async () => ({ accepted: true }) as never),
    homeForgetSearch: vi.fn(async () => ({ accepted: true }) as never),
  });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    homeMapChanged: {
      listen: vi.fn(async (listener: (event: { payload: unknown }) => void) => {
        harness.map = listener;
        return () => {};
      }),
    },
    homeChanged: { listen: vi.fn(async () => () => {}) },
  },
}));
vi.mock("$domain/operations", () => ({ settle: vi.fn(async () => harness.settled) }));
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "profile-a" }), activeSpaceId: () => "space-a", tabs: () => [] },
}));

const { home } = await import("../index");

const NOW = Date.now();
const vercel = wireSearch({
  id: "q-vercel",
  text: "vercel",
  at: NOW,
  live: true,
  changes: [
    first(1, { title: "Vercel", host: "vercel.com" }),
    placed(
      2,
      wirePlace({
        title: "Vercel",
        host: "vercel.com",
        rows: [entry("docs", "Docs", { open: true }), entry("blog", "Blog")],
      }),
      0,
      false,
    ),
  ],
});
const flush = async () => {
  for (let i = 0; i < 4; i++) await new Promise((done) => setTimeout(done, 0));
  flushSync();
};
const rejected = (reason: OperationReason) => ({
  outcome: "rejected",
  disposition: { reason } as never,
});
const insideOf = () => {
  const search = home.model().today.entries[0];
  return search?.kind === "place" ? search.place.rows[0]!.inside : undefined;
};

describe("Home's projection", () => {
  let release: () => void;
  beforeEach(async () => {
    harness.settled = { outcome: "applied", disposition: null };
    harness.inside = { accepted: true, refusal: null };
    harness.snapshot = snapshot([vercel]);
    harness.opened = [];
    release?.();
    release = home.retain();
    await flush();
  });

  it("reads native's searches for the scope in front", () => {
    expect(home.ready()).toBe(true);
    expect(home.model().today.entries.map((one) => one.id)).toEqual(["q-vercel"]);
  });

  it("opens a row in the slot native placed it, and refuses one it does not hold", async () => {
    expect(await home.open("q-vercel", { kind: "entry", id: "docs" })).toBeNull();
    expect(harness.opened.at(-1)).toEqual(["q-vercel", 0, { kind: "entry", id: "docs" }]);
    expect(await home.open("q-vercel", { kind: "entry", id: "nowhere" })).toBe("unavailable");
    expect(harness.opened).toHaveLength(1);
  });

  it.each([
    [rejected("home_search_unknown"), "gone"],
    [rejected("item_limit_reached"), "busy"],
    [rejected("home_unavailable"), "failed"],
    [{ outcome: "failed", reason: "timeout" }, "failed"],
    [{ outcome: "no_op", disposition: null }, null],
  ] as const)("says why a request did not go through: %j", async (settled, said) => {
    harness.settled = settled;
    expect(await home.openResults("q-vercel")).toBe(said);
    expect(await home.forget("q-vercel")).toBe(said);
    expect(await home.focus("q-vercel")).toBe(said);
  });

  it.each([
    ["map_gone", "gone"],
    ["too_many_waiting", "busy"],
    ["not_openable", "unavailable"],
    ["unknown_entry", "unavailable"],
  ] as const)("shows an inside refused at admission: %s", async (refusal, reason) => {
    harness.inside = { accepted: false, refusal: refusal as HomeInsideRefusalV1 };
    await home.openInside("q-vercel", "docs");
    flushSync();
    expect(insideOf()).toEqual({ state: "refused", reason });
    home.foldInside("q-vercel", "docs");
    flushSync();
    expect(insideOf()).toBeNull();
  });

  it("holds the waiting mark from admission until the stream answers for that entry", async () => {
    await home.openInside("q-vercel", "docs");
    flushSync();
    expect(insideOf()).toEqual({ state: "opening" });
    harness.map?.({
      payload: {
        map: "map-q-vercel",
        search: "q-vercel",
        change: placed(
          3,
          wirePlace({
            title: "Vercel",
            host: "vercel.com",
            rows: [
              entry("docs", "Docs", {
                open: true,
                read: "read",
                children: [entry("d1", "Getting started")],
              }),
            ],
          }),
          0,
          false,
        ),
      },
    });
    flushSync();
    expect(insideOf()).toMatchObject({ state: "open", rows: [{ label: "Getting started" }] });
  });

  it("forgets what it held once nothing draws Home", () => {
    release();
    flushSync();
    expect(home.ready()).toBe(false);
    expect(home.model().today.entries).toEqual([]);
    release = home.retain();
  });
});
