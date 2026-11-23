import { describe, expect, it } from "vitest";
import { overlaps } from "$shared/canvas";
import { layout, PLACE_X, SEARCH_X, SIZE, type HomeNode, type Laid } from "../lib/layout";
import { admit, type HomeModel, type Inside, type Row } from "../lib/model";
import * as fixture from "./fixtures";

const none = { expanded: new Set<string>() };
const laid = (model: HomeModel, view = none) => {
  const home = admit(model);
  return layout([home.today, ...home.earlier], home.today.key, view);
};
const solid = (canvas: Laid) => canvas.nodes.filter((node) => node.kind !== "lines");
const find = <K extends HomeNode["kind"]>(canvas: Laid, kind: K, id?: string) =>
  canvas.nodes.find(
    (node): node is Extract<HomeNode, { kind: K }> =>
      node.kind === kind && (id === undefined || node.id === id),
  )!;

const opening: Inside = { state: "opening" };
const refused: Inside = { state: "refused", reason: "unavailable" };
const MODELS: Record<string, () => HomeModel> = {
  today: fixture.today,
  states: fixture.states,
  edges: fixture.edges,
  comparing: fixture.comparing,
  busy: fixture.busy,
  small: () => fixture.home([fixture.raycast]),
  large: () => fixture.home([fixture.stripe()]),
  pages: () => fixture.home([fixture.grinders]),
  failed: () => fixture.home([fixture.failed]),
  arriving: () => fixture.home([fixture.arriving(1), { ...fixture.arriving(7), id: "q-vercel-2" }]),
  "three insides": () =>
    fixture.home([
      fixture.figma({ dev: opening, plugins: refused }),
      { ...fixture.stripe(opening), id: "q-stripe-1" },
      { ...fixture.stripe(refused), id: "q-stripe-2" },
    ]),
  "compare with insides": () => {
    const [a, b] = fixture.compare.places;
    const open = (inside: Inside) => (row: Row) => (row.opens ? { ...row, inside } : row);
    const read: Inside = { state: "open", more: true, rows: [fixture.row("x", "One")] };
    return fixture.home([
      {
        ...fixture.compare,
        places: [
          { ...a, rows: a.rows.map(open(read)) },
          { ...b, rows: b.rows.map(open(refused)) },
        ],
      },
    ]);
  },
};

describe("Home's layout", () => {
  it.each(Object.keys(MODELS))("places nothing over anything else: %s", (name) => {
    for (const expanded of [false, true]) {
      const model = MODELS[name]!();
      const keys = [model.today, ...model.earlier]
        .flatMap((day) => day.entries ?? [])
        .flatMap((entry) =>
          entry.kind === "place"
            ? [`${entry.id}/${entry.place.id}`]
            : entry.kind === "compare"
              ? entry.places.map((place) => `${entry.id}/${place.id}`)
              : [],
        );
      const nodes = solid(laid(model, { expanded: new Set(expanded ? keys : []) }));
      for (const [index, a] of nodes.entries())
        for (const b of nodes.slice(index + 1))
          expect(overlaps(a.box, b.box, 8), `${a.id} and ${b.id}`).toBe(false);
    }
  });

  it("is the same canvas every time for the same model", () => {
    for (const make of Object.values(MODELS)) {
      const once = laid(make());
      expect(JSON.stringify(laid(make()))).toBe(JSON.stringify(once));
    }
  });

  it("keeps every stop inside what is drawn", () => {
    for (const make of Object.values(MODELS)) {
      const canvas = laid(make());
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

  it("puts the newest search under the field, older ones below, and earlier days below today", () => {
    const canvas = laid(fixture.busy());
    const searches = canvas.nodes.filter(
      (node) => node.kind === "search" || node.kind === "address",
    );
    expect(searches[0]!.box.y).toBe(0);
    for (const [index, node] of searches.entries())
      if (index) expect(node.box.y).toBeGreaterThan(searches[index - 1]!.box.y);
    const days = canvas.nodes.filter((node) => node.kind === "day");
    expect(days.map((node) => node.id)).toEqual(["d:2026-11-22", "d:2026-11-21", "d:2026-11-20"]);
    const lastToday = canvas.nodes.findIndex((node) => node.id === "d:2026-11-22");
    const todayBottom = Math.max(
      ...canvas.nodes.slice(0, lastToday).map((node) => node.box.y + node.box.height),
    );
    expect(days[0]!.box.y).toBeGreaterThan(todayBottom);
    expect(days.at(-1)!.box.height).toBe(SIZE.day + SIZE.unread);
  });

  it("starts the keyboard on the row the newest search asked for", () => {
    expect(laid(fixture.today()).first).toBe("p:q-stripe/stripe.com/pricing");
    expect(laid(fixture.home([fixture.grinders])).first).toBe("s:q-grinders");
  });

  it("draws an inside to the right of its row, level with it", () => {
    const canvas = laid(fixture.home([fixture.stripe()]));
    const card = find(canvas, "place").box;
    const inside = find(canvas, "inside").box;
    const row = canvas.stops.find(
      (stop) => stop.id === "p:q-stripe/stripe.com/documentation",
    )!.rect;
    expect(inside.x).toBeGreaterThan(card.x + card.width);
    expect(
      Math.abs(inside.y + SIZE.pad + SIZE.insideHeader / 2 - (row.y + row.height / 2)),
    ).toBeLessThan(1);
  });

  it("stacks insides of neighbouring rows rather than over each other", () => {
    const place = fixture.stripe(null);
    const open: Inside = {
      state: "open",
      more: false,
      rows: [fixture.row("a", "A"), fixture.row("b", "B")],
    };
    const rows = place.place.rows.map((row) => (row.opens ? { ...row, inside: open } : row));
    const canvas = laid(fixture.home([{ ...place, place: { ...place.place, rows } }]));
    const insides = canvas.nodes.filter((node) => node.kind === "inside");
    expect(insides.length).toBeGreaterThan(5);
    for (const [index, node] of insides.entries())
      if (index)
        expect(node.box.y).toBeGreaterThanOrEqual(
          insides[index - 1]!.box.y + insides[index - 1]!.box.height,
        );
  });

  it("lines up the paired rows of a comparison", () => {
    const canvas = laid(fixture.comparing());
    const [a, b] = canvas.nodes.filter((node) => node.kind === "place");
    expect(a!.box.y).toBe(b!.box.y);
    const y = (id: string) => canvas.stops.find((stop) => stop.id === id)!.rect.y;
    expect(y("p:q-compare/notion.com/n-pricing")).toBe(y("p:q-compare/obsidian.md/o-pricing"));
    expect(y("p:q-compare/notion.com/n-blog")).toBe(y("p:q-compare/obsidian.md/o-blog"));
    expect(y("p:q-compare/obsidian.md/o-sync")).toBeGreaterThan(
      y("p:q-compare/notion.com/n-product"),
    );
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
    const canvas = laid(fixture.home([fixture.stripe()]));
    const search = find(canvas, "search").box;
    const card = find(canvas, "place").box;
    expect(find(canvas, "also").box).toMatchObject({ x: SEARCH_X });
    expect(find(canvas, "also").box.y).toBeGreaterThan(search.y + search.height);
    expect(find(canvas, "social").box).toMatchObject({ x: PLACE_X });
    expect(find(canvas, "social").box.y).toBeGreaterThan(card.y + card.height);
  });

  it("branches each opened page from its search", () => {
    const canvas = laid(fixture.home([fixture.grinders]));
    expect(canvas.nodes.filter((node) => node.kind === "page")).toHaveLength(3);
    expect(find(canvas, "lines").lines).toHaveLength(3);
  });

  it("draws a reading inside's line as working and a refused one as refused", () => {
    const canvas = laid(fixture.home([fixture.figma()]));
    expect(
      find(canvas, "lines")
        .lines.map((line) => line.state)
        .sort(),
    ).toEqual(["drawn", "opening", "refused"]);
  });

  it("gives a refused inside one stop that opens the page itself", () => {
    const canvas = laid(fixture.home([fixture.figma()]));
    expect(canvas.stops.find((stop) => stop.id === "i:q-figma/dev")).toMatchObject({
      activate: { kind: "row", row: "dev" },
      parent: "p:q-figma/figma.com/dev",
    });
  });

  it("grows a place by its known sections only when they are shown", () => {
    const shut = find(laid(fixture.home([fixture.stripe()])), "place").box.height;
    const open = find(
      laid(fixture.home([fixture.stripe()]), { expanded: new Set(["q-stripe/stripe.com"]) }),
      "place",
    ).box.height;
    expect(open - shut).toBe(10 * SIZE.section + 4);
  });
});
