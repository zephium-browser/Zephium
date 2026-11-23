import { describe, expect, it } from "vitest";
import { overlaps } from "$shared/canvas";
import {
  layout,
  PLACE_X,
  SEARCH_X,
  SIZE,
  linkWidth,
  type HomeNode,
  type Laid,
} from "../lib/layout";
import type { Entry, HomeModel } from "$domain/home";
import { children, mapEntries } from "$shared/testing/home-wire";
import * as fixture from "./fixtures";

const entries = (model: HomeModel) =>
  [model.today, ...model.earlier].flatMap((day): readonly Entry[] => day.entries);
const searches = (model: HomeModel) => entries(model).filter((entry) => entry.kind !== "address");
const laid = (
  model: HomeModel,
  view: {
    expanded?: ReadonlySet<string>;
    front?: string | null;
    shown?: ReadonlySet<string>;
    all?: ReadonlySet<string>;
    marked?: { search: string; entry: string } | null;
  } = {},
) => {
  const front = view.front === undefined ? (searches(model)[0]?.id ?? null) : view.front;
  return layout([model.today, ...model.earlier], model.today.key, {
    expanded: view.expanded ?? new Set(),
    shown: view.shown ?? new Set(),
    open: front ? [front] : [],
    lead: front,
    all: view.all ?? new Set(),
    marked: view.marked ?? null,
  });
};
const solid = (canvas: Laid) => canvas.nodes.filter((node) => node.kind !== "lines");
const find = <K extends HomeNode["kind"]>(canvas: Laid, kind: K, id?: string) =>
  canvas.nodes.find(
    (node): node is Extract<HomeNode, { kind: K }> =>
      node.kind === kind && (id === undefined || node.id === id),
  )!;

/** Every row that opens shows its inside, read, waiting or refused as asked. */
const unfoldAll = (wire: fixture.Wire, state: "read" | "waiting" | "refused") => {
  const changed = mapEntries(wire, (one) =>
    one.open
      ? {
          ...one,
          read: state === "read" ? "read" : state === "refused" ? "refused" : null,
          children: state === "read" ? children(`${one.id}-`, ["One", "Two"]) : one.children,
        }
      : one,
  );
  const keys = (changed.map?.changes ?? []).flatMap((change) =>
    change.type === "place"
      ? change.place.sections.filter((one) => one.open).map((one) => `${wire.id}/${one.id}`)
      : [],
  );
  return {
    wire: changed,
    unfolded: keys,
    waiting: state === "waiting" ? keys : [],
  };
};
const withInsides = (...parts: ReturnType<typeof unfoldAll>[]) =>
  fixture.home(
    parts.map((part) => part.wire),
    {
      unfolded: new Set(parts.flatMap((part) => part.unfolded)),
      waiting: new Set(parts.flatMap((part) => part.waiting)),
    },
  );
const MODELS: Record<string, () => HomeModel> = {
  today: fixture.today,
  states: fixture.states,
  edges: fixture.edges,
  comparing: fixture.comparing,
  busy: fixture.busy,
  small: () => fixture.home([fixture.raycast()]),
  large: fixture.largeOpen,
  menu: () => fixture.home([fixture.stripe()]),
  pages: () => fixture.home([fixture.grinders()]),
  failed: () => fixture.home([fixture.failed()]),
  thin: fixture.thinPlaces,
  rtl: fixture.rtlFirst,
  arriving: () => fixture.home([fixture.arriving(1), { ...fixture.arriving(7), id: "q-vercel-2" }]),
  "three insides": () =>
    withInsides(
      unfoldAll(fixture.figma(), "waiting"),
      unfoldAll({ ...fixture.large(), id: "q-stripe-1" }, "waiting"),
      unfoldAll({ ...fixture.large(), id: "q-stripe-2" }, "refused"),
    ),
  "compare with insides": () => {
    const both = unfoldAll(fixture.compare(0), "read");
    return fixture.home([both.wire], { unfolded: new Set(both.unfolded) });
  },
};

describe("Home's layout", () => {
  it.each(Object.keys(MODELS))("places nothing over anything else: %s", (name) => {
    const model = MODELS[name]!();
    const keys = entries(model).flatMap((entry) =>
      entry.kind === "place"
        ? [`${entry.id}/${entry.place.id}`]
        : entry.kind === "compare"
          ? entry.places.map((place) => `${entry.id}/${place.id}`)
          : [],
    );
    const menus = searches(model).flatMap((search) =>
      (search.kind === "place" ? search.place.rows : [])
        .filter((row) => row.menu)
        .map((row) => `${search.id}/${row.id}`),
    );
    for (const front of [null, ...searches(model).map((search) => search.id)])
      for (const expanded of [false, true]) {
        const nodes = solid(
          laid(model, {
            front,
            expanded: new Set(expanded ? keys : []),
            shown: new Set(expanded ? menus : []),
          }),
        );
        for (const [index, a] of nodes.entries())
          for (const b of nodes.slice(index + 1))
            expect(overlaps(a.box, b.box, 8), `${a.id} and ${b.id} (${front})`).toBe(false);
      }
  });

  it("is the same canvas every time for the same model", () => {
    for (const make of Object.values(MODELS)) {
      const once = laid(make());
      expect(JSON.stringify(laid(make()))).toBe(JSON.stringify(once));
    }
  });

  it("keeps every stop inside what is drawn", () => {
    for (const make of Object.values(MODELS))
      for (const front of [undefined, ...searches(make()).map((search) => search.id)]) {
        const canvas = laid(make(), { front });
        const nodes = solid(canvas);
        for (const stop of canvas.stops)
          expect(
            nodes.some(
              ({ box }) =>
                stop.rect.x >= box.x &&
                stop.rect.y >= box.y &&
                stop.rect.x + stop.rect.width <= box.x + box.width &&
                stop.rect.y + stop.rect.height <= box.y + box.height,
            ),
            stop.id,
          ).toBe(true);
        expect(new Set(canvas.stops.map((stop) => stop.id)).size).toBe(canvas.stops.length);
      }
  });

  it("draws only the search in front open, and every other entry as one line", () => {
    const model = fixture.busy();
    for (const front of [null, "q-grinders", "y-2", "q-payments"]) {
      const canvas = laid(model, { front });
      const open = canvas.nodes.filter((node) => node.kind === "search");
      expect(open.map((node) => node.id)).toEqual(front ? [`s:${front}`] : []);
      const lines = canvas.nodes.filter(
        (node) => node.kind === "compact" || node.kind === "address",
      );
      expect(lines).toHaveLength(entries(model).length - (front ? 1 : 0));
      for (const line of lines) expect(line.box.height).toBe(SIZE.line);
      expect(Object.keys(canvas.lanes)).toHaveLength(entries(model).length);
    }
    // A line keeps its search's stop, so the keyboard stays on it as it opens.
    const shut = laid(model).stops.find((stop) => stop.id === "s:q-grinders");
    expect(shut?.activate).toEqual({ kind: "open", search: "q-grinders" });
  });

  it("keeps a full day of searches short enough to scan", () => {
    const canvas = laid(fixture.busy());
    expect(find(canvas, "day", "d:2026-11-22").box.y).toBeLessThan(2400);
    const lines = canvas.nodes.filter((node) => node.kind === "compact" || node.kind === "address");
    const today = lines.filter((line) => /^[ct]:t-/u.test(line.id));
    for (const [index, line] of today.entries())
      if (index) expect(line.box.y - today[index - 1]!.box.y).toBe(SIZE.line + 8);
  });

  it("puts the newest search under the field, older ones below, and earlier days below today", () => {
    const canvas = laid(fixture.busy());
    const drawn = canvas.nodes.filter(
      (node) => node.kind === "search" || node.kind === "compact" || node.kind === "address",
    );
    expect(drawn[0]!.box.y).toBe(0);
    for (const [index, node] of drawn.entries())
      if (index) expect(node.box.y).toBeGreaterThan(drawn[index - 1]!.box.y);
    const days = canvas.nodes.filter((node) => node.kind === "day");
    expect(days.map((node) => node.id)).toEqual(["d:2026-11-22", "d:2026-11-21"]);
    const lastToday = canvas.nodes.findIndex((node) => node.id === "d:2026-11-22");
    const todayBottom = Math.max(
      ...canvas.nodes.slice(0, lastToday).map((node) => node.box.y + node.box.height),
    );
    expect(days[0]!.box.y).toBeGreaterThan(todayBottom);
    expect(days.at(-1)!.box.height).toBe(SIZE.day);
  });

  it("starts the keyboard on the row the search in front asked for", () => {
    expect(laid(fixture.today()).first).toBe("p:q-payments/0/pricing");
    expect(laid(fixture.home([fixture.grinders()])).first).toBe("s:q-grinders");
    expect(laid(fixture.today(), { front: "q-raycast" }).first).toBe("p:q-raycast/0/pro");
    expect(laid(fixture.thinPlaces()).first).toBe("p:q-studio/0/s-visit");
    expect(laid(fixture.busy(), { front: null }).first).toBe("s:q-payments");
  });

  it("draws an inside to the right of its row, level with it", () => {
    const canvas = laid(fixture.largeOpen());
    const card = find(canvas, "place").box;
    const inside = find(canvas, "inside").box;
    const row = canvas.stops.find((stop) => stop.id === "p:q-stripe/0/documentation")!.rect;
    expect(inside.x).toBeGreaterThan(card.x + card.width);
    expect(
      Math.abs(inside.y + SIZE.pad + SIZE.insideHeader / 2 - (row.y + row.height / 2)),
    ).toBeLessThan(1);
  });

  it("stacks insides of neighbouring rows rather than over each other", () => {
    const canvas = laid(withInsides(unfoldAll(fixture.large(), "read")));
    const insides = canvas.nodes.filter((node) => node.kind === "inside");
    expect(insides.length).toBeGreaterThan(5);
    for (const [index, node] of insides.entries())
      if (index)
        expect(node.box.y).toBeGreaterThanOrEqual(
          insides[index - 1]!.box.y + insides[index - 1]!.box.height,
        );
  });

  it("lines up the rows two places share at the top, and lists the rest after with no gaps", () => {
    const canvas = laid(fixture.comparing());
    const [a, b] = canvas.nodes.filter((node) => node.kind === "place");
    if (a?.kind !== "place" || b?.kind !== "place") throw new Error("two places");
    expect(a.box.y).toBe(b.box.y);
    expect(a.rows.map((row) => row.id)).toEqual([
      "n-pricing",
      "n-download",
      "n-templates",
      "n-help",
      "n-blog",
      "n-product",
      "n-enterprise",
    ]);
    expect(b.rows.map((row) => row.id)).toEqual([
      "o-pricing",
      "o-download",
      "o-plugins",
      "o-help",
      "o-blog",
      "o-sync",
      "o-publish",
    ]);
    expect([a.split, b.split]).toEqual([5, 5]);
    const y = (id: string) => canvas.stops.find((stop) => stop.id === id)!.rect.y;
    for (const [left, right] of [
      ["n-pricing", "o-pricing"],
      ["n-help", "o-help"],
      ["n-blog", "o-blog"],
    ])
      expect(y(`p:q-compare/0/${left}`)).toBe(y(`p:q-compare/1/${right}`));
    expect(y("p:q-compare/0/n-product") - y("p:q-compare/0/n-blog")).toBe(SIZE.row + SIZE.divide);
    expect(y("p:q-compare/0/n-enterprise") - y("p:q-compare/0/n-product")).toBe(SIZE.row);
  });

  it("opens a comparison's inside between its two cards", () => {
    const canvas = laid(MODELS["compare with insides"]!());
    const [a, b] = canvas.nodes.filter((node) => node.kind === "place");
    const insides = canvas.nodes.filter((node) => node.kind === "inside");
    expect(
      insides.some((node) => node.box.x > a!.box.x + a!.box.width && node.box.x < b!.box.x),
    ).toBe(true);
    expect(insides.some((node) => node.box.x > b!.box.x + b!.box.width)).toBe(true);
  });

  it("keeps the results' other pages under the search and the profiles under the place", () => {
    const canvas = laid(fixture.home([fixture.large()]));
    const search = find(canvas, "search").box;
    const card = find(canvas, "place").box;
    expect(find(canvas, "also").box).toMatchObject({ x: SEARCH_X });
    expect(find(canvas, "also").box.y).toBeGreaterThan(search.y + search.height);
    expect(find(canvas, "social").box).toMatchObject({ x: PLACE_X });
    expect(find(canvas, "social").box.y).toBeGreaterThan(card.y + card.height);
  });

  it("branches each opened page from its search", () => {
    const canvas = laid(fixture.home([fixture.grinders()]));
    expect(canvas.nodes.filter((node) => node.kind === "page")).toHaveLength(3);
    expect(find(canvas, "lines").lines).toHaveLength(3);
  });

  it("draws a reading inside's line as working and a refused one as refused", () => {
    const canvas = laid(fixture.home([fixture.figma()], fixture.figmaLocal));
    expect(
      find(canvas, "lines")
        .lines.map((line) => line.state)
        .sort(),
    ).toEqual(["drawn", "opening", "refused"]);
  });

  it("gives a refused inside one stop that opens the page itself", () => {
    const canvas = laid(fixture.home([fixture.figma()], fixture.figmaLocal));
    expect(canvas.stops.find((stop) => stop.id === "i:q-figma/dev")).toMatchObject({
      activate: { kind: "row", row: "dev" },
      parent: "p:q-figma/0/dev",
    });
  });

  it("shows what a menu row holds only when it is opened here, and never as being read", () => {
    const shut = laid(fixture.today());
    expect(shut.nodes.some((node) => node.kind === "inside")).toBe(false);
    const row = shut.stops.find((stop) => stop.id === "p:q-payments/0/developers")!;
    expect(row.activate).toEqual({ kind: "menu", search: "q-payments", row: "developers" });
    expect(row.row).toMatchObject({ menu: true, inside: null });
    const open = laid(fixture.today(), { shown: new Set(["q-payments/developers"]) });
    const inside = find(open, "inside", "i:q-payments/developers");
    expect(inside.inside).toMatchObject({ state: "open", more: true });
    expect(find(open, "lines").lines.map((line) => line.state)).toEqual(["drawn", "drawn"]);
    expect(open.stops.find((stop) => stop.id === "i:q-payments/developers/more")?.activate).toEqual(
      { kind: "menuAll", search: "q-payments", row: "developers" },
    );
    expect(open.stops.find((stop) => stop.id === "p:q-payments/0/developers")?.row?.first).toBe(
      "i:q-payments/developers/developers0",
    );
  });

  it("draws a place as one line only when the engine says it is thin", () => {
    const thinned = (wire: fixture.Wire, thin: boolean): fixture.Wire => ({
      ...wire,
      map: wire.map && {
        ...wire.map,
        changes: wire.map.changes.map((change) =>
          change.type === "place"
            ? { ...change, place: { ...change.place, notes: { ...change.place.notes, thin } } }
            : change,
        ),
      },
    });
    expect(find(laid(fixture.home([thinned(fixture.studio(), false)])), "place").rows).toHaveLength(
      2,
    );
    // A thin place is one line from its first paint, while the rest of it still arrives.
    const filling = laid(fixture.home([thinned(fixture.arriving(1), true)]));
    expect(filling.nodes.some((node) => node.kind === "place")).toBe(false);
    expect(find(filling, "thin").links.map((link) => link.id)).toEqual(["v-products"]);
  });

  it("draws a place the engine calls thin as one line with its links", () => {
    const canvas = laid(fixture.thinPlaces());
    const line = find(canvas, "thin", "p:q-studio/0");
    expect(line.box.height).toBe(SIZE.thin);
    expect(canvas.nodes.some((node) => node.kind === "place")).toBe(false);
    expect(line.links.map((link) => [link.kind, link.id])).toEqual([
      ["row", "s-classes"],
      ["row", "s-visit"],
      ["action", "act0"],
    ]);
    expect(canvas.stops.map((stop) => stop.id)).toEqual(
      expect.arrayContaining(["p:q-studio/0/s-classes", "p:q-studio/0/action:act0"]),
    );
    expect(find(canvas, "social").box.y).toBeGreaterThan(line.box.y + line.box.height);
    const alone = find(laid(fixture.thinPlaces(), { front: "q-bare" }), "thin");
    expect(alone.links).toEqual([]);
    // Its width follows the place column, so a line under a card lines up with it.
    expect(alone.box.width).toBe(SIZE.card);
  });

  it("keeps four actions with long labels on one line inside the card", () => {
    const canvas = laid(fixture.edges());
    const card = find(canvas, "place").box;
    const actions = canvas.stops.filter((stop) => stop.id.includes("/action:"));
    expect(actions).toHaveLength(4);
    for (const [index, stop] of actions.entries()) {
      expect(stop.rect.y).toBe(actions[0]!.rect.y);
      expect(stop.rect.width).toBeGreaterThan(40);
      if (index)
        expect(stop.rect.x).toBeGreaterThan(
          actions[index - 1]!.rect.x + actions[index - 1]!.rect.width,
        );
    }
    const last = actions.at(-1)!.rect;
    expect(last.x + last.width).toBeLessThanOrEqual(card.x + card.width - SIZE.pad);
    expect(card.y + card.height - (actions[0]!.rect.y + SIZE.section)).toBeLessThan(SIZE.foot);
    // A short label keeps its own width; the long ones give way.
    const short = laid(fixture.home([fixture.raycast()])).stops.filter((stop) =>
      stop.id.includes("/action:"),
    );
    expect(short[0]!.rect.width).toBe(linkWidth("Download"));
  });

  it("gives a result's long title two lines and a short one a single line", () => {
    expect(find(laid(fixture.thinPlaces()), "also").heights).toEqual([SIZE.alsoTwo, SIZE.also]);
  });

  it("draws a typed address as wide as a search's line", () => {
    const canvas = laid(fixture.today());
    const address = find(canvas, "address");
    expect(address.box.width).toBe(find(canvas, "compact").box.width);
    expect(address.box.width).toBeGreaterThan(SIZE.search.width + SIZE.card);
  });

  it("grows a place by its known sections only when they are shown", () => {
    const shut = find(laid(fixture.home([fixture.large()])), "place").box.height;
    const open = find(
      laid(fixture.home([fixture.large()]), { expanded: new Set(["q-stripe/0"]) }),
      "place",
    ).box.height;
    expect(open - shut).toBe(10 * SIZE.section + 4);
  });

  it("lists four of a search's other results, and the rest behind one more", () => {
    const shut = find(laid(fixture.today()), "also");
    expect(shut.heights).toHaveLength(4);
    expect(shut.hidden).toBe(4);
    const more = laid(fixture.today()).stops.find((stop) => stop.id === "a:q-payments/more");
    expect(more?.activate).toEqual({ kind: "also", search: "q-payments" });
    const all = find(laid(fixture.today(), { all: new Set(["q-payments"]) }), "also");
    expect([all.heights.length, all.hidden]).toEqual([8, 0]);
    expect(all.box.height).toBeGreaterThan(shut.box.height);
  });

  it("shows a menu's first rows and the rest it holds behind its more", () => {
    const shown = new Set(["q-payments/products"]);
    const some = find(laid(fixture.today(), { shown }), "inside", "i:q-payments/products");
    expect(some.inside.state === "open" && some.inside.rows).toHaveLength(7);
    expect(some.rest).toBe(1);
    const whole = find(
      laid(fixture.today(), { shown: new Set([...shown, "q-payments/products/all"]) }),
      "inside",
      "i:q-payments/products",
    );
    expect(whole.inside.state === "open" && [whole.inside.rows.length, whole.inside.more]).toEqual([
      8,
      false,
    ]);
  });

  it("starts the keyboard on the row a page was opened from when its search is shown again", () => {
    const marked = { search: "q-raycast", entry: "teams" };
    expect(laid(fixture.today(), { front: "q-raycast", marked }).first).toBe("p:q-raycast/0/teams");
    const child = { search: "q-stripe", entry: "docs3" };
    expect(laid(fixture.largeOpen(), { marked: child }).first).toBe(
      "i:q-stripe/documentation/docs3",
    );
  });

  it("measures a result's title where that decides whether it takes two lines", () => {
    const wide = (text: string, size: number) => text.length * size;
    const narrow = () => 10;
    const canvas = (measure: (text: string, size: number) => number) =>
      layout([fixture.today().today], fixture.today().today.key, {
        expanded: new Set(),
        shown: new Set(),
        open: ["q-payments"],
        lead: "q-payments",
        all: new Set(),
        marked: null,
        measure,
      });
    expect(find(canvas(wide), "also").heights.every((h) => h === SIZE.alsoTwo)).toBe(true);
    expect(find(canvas(narrow), "also").heights.every((h) => h === SIZE.also)).toBe(true);
  });

  it("gives a refused inside whose map has gone a way to ask again", () => {
    const model = fixture.home([fixture.figma()], {
      unfolded: new Set(["q-figma/design"]),
      refused: new Map([["q-figma/design", "gone" as const]]),
    });
    expect(laid(model).stops.find((stop) => stop.id === "i:q-figma/design")?.activate).toEqual({
      kind: "again",
      search: "q-figma",
    });
  });
});
