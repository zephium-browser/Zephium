import { SvelteMap } from "svelte/reactivity";
import type { ArtifactContent } from "$shared/ui/data/Artifact/artifact";
import type { CanvasPosition, CanvasSize } from "./canvas-model";
import * as m from "$shared/i18n/messages";
import { diagramRows, type DiagramRows } from "./diagram-rows";
import { placePlates } from "./diagram-labels";
import { DIAGRAM, ROUTE, gutterOf } from "./diagram-metrics";

export { DIAGRAM, PLATE, plateHeight, plateWidth } from "./diagram-metrics";

type Diagram = Extract<ArtifactContent, { kind: "diagram" }>;
type Rect = CanvasPosition & CanvasSize;

/** Words that make a part a side channel: where telemetry goes, not where the work does. */
const SIDE =
  /\b(analytics|metrics|telemetry|monitoring|observability|logs?|logging|traces?|tracing|apm|audit)\b/iu;

/**
 * What a layout depends on and nothing else: the parts with their tier (the
 * stated layer's place, `-1` without layers) and whether they are a side
 * channel, the named tiers, and the flows between known parts by their index.
 */
export type DiagramShape = {
  lanes: string[];
  parts: { id: string; lane: number; side?: boolean }[];
  flows: { index: number; from: string; to: string; label: string }[];
};

export function diagramShape(diagram: Diagram): DiagramShape {
  const used = diagram.layers.filter((layer) =>
    diagram.nodes.some((node) => node.layer === layer.id),
  );
  const lane = (layer: string | undefined | null) => {
    if (!used.length) return -1;
    const at = used.findIndex((entry) => entry.id === layer);
    return at < 0 ? used.length : at;
  };
  const known = new Set(diagram.nodes.map((node) => node.id));
  // Two flows between the same parts the same way are one line; the popover names both.
  const drawn = new Set<string>();
  return {
    lanes: used.map((layer) => layer.name),
    parts: diagram.nodes.map((node) => ({
      id: node.id,
      lane: lane(node.layer),
      ...(SIDE.test(node.name) ? { side: true } : {}),
    })),
    flows: diagram.edges
      .map((edge, index) => ({ index, ...edge }))
      .filter((edge) => edge.from !== edge.to && known.has(edge.from) && known.has(edge.to))
      .filter(
        (edge) => !drawn.has(`${edge.from}>${edge.to}`) && !!drawn.add(`${edge.from}>${edge.to}`),
      )
      .map((edge) => ({
        index: edge.index,
        from: edge.from,
        to: edge.to,
        label: edge.label?.trim() ?? "",
      })),
  };
}

/** One flow drawn: an orthogonal line from source to target and where its name stands. */
export type DiagramFlow = {
  from: string;
  to: string;
  points: CanvasPosition[];
  /** Its name's centre on the line, where it reads at rest or when its part is looked at. */
  plate?: CanvasPosition;
  /** Its name stands at rest: it found a free run of its own line. */
  resting: boolean;
  /** A side channel or a reply: drawn quieter than the work's own path. */
  quiet: boolean;
  /** Drawn up the picture, against the way the work goes: a reply or a callback. */
  back: boolean;
  /** The other way of a pair: drawn on its partner's line, only its head its own. */
  twin?: boolean;
};
/**
 * A tier's name and the rows it holds, along the way the picture reads: its
 * top and height when it reads down, its left and width when it reads right.
 */
export type DiagramTier = { name: string; start: number; extent: number };
/** The way a picture reads: rows down the page, or columns to the right. */
export type DiagramWay = "down" | "right";
/**
 * Where everything stands, from the picture's corner: parts, flows by their
 * index, the tiers' names, and the whole picture's `bounds`.
 */
export type DiagramLayout = {
  way: DiagramWay;
  at: Record<string, CanvasPosition>;
  bounds: Rect;
  /** The room the tiers are named in: a column at the left, or a band over the columns. */
  gutter: number;
  tiers: DiagramTier[];
  flows: Record<number, DiagramFlow>;
  /** Laid out by the layout engine rather than by rows alone. */
  settled: boolean;
};

/**
 * The layout while the engine answers, and wherever it cannot run: the rows
 * centred on each other down the picture, each flow an elbow from its source
 * to its target, a part set beside another joined straight across.
 */
function rowLayout(shape: DiagramShape, rows: DiagramRows): DiagramLayout {
  const { width: W, height: H } = DIAGRAM.node;
  const left = gutterOf(shape.lanes);
  const side = 64;
  const block = (id: string) => {
    const sides = rows.beside.get(id);
    return W + (sides?.before ? W + side : 0) + (sides?.after ? W + side : 0);
  };
  const span = (row: readonly string[]) =>
    row.reduce((sum, id) => sum + block(id), 0) + Math.max(0, row.length - 1) * DIAGRAM.column;
  const widest = Math.max(0, ...rows.rows.map(span));
  const at: Record<string, CanvasPosition> = {};
  const starts: number[] = [];
  let y = 0;
  rows.rows.forEach((row, index) => {
    if (index && rows.tierStart.has(index)) y += DIAGRAM.tier;
    starts.push(y);
    let x = left + (widest - span(row)) / 2;
    for (const id of row) {
      const sides = rows.beside.get(id);
      if (sides?.before) {
        at[sides.before] = { x, y };
        x += W + side;
      }
      at[id] = { x, y };
      x += W;
      if (sides?.after) {
        at[sides.after] = { x: x + side, y };
        x += W + side;
      }
      x += DIAGRAM.column;
    }
    y += H + DIAGRAM.row;
  });
  const flows: Record<number, DiagramFlow> = {};
  for (const flow of shape.flows) {
    const a = at[flow.from]!;
    const b = at[flow.to]!;
    let points: CanvasPosition[];
    if (a.y === b.y) {
      const right = a.x < b.x;
      const mid = a.y + H / 2;
      points = [
        { x: right ? a.x + W : a.x, y: mid },
        { x: right ? b.x : b.x + W, y: mid },
      ];
    } else {
      const down = a.y < b.y;
      const from = { x: a.x + W / 2, y: down ? a.y + H : a.y };
      const to = { x: b.x + W / 2, y: down ? b.y : b.y + H };
      const mid = down ? to.y - DIAGRAM.row / 2 : to.y + DIAGRAM.row / 2;
      points = clean([from, { x: from.x, y: mid }, { x: to.x, y: mid }, to]);
    }
    flows[flow.index] = {
      from: flow.from,
      to: flow.to,
      points,
      resting: false,
      quiet: !rows.forward.get(flow.index) || !!sideOf(shape, flow.to),
      back: !rows.forward.get(flow.index),
    };
  }
  return {
    way: "down",
    at,
    bounds: { x: 0, y: 0, width: left + widest, height: Math.max(0, y - DIAGRAM.row) },
    gutter: left,
    tiers: tiersOf(shape, rows, starts, H),
    flows,
    settled: false,
  };
}

/**
 * The rows turned into columns that read to the right, while the engine
 * answers: each column's parts stacked under each other, the tiers named over
 * their columns, each flow an elbow from a part's right edge to the next
 * part's left, a part under another joined straight down.
 */
function columnLayout(shape: DiagramShape, rows: DiagramRows): DiagramLayout {
  const { width: W, height: H } = DIAGRAM.node;
  const top = shape.lanes.length ? ROUTE.header : 0;
  const apart = 72;
  const stack = (row: readonly string[]) =>
    row.flatMap((id) => {
      const sides = rows.beside.get(id);
      return [...(sides?.before ? [sides.before] : []), id, ...(sides?.after ? [sides.after] : [])];
    });
  const tallest = Math.max(0, ...rows.rows.map((row) => stack(row).length));
  const at: Record<string, CanvasPosition> = {};
  const starts: number[] = [];
  let x = 0;
  rows.rows.forEach((row, index) => {
    if (index && rows.tierStart.has(index)) x += DIAGRAM.tier;
    starts.push(x);
    stack(row).forEach((id, place) => (at[id] = { x, y: top + place * (H + DIAGRAM.column) }));
    x += W + apart;
  });
  const flows: Record<number, DiagramFlow> = {};
  for (const flow of shape.flows) {
    const a = at[flow.from]!;
    const b = at[flow.to]!;
    let points: CanvasPosition[];
    if (a.x === b.x) {
      const down = a.y < b.y;
      points = [
        { x: a.x + W / 2, y: down ? a.y + H : a.y },
        { x: b.x + W / 2, y: down ? b.y : b.y + H },
      ];
    } else {
      const right = a.x < b.x;
      const from = { x: right ? a.x + W : a.x, y: a.y + H / 2 };
      const to = { x: right ? b.x : b.x + W, y: b.y + H / 2 };
      const mid = right ? to.x - apart / 2 : to.x + apart / 2;
      points = clean([from, { x: mid, y: from.y }, { x: mid, y: to.y }, to]);
    }
    flows[flow.index] = {
      from: flow.from,
      to: flow.to,
      points,
      resting: false,
      quiet: !rows.forward.get(flow.index) || !!sideOf(shape, flow.to),
      back: !rows.forward.get(flow.index),
    };
  }
  return {
    way: "right",
    at,
    bounds: {
      x: 0,
      y: 0,
      width: Math.max(0, x - apart),
      height: top + Math.max(0, tallest * (H + DIAGRAM.column) - DIAGRAM.column),
    },
    gutter: top,
    tiers: tiersOf(shape, rows, starts, W),
    flows,
    settled: false,
  };
}

const sideOf = (shape: DiagramShape, id: string) =>
  shape.parts.find((part) => part.id === id)?.side;

/** Each named tier along the way the picture reads: from its first row's start past its last. */
export function tiersOf(
  shape: DiagramShape,
  rows: DiagramRows,
  starts: readonly number[],
  depth: number,
): DiagramTier[] {
  if (!shape.lanes.length) return [];
  return rows.tiers.flatMap((tier) => {
    const name = shape.lanes[tier.lane];
    const start = starts[tier.first];
    const end = starts[tier.last];
    if (name === undefined || start === undefined || end === undefined) return [];
    return [{ name, start, extent: end + depth - start }];
  });
}

/** A line with no repeated points and no point in the middle of a straight run. */
export function clean(points: readonly CanvasPosition[]): CanvasPosition[] {
  const out: CanvasPosition[] = [];
  const same = (a: number, b: number) => Math.abs(a - b) < 0.01;
  for (const point of points) {
    const last = out.at(-1);
    if (last && same(last.x, point.x) && same(last.y, point.y)) continue;
    const before = out.at(-2);
    if (
      before &&
      last &&
      ((same(before.x, last.x) && same(last.x, point.x)) ||
        (same(before.y, last.y) && same(last.y, point.y)))
    )
      out.pop();
    out.push(point);
  }
  return out;
}

/** The point halfway along a line. */
export function midpoint(points: readonly CanvasPosition[]): CanvasPosition {
  const lengths = points.slice(1).map((p, i) => Math.hypot(p.x - points[i]!.x, p.y - points[i]!.y));
  let rest = lengths.reduce((sum, length) => sum + length, 0) / 2;
  for (let i = 0; i < lengths.length; i += 1) {
    const a = points[i]!;
    const b = points[i + 1]!;
    if (rest <= lengths[i]! && lengths[i]) {
      const t = rest / lengths[i]!;
      return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
    }
    rest -= lengths[i]!;
  }
  return points[0] ?? { x: 0, y: 0 };
}

/** A flow's line: rounded corners, stopping `short` before its target for the arrowhead. */
export function flowPath(points: readonly CanvasPosition[], short = 0, radius = 8): string {
  if (points.length < 2) return "";
  const end = points.at(-1)!;
  const last = points.at(-2)!;
  const run = Math.hypot(end.x - last.x, end.y - last.y) || 1;
  const cut = Math.min(short, run);
  const stop = {
    x: end.x - ((end.x - last.x) / run) * cut,
    y: end.y - ((end.y - last.y) / run) * cut,
  };
  const line = [...points.slice(0, -1), stop];
  let d = `M ${line[0]!.x},${line[0]!.y}`;
  for (let i = 1; i < line.length - 1; i += 1) {
    const [p, c, n] = [line[i - 1]!, line[i]!, line[i + 1]!];
    const inLength = Math.hypot(c.x - p.x, c.y - p.y);
    const outLength = Math.hypot(n.x - c.x, n.y - c.y);
    const r = Math.min(radius, inLength / 2, outLength / 2);
    if (!r) {
      d += ` L ${c.x},${c.y}`;
      continue;
    }
    const a = { x: c.x - ((c.x - p.x) / inLength) * r, y: c.y - ((c.y - p.y) / inLength) * r };
    const b = { x: c.x + ((n.x - c.x) / outLength) * r, y: c.y + ((n.y - c.y) / outLength) * r };
    d += ` L ${a.x},${a.y} Q ${c.x},${c.y} ${b.x},${b.y}`;
  }
  return `${d} L ${stop.x},${stop.y}`;
}

/** The arrowhead: `size` long, landing on the line's last point along its last segment. */
export function arrowHead(points: readonly CanvasPosition[], size = 5): string {
  const end = points.at(-1);
  const last = points.at(-2);
  if (!end || !last) return "";
  const run = Math.hypot(end.x - last.x, end.y - last.y) || 1;
  const [dx, dy] = [(end.x - last.x) / run, (end.y - last.y) / run];
  const base = { x: end.x - dx * size, y: end.y - dy * size };
  const half = size * 0.7;
  return `M ${end.x},${end.y} L ${base.x - dy * half},${base.y + dx * half} L ${base.x + dy * half},${base.y - dx * half} Z`;
}

const KEYS: Record<string, CanvasPosition> = {
  ArrowRight: { x: 1, y: 0 },
  ArrowLeft: { x: -1, y: 0 },
  ArrowDown: { x: 0, y: 1 },
  ArrowUp: { x: 0, y: -1 },
};
/**
 * The part an arrow key moves to from `from`: of the parts a flow joins it to,
 * the nearest that lies that way, a sideways step costing twice a straight one.
 */
export function partAlong(
  from: CanvasPosition,
  joined: readonly { id: string; at: CanvasPosition }[],
  key: string,
): string | null {
  const way = KEYS[key];
  if (!way) return null;
  let best: { id: string; cost: number } | null = null;
  for (const { id, at } of joined) {
    const dx = at.x - from.x;
    const dy = at.y - from.y;
    const along = dx * way.x + dy * way.y;
    if (along <= 0) continue;
    const cost = along + 2 * Math.abs(dx * way.y - dy * way.x);
    if (!best || cost < best.cost) best = { id, cost };
  }
  return best?.id ?? null;
}

// Layouts the engine settled, by shape; reading one makes a derivation wait for it.
const settled = new SvelteMap<string, DiagramLayout>();
const rowed = new Map<string, DiagramLayout>();
const asked = new Set<string>();
const pending = new Set<string>();
const KEPT = 48;
const keyOf = (shape: DiagramShape, way: DiagramWay) => `${way}${JSON.stringify(shape)}`;

/** The shape's names placed on the rows' lines: the layout at once, before the engine answers. */
function provisional(shape: DiagramShape, key: string, way: DiagramWay): DiagramLayout {
  let layout = rowed.get(key);
  if (!layout) {
    const rows = diagramRows(shape);
    layout = named(shape, way === "down" ? rowLayout(shape, rows) : columnLayout(shape, rows));
    if (rowed.size >= KEPT) {
      const evicted = rowed.keys().next().value!;
      rowed.delete(evicted);
      if (!settled.has(evicted) && !pending.has(evicted)) asked.delete(evicted);
    }
    rowed.set(key, layout);
  }
  return layout;
}

/** A layout with its flows' names placed where they read. */
function named(shape: DiagramShape, layout: DiagramLayout): DiagramLayout {
  const plates = placePlates(shape, layout);
  const flows: Record<number, DiagramFlow> = {};
  for (const [index, flow] of Object.entries(layout.flows)) {
    const plate = plates.get(Number(index));
    const { plate: _old, ...rest } = flow;
    flows[Number(index)] = plate
      ? { ...rest, plate: plate.at, resting: plate.resting }
      : { ...rest, resting: false };
  }
  return { ...layout, flows };
}

/**
 * A diagram's layout now: the engine's once it has answered, otherwise the
 * columns while it is asked (in a browser only), so a picture appears at once
 * and settles when the answer lands. A picture reads left to right unless
 * `way` holds it to reading down.
 */
export function diagramLayout(diagram: Diagram, way: DiagramWay = "right"): DiagramLayout {
  const shape = diagramShape(diagram);
  const key = keyOf(shape, way);
  const done = settled.get(key);
  if (done) return done;
  if (typeof window !== "undefined" && !asked.has(key)) void settle(shape, key, way);
  return provisional(shape, key, way);
}

/** A diagram laid out by the engine, off the main thread where the page allows it. */
export async function layoutDiagram(
  diagram: Diagram,
  way: DiagramWay = "right",
): Promise<DiagramLayout> {
  const shape = diagramShape(diagram);
  const key = keyOf(shape, way);
  return settled.get(key) ?? (await settle(shape, key, way));
}

async function settle(shape: DiagramShape, key: string, way: DiagramWay): Promise<DiagramLayout> {
  asked.add(key);
  if (!shape.parts.length) return provisional(shape, key, way);
  pending.add(key);
  try {
    const { engineLayout } = await import("./diagram-worker");
    const layout = named(shape, await engineLayout(shape, way));
    if (settled.size >= KEPT) {
      const evicted = settled.keys().next().value!;
      settled.delete(evicted);
      asked.delete(evicted);
    }
    settled.set(key, layout);
    return layout;
  } catch {
    // The rows stand for good; asking again would fail the same way.
    return provisional(shape, key, way);
  } finally {
    pending.delete(key);
    if (!settled.has(key) && !rowed.has(key)) asked.delete(key);
  }
}

const KINDS: Record<string, () => string> = {
  client: m.work_diagram_client,
  edge: m.work_diagram_edge,
  gateway: m.work_diagram_gateway,
  service: m.work_diagram_service,
  worker: m.work_diagram_worker,
  model: m.work_diagram_model,
  store: m.work_diagram_store,
  queue: m.work_diagram_queue,
  cache: m.work_diagram_cache,
  storage: m.work_diagram_storage,
  external: m.work_diagram_external,
};
/** What a part is, in a word; an unnamed kind says nothing. */
export const diagramKindLabel = (kind: string) => KINDS[kind]?.() ?? m.work_diagram_other();

/** The width a diagram draws at full size, with no scaling: what its object should be given. */
export const diagramWidth = (diagram: Diagram) => Math.ceil(diagramLayout(diagram).bounds.width);
