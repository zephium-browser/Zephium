export type Point = { x: number; y: number };

/** Elbows turn on a 12 px radius. */
const ELBOW = 12;

/**
 * An orthogonal polyline as an SVG path with rounded elbows. A corner takes
 * the whole of a segment that ends the line and half of one it shares with
 * another corner, so a short stub still turns on its full radius.
 */
export function roundedPath(points: readonly Point[], radius = ELBOW): string {
  const list = points.filter(
    (point, index) =>
      index === 0 || point.x !== points[index - 1]!.x || point.y !== points[index - 1]!.y,
  );
  if (!list.length) return "";
  const first = list[0]!;
  let d = `M ${first.x},${first.y}`;
  for (let index = 1; index < list.length - 1; index++) {
    const prev = list[index - 1]!;
    const at = list[index]!;
    const next = list[index + 1]!;
    const inLength = Math.hypot(at.x - prev.x, at.y - prev.y);
    const outLength = Math.hypot(next.x - at.x, next.y - at.y);
    const r = Math.min(
      radius,
      index === 1 ? inLength : inLength / 2,
      index === list.length - 2 ? outLength : outLength / 2,
    );
    const straight =
      (prev.x === at.x && at.x === next.x) || (prev.y === at.y && at.y === next.y) || r <= 0;
    if (straight) {
      d += ` L ${at.x},${at.y}`;
      continue;
    }
    const a = {
      x: at.x - ((at.x - prev.x) / inLength) * r,
      y: at.y - ((at.y - prev.y) / inLength) * r,
    };
    const b = {
      x: at.x + ((next.x - at.x) / outLength) * r,
      y: at.y + ((next.y - at.y) / outLength) * r,
    };
    d += ` L ${a.x},${a.y} Q ${at.x},${at.y} ${b.x},${b.y}`;
  }
  const last = list.at(-1)!;
  if (list.length > 1) d += ` L ${last.x},${last.y}`;
  return d;
}

/**
 * Lines from one point to several ends, as a fork: level ends straight
 * across, the rest up or down a trunk from the same branch point, each side
 * a bus whose later lines leave the trunk where the one before turned away,
 * so no two lines ever draw the same pixels.
 */
export function branch(start: Point, trunk: number, ends: readonly Point[]): Point[][] {
  const out: Point[][] = [];
  let stub = false;
  ends.forEach((end, index) => {
    if (end.y !== start.y) return;
    out[index] = [start, end];
    stub = true;
  });
  for (const direction of [-1, 1]) {
    const side = ends
      .map((end, index) => ({ end, index }))
      .filter(({ end }) => Math.sign(end.y - start.y) === direction)
      .sort((a, b) => direction * (a.end.y - b.end.y));
    let joint: Point | undefined;
    for (const { end, index } of side) {
      if (!joint) {
        const from = stub ? { x: trunk - ELBOW, y: start.y } : start;
        out[index] = [from, { x: trunk, y: start.y }, { x: trunk, y: end.y }, end];
        stub = true;
      } else out[index] = [joint, { x: trunk, y: end.y }, end];
      joint = { x: trunk, y: end.y - direction * ELBOW };
    }
  }
  return out;
}

/**
 * Lines from several starts into one end: the mirror of a fork, each line
 * running level to the collector and joining the trunk into the end.
 */
export function merge(starts: readonly Point[], collector: number, end: Point): Point[][] {
  return branch(
    { x: -end.x, y: end.y },
    -collector,
    starts.map((start) => ({ x: -start.x, y: start.y })),
  ).map((route) => route.map((point) => ({ x: -point.x, y: point.y })).reverse());
}

export type Side = "top" | "right" | "bottom" | "left";
const OUT: Record<Side, readonly [number, number]> = {
  top: [0, -1],
  right: [1, 0],
  bottom: [0, 1],
  left: [-1, 0],
};

/** A tie between two things nothing laid out: a cubic along their handles. */
export function tiePath(sx: number, sy: number, from: Side, tx: number, ty: number, to: Side) {
  const upright = from === "top" || from === "bottom";
  const along = Math.max(24, (upright ? Math.abs(ty - sy) : Math.abs(tx - sx)) / 2);
  const [ax, ay] = OUT[from];
  const [bx, by] = OUT[to];
  return `M ${sx},${sy} C ${sx + ax * along},${sy + ay * along} ${tx + bx * along},${ty + by * along} ${tx},${ty}`;
}

/** Between two ends that no longer stand where their layout put them: across, down, across. */
export function elbow(from: Point, to: Point): Point[] {
  if (Math.abs(from.y - to.y) < 1) return [from, to];
  if (Math.abs(from.x - to.x) < 1) return [from, to];
  const x = to.x > from.x + 24 ? Math.round((from.x + to.x) / 2) : from.x + 24;
  return to.x > from.x + 24
    ? [from, { x, y: from.y }, { x, y: to.y }, to]
    : [
        from,
        { x, y: from.y },
        { x, y: (from.y + to.y) / 2 },
        { x: to.x - 24, y: (from.y + to.y) / 2 },
        { x: to.x - 24, y: to.y },
        to,
      ];
}

/** A line as its layout routed it, and where it truly meets the corners of its two ends. */
export type Route = {
  points: readonly Point[];
  from: Point;
  to: Point;
  laid: { source: Point; target: Point };
};

/**
 * A routed line follows its route while both ends stand where the layout put
 * them, moves with them when they move together, and takes an elbow between
 * where they stand once one moves alone.
 */
export function routedPoints(route: Route, source?: Point, target?: Point): readonly Point[] {
  if (!source || !target) return route.points;
  const da = { x: source.x - route.laid.source.x, y: source.y - route.laid.source.y };
  const db = { x: target.x - route.laid.target.x, y: target.y - route.laid.target.y };
  const still = (d: Point) => Math.abs(d.x) < 0.5 && Math.abs(d.y) < 0.5;
  if (still(da) && still(db)) return route.points;
  if (Math.abs(da.x - db.x) < 0.5 && Math.abs(da.y - db.y) < 0.5)
    return route.points.map((point) => ({ x: point.x + da.x, y: point.y + da.y }));
  return elbow(
    { x: source.x + route.from.x, y: source.y + route.from.y },
    { x: target.x + route.to.x, y: target.y + route.to.y },
  );
}
