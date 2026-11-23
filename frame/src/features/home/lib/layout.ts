import {
  bounds,
  branch,
  elbow,
  roundedPath,
  snap,
  type FocusStop,
  type Point,
} from "$shared/canvas";
import type { Day, Entry, HomeIntent, Inside, Place, Row, Search } from "./model";

/** Every size a card is drawn at. The cards read these, so the layout is the drawing. */
export const SIZE = {
  search: { width: 208, height: 64 },
  card: 288,
  inside: 232,
  pad: 6,
  header: 52,
  row: 32,
  section: 28,
  tray: 8,
  coming: 1,
  failed: 92,
  child: 30,
  insideHeader: 36,
  waiting: 64,
  refused: 84,
  label: 28,
  also: 40,
  page: 64,
  address: 32,
  day: 32,
  unread: 96,
} as const;

const GAP = 32;
const INSIDE_GAP = 24;
const PAIR_GAP = 16;
const STACK_GAP = 12;
const LANE_GAP = 48;
const DAY_GAP = 72;
/** The place stands centred under the field; the search to its left, insides to its right. */
export const PLACE_X = -SIZE.card / 2;
export const SEARCH_X = PLACE_X - GAP - SIZE.search.width;

type Box = { x: number; y: number; width: number; height: number };

type Line = { d: string; state: "drawn" | "opening" | "refused" };

export type HomeNode =
  | { kind: "search"; id: string; box: Box; search: Search; day: string }
  | {
      kind: "place";
      id: string;
      box: Box;
      search: Search;
      place: Place;
      rows: readonly (Row | null)[];
    }
  | { kind: "inside"; id: string; box: Box; search: Search; row: Row; inside: Inside }
  | { kind: "also"; id: string; box: Box; search: Search }
  | { kind: "social"; id: string; box: Box; search: Search; place: Place }
  | { kind: "page"; id: string; box: Box; search: Search; page: number }
  | { kind: "address"; id: string; box: Box; entry: Entry & { kind: "address" } }
  | { kind: "day"; id: string; box: Box; day: Day; today: string }
  | { kind: "lines"; id: string; box: Box; lines: readonly Line[] };

/** What a stop is, for the keyboard: what Enter does, and where → and ← go. */
type Stop = FocusStop & {
  activate: HomeIntent | { kind: "more"; place: string } | null;
  /** A row whose inside can be read, and the inside it has now. */
  row?: { search: string; row: string; inside: Inside | null; first: string | null };
  /** A row inside another: ← goes back to it. */
  parent?: string;
};

export type Laid = {
  nodes: readonly HomeNode[];
  stops: readonly Stop[];
  /** The stop the keyboard starts on: the asked row of the newest search. */
  first: string | null;
  /** What the newest entry takes up, for the first view. */
  head: Box | null;
  height: number;
};

/** Which places show their full list of known sections. */
export type View = { expanded: ReadonlySet<string> };

const placeKey = (search: Search, place: Place) => `${search.id}/${place.id}`;
const stopId = (node: string, part?: string) => (part === undefined ? node : `${node}/${part}`);

const pairs = (count: number) => Math.ceil(count / 2);

function placeHeight(place: Place, slots: number, expanded: boolean): number {
  const { pad, header, row, section, tray } = SIZE;
  if (place.state === "failed") return pad + header + SIZE.failed + pad;
  let height = pad + header + slots * row;
  if (place.state === "arriving") height += SIZE.coming * row;
  if (place.more.length) height += row + (expanded ? pairs(place.more.length) * section + 4 : 0);
  if (place.actions.length) height += tray + pairs(place.actions.length) * section + tray * 2;
  return height + pad;
}

function insideHeight(inside: Inside): number {
  const { pad, insideHeader, child } = SIZE;
  if (inside.state === "opening") return pad + insideHeader + SIZE.waiting + pad;
  if (inside.state === "refused") return pad + insideHeader + SIZE.refused + pad;
  return pad + insideHeader + (inside.rows.length + (inside.more ? 1 : 0)) * child + pad;
}

const socialHeight = (count: number) =>
  SIZE.pad + SIZE.label + pairs(count) * SIZE.section + SIZE.pad;
const alsoHeight = (count: number) => SIZE.pad + SIZE.label + count * SIZE.also + SIZE.pad;

/** Where a row of a place card stands, from the card's top. */
const rowTop = (index: number) => SIZE.pad + SIZE.header + index * SIZE.row;
/** Where the first known section past the rows stands, and the actions' tray. */
const sectionsTop = (place: Place, slots: number) =>
  rowTop(slots) + (place.state === "arriving" ? SIZE.coming * SIZE.row : 0);
const trayTop = (place: Place, slots: number, expanded: boolean) =>
  sectionsTop(place, slots) +
  (place.more.length
    ? SIZE.row + (expanded ? pairs(place.more.length) * SIZE.section + 4 : 0)
    : 0) +
  SIZE.tray;

const centre = (box: Box): Point => ({ x: box.x, y: box.y + box.height / 2 });

type Lane = { nodes: HomeNode[]; stops: Stop[]; lines: Line[]; first: string | null };

/** A place card and the insides of its rows, from `x`; returns where the next thing may start. */
function laidPlace(
  lane: Lane,
  search: Search,
  place: Place,
  slots: readonly (Row | null)[],
  x: number,
  top: number,
  view: View,
): { right: number; bottom: number } {
  const key = placeKey(search, place);
  const id = `p:${key}`;
  const expanded = view.expanded.has(key);
  const card = { x, y: top, width: SIZE.card, height: placeHeight(place, slots.length, expanded) };
  lane.nodes.push({ kind: "place", id, box: card, search, place, rows: slots });
  const at = (y: number, height: number, width = SIZE.card - SIZE.pad * 2) => ({
    x: x + SIZE.pad,
    y: top + y,
    width,
    height,
  });
  lane.stops.push({
    id,
    rect: at(SIZE.pad, SIZE.header),
    activate: { kind: "site", search: search.id, place: place.id },
  });
  let right = x + SIZE.card;
  let bottom = card.y + card.height;
  let insideFloor = -Infinity;
  const insideX = right + INSIDE_GAP;
  slots.forEach((row, index) => {
    if (!row) return;
    const rect = at(rowTop(index), SIZE.row);
    const rowStop = stopId(id, row.id);
    const insideId = `i:${search.id}/${row.id}`;
    const first =
      row.inside?.state === "open" && row.inside.rows[0]
        ? stopId(insideId, row.inside.rows[0].id)
        : row.inside?.state === "refused"
          ? insideId
          : null;
    lane.stops.push({
      id: rowStop,
      rect,
      activate: { kind: "row", search: search.id, row: row.id },
      ...(row.opens ? { row: { search: search.id, row: row.id, inside: row.inside, first } } : {}),
    });
    if (row.id === place.asked && lane.first === null) lane.first = rowStop;
    if (!row.inside || !row.opens) return;
    const height = insideHeight(row.inside);
    const y = Math.max(rect.y - SIZE.pad - (SIZE.insideHeader - SIZE.row) / 2, insideFloor);
    const box = { x: insideX, y, width: SIZE.inside, height };
    insideFloor = y + height + STACK_GAP;
    lane.nodes.push({ kind: "inside", id: insideId, box, search, row, inside: row.inside });
    const from = { x: x + SIZE.card - SIZE.pad, y: rect.y + SIZE.row / 2 };
    const to = { x: insideX, y: y + SIZE.pad + SIZE.insideHeader / 2 };
    lane.lines.push({
      d: roundedPath(elbow(from, to)),
      state: row.inside.state === "open" ? "drawn" : row.inside.state,
    });
    const inner = (offset: number, h: number) => ({
      x: insideX + SIZE.pad,
      y: y + offset,
      width: SIZE.inside - SIZE.pad * 2,
      height: h,
    });
    if (row.inside.state === "open") {
      row.inside.rows.forEach((child, n) => {
        lane.stops.push({
          id: stopId(insideId, child.id),
          rect: inner(SIZE.pad + SIZE.insideHeader + n * SIZE.child, SIZE.child),
          activate: { kind: "row", search: search.id, row: child.id },
          parent: rowStop,
        });
      });
      if (row.inside.more)
        lane.stops.push({
          id: stopId(insideId, "more"),
          rect: inner(
            SIZE.pad + SIZE.insideHeader + row.inside.rows.length * SIZE.child,
            SIZE.child,
          ),
          activate: { kind: "row", search: search.id, row: row.id },
          parent: rowStop,
        });
    } else if (row.inside.state === "refused") {
      lane.stops.push({
        id: insideId,
        rect: inner(SIZE.pad, SIZE.insideHeader + SIZE.refused),
        activate: { kind: "row", search: search.id, row: row.id },
        parent: rowStop,
      });
    }
    right = Math.max(right, box.x + box.width);
    bottom = Math.max(bottom, box.y + box.height);
  });
  if (place.state !== "failed") {
    const sections = sectionsTop(place, slots.length);
    if (place.more.length) {
      lane.stops.push({
        id: stopId(id, "more"),
        rect: at(sections, SIZE.row),
        activate: { kind: "more", place: key },
      });
      if (expanded)
        place.more.forEach((section, n) => {
          const half = (SIZE.card - SIZE.pad * 2) / 2;
          lane.stops.push({
            id: stopId(id, `more:${section.id}`),
            rect: {
              ...at(sections + SIZE.row + Math.floor(n / 2) * SIZE.section, SIZE.section, half),
              x: x + SIZE.pad + (n % 2) * half,
            },
            activate: { kind: "row", search: search.id, row: section.id },
          });
        });
    }
    const tray = trayTop(place, slots.length, expanded);
    const column = (SIZE.card - SIZE.pad * 2 - SIZE.tray * 3) / 2;
    place.actions.forEach((action, n) => {
      lane.stops.push({
        id: stopId(id, `action:${action.id}`),
        rect: {
          x: x + SIZE.pad + SIZE.tray + (n % 2) * (column + SIZE.tray),
          y: top + tray + SIZE.tray + Math.floor(n / 2) * SIZE.section,
          width: column,
          height: SIZE.section,
        },
        activate: { kind: "row", search: search.id, row: action.id },
      });
    });
  }
  return { right, bottom };
}

function laidSearch(search: Search, top: number, day: string, view: View): Lane {
  const lane: Lane = { nodes: [], stops: [], lines: [], first: null };
  const card = { x: SEARCH_X, y: top, ...SIZE.search };
  lane.nodes.push({ kind: "search", id: `s:${search.id}`, box: card, search, day });
  lane.stops.push({
    id: `s:${search.id}`,
    rect: card,
    activate: { kind: "results", search: search.id },
  });
  const out = { x: card.x + card.width, y: card.y + card.height / 2 };
  let left = card.y + card.height;
  if (search.also.length) {
    const box = {
      x: SEARCH_X,
      y: left + STACK_GAP,
      width: SIZE.search.width,
      height: alsoHeight(search.also.length),
    };
    lane.nodes.push({ kind: "also", id: `a:${search.id}`, box, search });
    search.also.forEach((also, n) =>
      lane.stops.push({
        id: stopId(`a:${search.id}`, also.id),
        rect: {
          x: box.x + SIZE.pad,
          y: box.y + SIZE.pad + SIZE.label + n * SIZE.also,
          width: box.width - SIZE.pad * 2,
          height: SIZE.also,
        },
        activate: { kind: "result", search: search.id, result: also.id },
      }),
    );
    left = box.y + box.height;
  }
  let bottom = left;
  const social = (place: Place, x: number, under: number) => {
    if (!place.social.length || place.state === "failed") return under;
    const box = {
      x,
      y: under + STACK_GAP,
      width: SIZE.card,
      height: socialHeight(place.social.length),
    };
    const id = `e:${placeKey(search, place)}`;
    lane.nodes.push({ kind: "social", id, box, search, place });
    place.social.forEach((social, n) =>
      lane.stops.push({
        id: stopId(id, social.network),
        rect: {
          x: x + SIZE.pad + (n % 2) * ((SIZE.card - SIZE.pad * 2) / 2),
          y: box.y + SIZE.pad + SIZE.label + Math.floor(n / 2) * SIZE.section,
          width: (SIZE.card - SIZE.pad * 2) / 2,
          height: SIZE.section,
        },
        activate: { kind: "social", search: search.id, place: place.id, network: social.network },
      }),
    );
    return box.y + box.height;
  };
  if (search.kind === "place") {
    const placed = laidPlace(lane, search, search.place, search.place.rows, PLACE_X, top, view);
    const card = lane.nodes.find((node) => node.kind === "place")!.box;
    bottom = Math.max(bottom, placed.bottom, social(search.place, PLACE_X, card.y + card.height));
    lane.lines.push({ d: roundedPath([out, { x: PLACE_X, y: out.y }]), state: "drawn" });
  } else if (search.kind === "compare") {
    const [a, b] = search.places;
    const byId = (place: Place) => new Map(place.rows.map((row) => [row.id, row]));
    const left = byId(a);
    const right = byId(b);
    const slotsA = search.pairs.map((pair) => (pair.left ? (left.get(pair.left) ?? null) : null));
    const slotsB = search.pairs.map((pair) =>
      pair.right ? (right.get(pair.right) ?? null) : null,
    );
    const first = laidPlace(lane, search, a, slotsA, PLACE_X, top, view);
    const nextX =
      first.right === PLACE_X + SIZE.card ? first.right + PAIR_GAP : first.right + INSIDE_GAP;
    const second = laidPlace(lane, search, b, slotsB, nextX, top, view);
    const [cardA, cardB] = lane.nodes
      .filter((node) => node.kind === "place")
      .map((node) => node.box);
    bottom = Math.max(
      bottom,
      first.bottom,
      second.bottom,
      social(a, PLACE_X, cardA!.y + cardA!.height),
      social(b, nextX, cardB!.y + cardB!.height),
    );
    lane.lines.push({ d: roundedPath([out, { x: PLACE_X, y: out.y }]), state: "drawn" });
  } else {
    const ends: Point[] = [];
    search.pages.forEach((page, n) => {
      const box = {
        x: PLACE_X,
        y: top + n * (SIZE.page + STACK_GAP),
        width: SIZE.card,
        height: SIZE.page,
      };
      const id = `o:${search.id}/${page.id}`;
      lane.nodes.push({ kind: "page", id, box, search, page: n });
      lane.stops.push({
        id,
        rect: box,
        activate: { kind: "page", search: search.id, page: page.id },
      });
      ends.push(centre(box));
      bottom = Math.max(bottom, box.y + box.height);
    });
    for (const route of branch(out, out.x + GAP / 2, ends))
      if (route) lane.lines.push({ d: roundedPath(route), state: "drawn" });
  }
  return lane;
}

function laidEntry(entry: Entry, top: number, day: string, view: View): Lane {
  if (entry.kind !== "address") return laidSearch(entry, top, day, view);
  const box = { x: SEARCH_X, y: top, width: SIZE.search.width, height: SIZE.address };
  const id = `t:${entry.id}`;
  return {
    nodes: [{ kind: "address", id, box, entry }],
    stops: [{ id, rect: box, activate: { kind: "address", entry: entry.id } }],
    lines: [],
    first: null,
  };
}

/**
 * Home's canvas from its model: today's entries newest first from the top,
 * each earlier day under the one after it. The same model and view always
 * give the same canvas.
 */
export function layout(days: readonly Day[], today: string, view: View): Laid {
  const nodes: HomeNode[] = [];
  const stops: Stop[] = [];
  let first: string | null = null;
  let head: Box | null = null;
  let at = 0;
  days.forEach((day, index) => {
    if (index > 0 || day.key !== today) {
      if (index > 0) at = snap(at + DAY_GAP - LANE_GAP);
      const height = day.entries === null ? SIZE.day + SIZE.unread : SIZE.day;
      nodes.push({
        kind: "day",
        id: `d:${day.key}`,
        box: { x: SEARCH_X, y: at, width: SIZE.search.width, height },
        day,
        today,
      });
      at = snap(at + height + (day.entries === null ? LANE_GAP : STACK_GAP * 2));
    }
    for (const entry of day.entries ?? []) {
      const lane = laidEntry(entry, at, day.key, view);
      const box = bounds(lane.nodes.map((node) => node.box))!;
      head ??= box;
      if (lane.lines.length) {
        const origin = { x: box.x, y: box.y };
        nodes.push({
          kind: "lines",
          id: `l:${entry.id}`,
          box,
          lines: lane.lines.map((line) => ({ ...line, d: shift(line.d, origin) })),
        });
      }
      nodes.push(...lane.nodes);
      stops.push(...lane.stops);
      first ??= lane.first ?? lane.stops[0]?.id ?? null;
      at = snap(box.y + box.height + LANE_GAP);
    }
  });
  return { nodes, stops, first, head, height: Math.max(0, at - LANE_GAP) };
}

/** A path moved so that `origin` is its zero. */
function shift(d: string, origin: Point): string {
  let axis = 0;
  return d.replace(/-?\d+(?:\.\d+)?/gu, (value) => {
    const moved = Number(value) - (axis++ % 2 === 0 ? origin.x : origin.y);
    return String(Math.round(moved * 100) / 100);
  });
}
