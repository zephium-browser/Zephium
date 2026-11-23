import {
  bounds,
  branch,
  elbow,
  roundedPath,
  snap,
  type FocusStop,
  type Point,
} from "$shared/canvas";
import type { Day, Entry, Inside, Place, Row, Search } from "$domain/home";
import type { HomeIntent } from "./intent";

/** Every size a card is drawn at. The cards read these, so the layout is the drawing. */
export const SIZE = {
  search: { width: 208, height: 64 },
  card: 288,
  inside: 232,
  pad: 6,
  header: 52,
  row: 32,
  section: 28,
  divide: 9,
  foot: 36,
  coming: 1,
  failed: 92,
  child: 30,
  insideHeader: 36,
  waiting: 64,
  refused: 84,
  label: 28,
  also: 40,
  alsoTwo: 56,
  page: 64,
  line: 40,
  thin: 44,
  day: 32,
  moreAlso: 30,
} as const;

/** Other results shown under an open search before "more". */
const ALSO_FIRST = 4;
/** Rows of a menu shown before the rest of what it holds. */
const MENU_FIRST = 7;

const GAP = 32;
const INSIDE_GAP = 24;
const PAIR_GAP = 16;
const STACK_GAP = 12;
const LINE_GAP = 8;
const LANE_GAP = 48;
const DAY_GAP = 72;
const AFTER_DAY = 24;
/** Text links in a row: the space between them and inside each. */
const LINK_GAP = 4;
const LINK_PAD = 12;
const THIN_LINK = 148;
const THIN_HEAD = 168;
/** The place stands centred under the field; the search to its left, insides to its right. */
export const PLACE_X = -SIZE.card / 2;
export const SEARCH_X = PLACE_X - GAP - SIZE.search.width;
/** A search drawn as one line spans the search and its place; its place part starts here. */
export const SPLIT = PLACE_X - SEARCH_X;
const LINE_WIDTH = SPLIT + SIZE.card;
/** The width a result's title wraps at inside its card. */
const ALSO_ROOM = SIZE.search.width - SIZE.pad * 2 - 20;

type Box = { x: number; y: number; width: number; height: number };

type Line = { d: string; state: "drawn" | "opening" | "refused" };

/** A text link in a row of them, as wide as the layout gives it. */
type Link = { id: string; label: string; kind: "row" | "action"; width: number };

export type HomeNode =
  | { kind: "search"; id: string; box: Box; search: Search; day: string }
  | { kind: "compact"; id: string; box: Box; search: Search; day: string }
  | {
      kind: "place";
      id: string;
      box: Box;
      search: Search;
      place: Place;
      rows: readonly Row[];
      /** Rows before this one are the ones two places share; a hairline parts them from the rest. */
      split: number;
      /** How wide each of the place's actions is drawn. */
      actions: readonly number[];
    }
  | {
      kind: "thin";
      id: string;
      box: Box;
      search: Search;
      place: Place;
      head: number;
      links: readonly Link[];
    }
  | {
      kind: "inside";
      id: string;
      box: Box;
      search: Search;
      place: string;
      row: Row;
      inside: Inside;
      /** A menu's rows still held behind its "more". */
      rest: number;
    }
  | {
      kind: "also";
      id: string;
      box: Box;
      search: Search;
      heights: readonly number[];
      /** Results past the first few, still behind "more". */
      hidden: number;
    }
  | { kind: "social"; id: string; box: Box; search: Search; place: Place }
  | { kind: "page"; id: string; box: Box; search: Search; page: number }
  | { kind: "address"; id: string; box: Box; entry: Entry & { kind: "address" } }
  | { kind: "day"; id: string; box: Box; day: Day; today: string }
  | { kind: "lines"; id: string; box: Box; lines: readonly Line[] };

/** What a stop is, for the keyboard: what Enter does, and where → and ← go. */
type Stop = FocusStop & {
  activate:
    | HomeIntent
    | { kind: "more"; place: string }
    | { kind: "open"; search: string }
    | { kind: "menu"; search: string; row: string }
    | { kind: "menuAll"; search: string; row: string }
    | { kind: "also"; search: string }
    | null;
  /** A row whose inside can be read or is held as a menu, and the inside it shows now. */
  row?: {
    search: string;
    row: string;
    inside: Inside | null;
    first: string | null;
    menu: boolean;
  };
  /** A row inside another: ← goes back to it. */
  parent?: string;
};

export type Laid = {
  nodes: readonly HomeNode[];
  stops: readonly Stop[];
  /** The stop the keyboard starts on: the asked row of the search in front. */
  first: string | null;
  /** What the search in front takes up, for the first view. */
  head: Box | null;
  /** Each entry's room on the canvas and the items drawn for it. */
  lanes: Readonly<Record<string, { box: Box; nodes: readonly string[]; first: string | null }>>;
  height: number;
};

/** How wide a line of text is drawn at a font size and weight, in pixels. */
export type Measure = (text: string, size: number, weight?: number) => number;

export type View = {
  /** Which places show their full list of known sections. */
  expanded: ReadonlySet<string>;
  /** The searches drawn open; every other is a line. */
  open: readonly string[];
  /** The open search the keyboard and the first view go to. */
  lead: string | null;
  /** Which menu rows show what they hold, by search and row, and `/all` for all of it. */
  shown: ReadonlySet<string>;
  /** Searches showing every one of their other results. */
  all: ReadonlySet<string>;
  /** The row a page was opened from, marked on return. */
  marked: { search: string; entry: string } | null;
  measure?: Measure;
};

const placeKey = (search: Search, place: Place) => `${search.id}/${place.id}`;
const stopId = (node: string, part?: string) => (part === undefined ? node : `${node}/${part}`);

const pairs = (count: number) => Math.ceil(count / 2);

const WIDE = /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]|\p{Extended_Pictographic}/u;
const NARROW = /[\sfijlrt.,:;'"!|()[\]]/u;

/** About how wide a line of text is drawn at a font size: what is laid out by where nothing measures. */
function estimate(text: string, size: number): number {
  let em = 0;
  for (const char of text)
    em += WIDE.test(char)
      ? 1
      : NARROW.test(char)
        ? 0.32
        : /[mwMW]/u.test(char)
          ? 0.86
          : /[A-Z]/u.test(char)
            ? 0.68
            : 0.56;
  return Math.ceil(em * size);
}

let measured: Measure = estimate;
const textWidth = (text: string, size: number, weight?: number) =>
  Math.ceil(measured(text, size, weight));

/** How wide a text link is drawn to hold its label. */
export const linkWidth = (label: string) => textWidth(label, 12) + LINK_PAD + 2;

/** Widths for text links that share `room`: each as wide as its text, the longest cut first. */
function share(labels: readonly string[], room: number): number[] {
  const want = labels.map(linkWidth);
  let left = room - LINK_GAP * Math.max(0, labels.length - 1);
  const widths = labels.map(() => 0);
  const order = want.map((_, index) => index).sort((a, b) => want[a]! - want[b]! || a - b);
  order.forEach((index, n) => {
    widths[index] = Math.max(0, Math.min(want[index]!, Math.floor(left / (order.length - n))));
    left -= widths[index]!;
  });
  return widths;
}

const divided = (split: number, count: number) => split > 0 && split < count;

function placeHeight(place: Place, count: number, split: number, expanded: boolean): number {
  const { pad, header, row, section } = SIZE;
  if (place.state === "failed") return pad + header + SIZE.failed + pad;
  let height = pad + header + count * row + (divided(split, count) ? SIZE.divide : 0);
  if (place.state === "arriving") height += SIZE.coming * row;
  if (place.more.length) height += row + (expanded ? pairs(place.more.length) * section + 4 : 0);
  if (place.actions.length) height += SIZE.foot;
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
/** A result's title takes a second line only when it does not fit on one. */
const alsoHeights = (list: Search["also"]) =>
  list.map((also) => (textWidth(also.title, 12, 400) > ALSO_ROOM ? SIZE.alsoTwo : SIZE.also));

/** Where a row of a place card stands, from the card's top. */
const rowTop = (index: number, split: number, count: number) =>
  SIZE.pad +
  SIZE.header +
  index * SIZE.row +
  (divided(split, count) && index >= split ? SIZE.divide : 0);
/** Where the first known section past the rows stands, and the actions' line. */
const sectionsTop = (place: Place, count: number, split: number) =>
  SIZE.pad +
  SIZE.header +
  count * SIZE.row +
  (divided(split, count) ? SIZE.divide : 0) +
  (place.state === "arriving" ? SIZE.coming * SIZE.row : 0);
const footTop = (place: Place, count: number, split: number, expanded: boolean) =>
  sectionsTop(place, count, split) +
  (place.more.length ? SIZE.row + (expanded ? pairs(place.more.length) * SIZE.section + 4 : 0) : 0);

const centre = (box: Box): Point => ({ x: box.x, y: box.y + box.height / 2 });

type Lane = { nodes: HomeNode[]; stops: Stop[]; lines: Line[]; first: string | null };

/** A place card and the insides of its rows, from `x`; returns where the next thing may start. */
function laidPlace(
  lane: Lane,
  search: Search,
  place: Place,
  slots: readonly Row[],
  split: number,
  x: number,
  top: number,
  view: View,
): { right: number; bottom: number } {
  const key = placeKey(search, place);
  const id = `p:${key}`;
  const expanded = view.expanded.has(key);
  const count = slots.length;
  const card = {
    x,
    y: top,
    width: SIZE.card,
    height: placeHeight(place, count, split, expanded),
  };
  const inner = SIZE.card - SIZE.pad * 2;
  const actions =
    place.state === "failed"
      ? []
      : share(
          place.actions.map((action) => action.label),
          inner - LINK_GAP * 2,
        );
  lane.nodes.push({ kind: "place", id, box: card, search, place, rows: slots, split, actions });
  const at = (y: number, height: number, width = inner) => ({
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
    const rect = at(rowTop(index, split, count), SIZE.row);
    const rowStop = stopId(id, row.id);
    const insideId = `i:${search.id}/${row.id}`;
    // A menu shows what it holds when it is opened here, its first rows and
    // then the rest; a page's inside, when it has been read.
    const held = row.menu
      ? view.shown.has(`${search.id}/${row.id}`)
        ? row.inside
        : null
      : row.inside;
    const whole = view.shown.has(`${search.id}/${row.id}/all`);
    const rest =
      row.menu && held?.state === "open" && !whole ? Math.max(0, held.rows.length - MENU_FIRST) : 0;
    const inside: Inside | null =
      held?.state === "open" && row.menu
        ? { state: "open", rows: rest ? held.rows.slice(0, MENU_FIRST) : held.rows, more: rest > 0 }
        : held;
    const first =
      inside?.state === "open" && inside.rows[0]
        ? stopId(insideId, inside.rows[0].id)
        : inside?.state === "refused"
          ? insideId
          : null;
    lane.stops.push({
      id: rowStop,
      rect,
      activate: row.menu
        ? { kind: "menu", search: search.id, row: row.id }
        : { kind: "row", search: search.id, row: row.id },
      ...(row.opens || row.menu
        ? { row: { search: search.id, row: row.id, inside, first, menu: row.menu } }
        : {}),
    });
    const marked = view.marked?.search === search.id && view.marked.entry === row.id;
    if (marked) lane.first = rowStop;
    else if (row.id === place.asked && lane.first === null) lane.first = rowStop;
    if (!inside || !(row.opens || row.menu)) return;
    const height = insideHeight(inside);
    const y = Math.max(rect.y - SIZE.pad - (SIZE.insideHeader - SIZE.row) / 2, insideFloor);
    const box = { x: insideX, y, width: SIZE.inside, height };
    insideFloor = y + height + STACK_GAP;
    lane.nodes.push({
      kind: "inside",
      id: insideId,
      box,
      search,
      place: place.id,
      row,
      inside,
      rest,
    });
    const from = { x: x + SIZE.card - SIZE.pad, y: rect.y + SIZE.row / 2 };
    const to = { x: insideX, y: y + SIZE.pad + SIZE.insideHeader / 2 };
    lane.lines.push({
      d: roundedPath(elbow(from, to)),
      state: inside.state === "open" ? "drawn" : inside.state,
    });
    const within = (offset: number, h: number) => ({
      x: insideX + SIZE.pad,
      y: y + offset,
      width: SIZE.inside - SIZE.pad * 2,
      height: h,
    });
    if (inside.state === "open") {
      inside.rows.forEach((child, n) => {
        if (view.marked?.search === search.id && view.marked.entry === child.id)
          lane.first = stopId(insideId, child.id);
        lane.stops.push({
          id: stopId(insideId, child.id),
          rect: within(SIZE.pad + SIZE.insideHeader + n * SIZE.child, SIZE.child),
          activate: { kind: "row", search: search.id, row: child.id },
          parent: rowStop,
        });
      });
      if (inside.more)
        lane.stops.push({
          id: stopId(insideId, "more"),
          rect: within(SIZE.pad + SIZE.insideHeader + inside.rows.length * SIZE.child, SIZE.child),
          // A menu has no page of its own: its "more" shows the rest it holds.
          activate: row.menu
            ? { kind: "menuAll", search: search.id, row: row.id }
            : { kind: "row", search: search.id, row: row.id },
          parent: rowStop,
        });
    } else if (inside.state === "refused") {
      lane.stops.push({
        id: insideId,
        rect: within(SIZE.pad, SIZE.insideHeader + SIZE.refused),
        activate:
          inside.reason === "gone"
            ? { kind: "again", search: search.id }
            : { kind: "row", search: search.id, row: row.id },
        parent: rowStop,
      });
    }
    right = Math.max(right, box.x + box.width);
    bottom = Math.max(bottom, box.y + box.height);
  });
  if (place.state !== "failed") {
    const sections = sectionsTop(place, count, split);
    if (place.more.length) {
      lane.stops.push({
        id: stopId(id, "more"),
        rect: at(sections, SIZE.row),
        activate: { kind: "more", place: key },
      });
      if (expanded)
        place.more.forEach((section, n) => {
          const half = inner / 2;
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
    // The actions' line: a hairline, then the links, from the card's inner edge.
    const foot = top + footTop(place, count, split, expanded) + 5;
    let along = x + SIZE.pad + LINK_GAP;
    place.actions.forEach((action, n) => {
      const width = actions[n]!;
      lane.stops.push({
        id: stopId(id, `action:${action.id}`),
        rect: { x: along, y: foot, width, height: SIZE.section },
        activate: { kind: "row", search: search.id, row: action.id },
      });
      along += width + LINK_GAP;
    });
  }
  return { right, bottom };
}

/**
 * A place with too little to read for a card: one line as wide as the place
 * column, its name and then the few links it has, the longest cut first.
 */
function laidThin(lane: Lane, search: Search, place: Place, top: number, view: View): Box {
  const key = placeKey(search, place);
  const id = `p:${key}`;
  const head = Math.min(
    THIN_HEAD,
    Math.max(120, 54 + textWidth(place.title, 14) + 8 + textWidth(place.host, 12)),
  );
  const listed = [
    ...place.rows.map((row) => ({ id: row.id, label: row.label, kind: "row" as const })),
    ...place.actions.map((action) => ({
      id: action.id,
      label: action.label,
      kind: "action" as const,
    })),
  ];
  const widths = share(
    listed.map((link) => link.label),
    SIZE.card - head - LINK_GAP - 8,
  ).map((width) => Math.min(THIN_LINK, width));
  const links: Link[] = listed.map((link, n) => ({ ...link, width: widths[n]! }));
  const box = {
    x: PLACE_X,
    y: top + (SIZE.search.height - SIZE.thin) / 2,
    width: SIZE.card,
    height: SIZE.thin,
  };
  lane.nodes.push({ kind: "thin", id, box, search, place, head, links });
  lane.stops.push({
    id,
    rect: { ...box, width: head },
    activate: { kind: "site", search: search.id, place: place.id },
  });
  let along = box.x + head + LINK_GAP;
  for (const link of links) {
    if (link.width <= 0) continue;
    const stop = stopId(id, link.kind === "row" ? link.id : `action:${link.id}`);
    lane.stops.push({
      id: stop,
      rect: {
        x: along,
        y: box.y + (SIZE.thin - SIZE.section) / 2,
        width: link.width,
        height: SIZE.section,
      },
      activate: { kind: "row", search: search.id, row: link.id },
    });
    const marked = view.marked?.search === search.id && view.marked.entry === link.id;
    if (marked || (link.id === place.asked && lane.first === null)) lane.first = stop;
    along += link.width + LINK_GAP;
  }
  return box;
}

/** The rows of a comparison: the ones both places have, in one order, then each place's own. */
function compared(search: Extract<Search, { kind: "compare" }>) {
  const [a, b] = search.places;
  const left = new Map(a.rows.map((row) => [row.id, row]));
  const right = new Map(b.rows.map((row) => [row.id, row]));
  const sharedA: Row[] = [];
  const sharedB: Row[] = [];
  for (const pair of search.pairs) {
    const one = pair.left === null ? undefined : left.get(pair.left);
    const two = pair.right === null ? undefined : right.get(pair.right);
    if (!one || !two || sharedA.includes(one) || sharedB.includes(two)) continue;
    sharedA.push(one);
    sharedB.push(two);
  }
  return {
    split: sharedA.length,
    a: [...sharedA, ...a.rows.filter((row) => !sharedA.includes(row))],
    b: [...sharedB, ...b.rows.filter((row) => !sharedB.includes(row))],
  };
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
    const listed = view.all.has(search.id) ? search.also : search.also.slice(0, ALSO_FIRST);
    const hidden = search.also.length - listed.length;
    const heights = alsoHeights(listed);
    const box = {
      x: SEARCH_X,
      y: left + STACK_GAP,
      width: SIZE.search.width,
      height:
        SIZE.pad * 2 +
        SIZE.label +
        heights.reduce((sum, height) => sum + height, 0) +
        (hidden ? SIZE.moreAlso : 0),
    };
    lane.nodes.push({ kind: "also", id: `a:${search.id}`, box, search, heights, hidden });
    let y = box.y + SIZE.pad + SIZE.label;
    listed.forEach((also, n) => {
      lane.stops.push({
        id: stopId(`a:${search.id}`, also.id),
        rect: { x: box.x + SIZE.pad, y, width: box.width - SIZE.pad * 2, height: heights[n]! },
        activate: { kind: "result", search: search.id, result: also.id },
      });
      y += heights[n]!;
    });
    if (hidden)
      lane.stops.push({
        id: stopId(`a:${search.id}`, "more"),
        rect: { x: box.x + SIZE.pad, y, width: box.width - SIZE.pad * 2, height: SIZE.moreAlso },
        activate: { kind: "also", search: search.id },
      });
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
        id: stopId(id, social),
        rect: {
          x: x + SIZE.pad + (n % 2) * ((SIZE.card - SIZE.pad * 2) / 2),
          y: box.y + SIZE.pad + SIZE.label + Math.floor(n / 2) * SIZE.section,
          width: (SIZE.card - SIZE.pad * 2) / 2,
          height: SIZE.section,
        },
        activate: { kind: "social", search: search.id, place: place.id, network: social },
      }),
    );
    return box.y + box.height;
  };
  if (search.kind === "place") {
    const place = search.place;
    let under: Box;
    if (place.thin && place.state !== "failed") under = laidThin(lane, search, place, top, view);
    else {
      bottom = Math.max(
        bottom,
        laidPlace(lane, search, place, place.rows, 0, PLACE_X, top, view).bottom,
      );
      under = lane.nodes.find((node) => node.kind === "place")!.box;
    }
    bottom = Math.max(
      bottom,
      under.y + under.height,
      social(place, PLACE_X, under.y + under.height),
    );
    lane.lines.push({ d: roundedPath([out, { x: PLACE_X, y: out.y }]), state: "drawn" });
  } else if (search.kind === "compare") {
    const [a, b] = search.places;
    const rows = compared(search);
    const first = laidPlace(lane, search, a, rows.a, rows.split, PLACE_X, top, view);
    const nextX =
      first.right === PLACE_X + SIZE.card ? first.right + PAIR_GAP : first.right + INSIDE_GAP;
    const second = laidPlace(lane, search, b, rows.b, rows.split, nextX, top, view);
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

/** An entry that is drawn as one line: an address, or a search that is not open. */
const asLine = (entry: Entry, view: View) =>
  entry.kind === "address" || !view.open.includes(entry.id);

function laidEntry(entry: Entry, top: number, day: string, view: View): Lane {
  const box = { x: SEARCH_X, y: top, width: LINE_WIDTH, height: SIZE.line };
  if (entry.kind === "address") {
    const id = `t:${entry.id}`;
    return {
      nodes: [{ kind: "address", id, box, entry }],
      stops: [{ id, rect: box, activate: { kind: "address", entry: entry.id } }],
      lines: [],
      first: null,
    };
  }
  if (view.open.includes(entry.id)) return laidSearch(entry, top, day, view);
  return {
    nodes: [{ kind: "compact", id: `c:${entry.id}`, box, search: entry, day }],
    stops: [{ id: `s:${entry.id}`, rect: box, activate: { kind: "open", search: entry.id } }],
    lines: [],
    first: null,
  };
}

/**
 * Home's canvas from its model: today's entries newest first from the top,
 * each earlier day under the one after it. Only the search in front is drawn
 * open; the rest are lines to scan. The same model and view always give the
 * same canvas.
 */
export function layout(days: readonly Day[], today: string, view: View): Laid {
  measured = view.measure ?? estimate;
  const nodes: HomeNode[] = [];
  const stops: Stop[] = [];
  const lanes: Record<string, { box: Box; nodes: string[]; first: string | null }> = {};
  let first: string | null = null;
  let head: Box | null = null;
  let bottom: number | null = null;
  let after: "day" | "line" | "lane" = "lane";
  days.forEach((day, index) => {
    if (index > 0 || day.key !== today) {
      const y = bottom === null ? 0 : snap(bottom + DAY_GAP);
      const height = SIZE.day;
      nodes.push({
        kind: "day",
        id: `d:${day.key}`,
        box: { x: SEARCH_X, y, width: SIZE.search.width, height },
        day,
        today,
      });
      bottom = y + height;
      after = "day";
    }
    for (const entry of day.entries) {
      const line = asLine(entry, view);
      const gap = after === "day" ? AFTER_DAY : line && after === "line" ? LINE_GAP : LANE_GAP;
      const lane = laidEntry(entry, bottom === null ? 0 : snap(bottom + gap), day.key, view);
      const box = bounds(lane.nodes.map((node) => node.box))!;
      if (entry.id === view.lead || (!view.lead && head === null)) head = box;
      if (lane.lines.length) {
        const origin = { x: box.x, y: box.y };
        nodes.push({
          kind: "lines",
          id: `l:${entry.id}`,
          box,
          lines: lane.lines.map((path) => ({ ...path, d: shift(path.d, origin) })),
        });
      }
      nodes.push(...lane.nodes);
      stops.push(...lane.stops);
      const start = lane.first ?? lane.stops[0]?.id ?? null;
      lanes[entry.id] = {
        box,
        nodes: [
          ...(lane.lines.length ? [`l:${entry.id}`] : []),
          ...lane.nodes.map((node) => node.id),
        ],
        first: start,
      };
      if (entry.id === view.lead) first = start;
      bottom = box.y + box.height;
      after = line ? "line" : "lane";
    }
  });
  first ??= stops[0]?.id ?? null;
  return { nodes, stops, first, head, lanes, height: bottom ?? 0 };
}

/** A path moved so that `origin` is its zero. */
function shift(d: string, origin: Point): string {
  let axis = 0;
  return d.replace(/-?\d+(?:\.\d+)?/gu, (value) => {
    const moved = Number(value) - (axis++ % 2 === 0 ? origin.x : origin.y);
    return String(Math.round(moved * 100) / 100);
  });
}
