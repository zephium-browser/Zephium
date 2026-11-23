import type { BoardLayout, Rect } from "../board/layout";
import { branch, digest, merge, snap, type Point } from "$shared/canvas";

/**
 * A run on the 8 px grid, left to right: the inputs hang left of the request
 * at x = 0, then the run's work in the order it happened, one column per
 * stretch of time (work that overlapped stands together), each 96 past the
 * last, and the result 96 past the last column. The request, every column
 * and the result's headline begin level at the run's top and grow downward, so
 * a tall result never moves the work and tall work never moves the request;
 * parallel work forks from the request's line, each column's work merges and
 * forks on into the next, and the last merges into the result. Its Sources
 * stand under its result.
 */
export const RUN = {
  inputs: 200,
  request: 320,
  gutter: 48,
  /** From the request to its parts: the fork's trunk stands halfway. */
  fork: 96,
  /** Between part rows: room for each helper's line over its part. */
  rowGap: 48,
  between: 120,
  /** From the widest part row to the result: room for the lines to merge. */
  feed: 96,
  found: 32,
  foundGap: 24,
  /** A row's finds wrap past this width, so the result stays in view. */
  foundRow: 1080,
  /** The Sources under the result. */
  sources: 40,
  spine: 32,
  /** A part's name sits 12 under its row's top. */
  labelMid: 12,
  /** A part's mark stands centred this far in from its row's left. */
  markMid: 7,
  /** The answer's headline centre, under the result's top. */
  resultMid: 14,
  /** Lines stop short of what they join. */
  air: 6,
  head: 16,
} as const;

type Sized = { id: string; width: number; height: number };
type RunRow = {
  part: Sized;
  /** What the part found, in the order it found it, at the end of its row. */
  found: readonly Sized[];
  /** The part's work reaches the result. */
  feeds: boolean;
  /** Its column: later work stands further right; one not yet placed stands in the newest. */
  phase?: number;
  /** The row just above it whose work it carries on: joined down from it, not forked to beside it. */
  after?: string;
};
export type RunInputs = {
  /** The request, and where its first line of words ends, when that is short of its column. */
  request: Sized & { end?: number };
  inputs?: readonly Sized[];
  rows: readonly RunRow[];
  head?: Sized;
  board: BoardLayout;
  /** What the run drew on, under its result. */
  sources?: Sized;
  /** Blocks the person moved, by their offset from the result's corner. */
  pins?: ReadonlyMap<string, Rect>;
};
type RunLine = {
  id: string;
  kind: "part" | "feed" | "input" | "found";
  source: string;
  target: string;
  /** The route as laid out, in canvas coordinates. */
  points: Point[];
  /** Where the line truly leaves and lands, relative to its two nodes' corners. */
  from: Point;
  to: Point;
  /** Where the run put its two nodes' corners. */
  laid: { source: Point; target: Point };
};
export type RunPlace = {
  /** Where the request's first line ends: its lines leave from here. */
  requestEnd: number;
  rects: Record<string, Rect>;
  /** The result's blocks, under its head. */
  board: Rect;
  /** The result's top-left: a moved block is kept relative to it. */
  corner: Point;
  /** The bottom of whatever the run holds; the next run starts 120 under it. */
  extent: number;
  /** Everything the run covers, for keeping the person's own things clear of it. */
  box: Rect;
  lines: RunLine[];
};

const ORIGIN = { source: { x: 0, y: 0 }, target: { x: 0, y: 0 } };

type RowEnd = {
  row: RunRow;
  end: Point;
  last: string;
  lastRect: Rect;
  /** The row above carries on into it: no fork reaches it. */
  joined: boolean;
  /** The row below carries it on: its line leaves through that row. */
  carried: boolean;
};

/** One column of rows from its top-left: where each row and what it found stand. */
function placeColumn(rows: readonly RunRow[], x: number, top: number) {
  const rects: Record<string, Rect> = {};
  const lines: RunLine[] = [];
  const ends: RowEnd[] = [];
  const labels: number[] = [];
  let y = top;
  let right = x;
  for (const [index, row] of rows.entries()) {
    const rowTop = y;
    const above = rows[index - 1];
    const joined = !!above && row.after === above.part.id;
    if (joined) {
      const from = rects[above.part.id]!;
      const at = x + RUN.markMid;
      lines.push({
        id: `line:${row.part.id}`,
        kind: "part",
        source: above.part.id,
        target: row.part.id,
        points: [
          { x: at, y: from.y + from.height + RUN.air },
          { x: at, y: rowTop - RUN.air },
        ],
        from: { x: RUN.markMid, y: from.height + RUN.air },
        to: { x: RUN.markMid, y: -RUN.air },
        laid: ORIGIN,
      });
    }
    rects[row.part.id] = { x, y: rowTop, width: row.part.width, height: row.part.height };
    labels.push(rowTop + RUN.labelMid);
    const start = snap(x + row.part.width + RUN.found);
    let at = start;
    let lineTop = rowTop;
    let lineHeight = 0;
    let height = row.part.height;
    let last = row.part.id;
    let farthest = x + row.part.width;
    // What a part found runs along its row, and wraps under itself rather than pushing the result off the view.
    for (const found of row.found) {
      if (at > start && at + found.width - start > RUN.foundRow) {
        lineTop = snap(lineTop + lineHeight + RUN.foundGap);
        at = start;
        lineHeight = 0;
      }
      rects[found.id] = { x: at, y: lineTop, width: found.width, height: found.height };
      if (lineTop === rowTop) last = found.id;
      farthest = Math.max(farthest, at + found.width);
      at = snap(at + found.width + RUN.foundGap);
      lineHeight = Math.max(lineHeight, found.height);
      height = Math.max(height, lineTop - rowTop + found.height);
    }
    const first = row.found[0] ? rects[row.found[0].id] : undefined;
    // The part's work is joined to what it found, straight along the row.
    if (first) {
      const ly = rowTop + RUN.labelMid;
      lines.push({
        id: `found:${row.part.id}`,
        kind: "found",
        source: row.part.id,
        target: row.found[0]!.id,
        points: [
          { x: x + row.part.width + RUN.air, y: ly },
          { x: first.x - RUN.air, y: ly },
        ],
        from: { x: row.part.width + RUN.air, y: RUN.labelMid },
        to: { x: -RUN.air, y: RUN.labelMid },
        laid: ORIGIN,
      });
    }
    const lastRect = rects[last]!;
    right = Math.max(right, farthest);
    ends.push({
      row,
      end: { x: lastRect.x + lastRect.width + RUN.air, y: rowTop + RUN.labelMid },
      last,
      lastRect,
      joined,
      carried: rows[index + 1]?.after === row.part.id,
    });
    y = snap(rowTop + height + RUN.rowGap);
  }
  return { rects, lines, ends, labels, width: right - x, bottom: y - RUN.rowGap };
}

export function placeRun(top: number, run: RunInputs): RunPlace {
  const rects: Record<string, Rect> = {};
  const lines: RunLine[] = [];
  const partsX = RUN.request + RUN.fork;
  const known = run.rows.flatMap((row) => (row.phase === undefined ? [] : [row.phase]));
  // A row whose column is not known yet stands in the newest, never back at the first.
  const newest = known.length ? Math.max(...known) : 0;
  const phaseOf = (row: RunRow) => row.phase ?? newest;
  const phases = [...new Set(run.rows.map(phaseOf))].sort((a, b) => a - b);
  const groups = phases.map((phase) => run.rows.filter((row) => phaseOf(row) === phase));
  // Every column and the request begin at the run's top; the result starts there too.
  const first = top + RUN.spine - RUN.labelMid;
  const spine = top + RUN.spine;
  const columns: (ReturnType<typeof placeColumn> & { x: number })[] = [];
  let x = partsX;
  groups.forEach((rows) => {
    const column = placeColumn(rows, x, first);
    Object.assign(rects, column.rects);
    lines.push(...column.lines);
    columns.push({ ...column, x });
    x = snap(x + column.width + RUN.fork);
  });
  const labels = columns.flatMap((column) => column.labels);
  const partsBottom = columns.length ? Math.max(...columns.map((column) => column.bottom)) : top;
  const lastColumn = columns.at(-1);
  const request = { x: 0, y: spine - RUN.spine, width: RUN.request, height: run.request.height };
  // Lines leave from the end of the words, never from the column's edge past a short request.
  const requestEnd = Math.min(RUN.request, run.request.end ?? RUN.request);
  rects[run.request.id] = request;

  let inputsBottom = top;
  const inputs = run.inputs ?? [];
  if (inputs.length) {
    const x = -(RUN.inputs + RUN.gutter);
    const tall = inputs.reduce((sum, input) => sum + input.height, 0) + (inputs.length - 1) * 8;
    let y = Math.max(top, spine - tall / 2);
    const ends: Point[] = [];
    for (const input of inputs) {
      rects[input.id] = { x, y, width: input.width, height: input.height };
      ends.push({ x: x + input.width + RUN.air, y: y + input.height / 2 });
      y += input.height + 8;
    }
    inputsBottom = y;
    const into = { x: -RUN.air, y: spine };
    merge(ends, -RUN.gutter / 2, into).forEach((points, index) => {
      const input = inputs[index]!;
      const rect = rects[input.id]!;
      lines.push({
        id: `input:${input.id}`,
        kind: "input",
        source: input.id,
        target: run.request.id,
        points,
        from: { x: rect.width + RUN.air, y: rect.height / 2 },
        to: { x: -RUN.air, y: RUN.spine },
        laid: ORIGIN,
      });
    });
  }

  // Without parts the answer stands a fork's width past the words, joined to them by one line.
  const resultX = lastColumn
    ? snap(lastColumn.x + lastColumn.width + RUN.feed)
    : snap(requestEnd + RUN.fork);
  const resultTop = (labels.length ? Math.min(spine, ...labels) : spine) - RUN.resultMid;
  let blocks = resultTop;
  if (run.head) {
    rects[run.head.id] = {
      x: resultX,
      y: resultTop,
      width: run.head.width,
      height: run.head.height,
    };
    blocks = resultTop + run.head.height + RUN.head;
  }
  for (const [id, rect] of Object.entries(run.board.at))
    rects[id] = { ...rect, x: rect.x + resultX, y: rect.y + blocks };
  const resultBottom = blocks + run.board.height - (run.head && !run.board.height ? RUN.head : 0);
  const resultId = run.head?.id ?? Object.keys(run.board.at)[0];
  let resultRight = resultX + Math.max(run.head?.width ?? 0, run.board.width);

  // The request's line branches to the head of every row of the first column a fork reaches.
  const start = { x: requestEnd + RUN.air, y: spine };
  const firstRows = (columns[0]?.ends ?? []).filter((entry) => !entry.joined);
  const partEnds = firstRows.map((entry) => ({ x: partsX - RUN.air, y: entry.end.y }));
  branch(start, RUN.request + RUN.fork / 2, partEnds).forEach((points, index) => {
    const { row } = firstRows[index]!;
    lines.push({
      id: `line:${row.part.id}`,
      kind: "part",
      source: run.request.id,
      target: row.part.id,
      points,
      from: { x: requestEnd + RUN.air, y: RUN.spine },
      to: { x: -RUN.air, y: RUN.labelMid },
      laid: ORIGIN,
    });
  });
  // Each column's work merges on the spine and forks on into the next column.
  columns.slice(1).forEach((column, index) => {
    const leaving = columns[index]!.ends.filter((entry) => !entry.carried);
    const reached = column.ends.filter((entry) => !entry.joined);
    const junction = { x: column.x - RUN.fork / 2, y: spine };
    const merged = merge(
      leaving.map((entry) => entry.end),
      junction.x,
      junction,
    );
    const forked = branch(
      junction,
      junction.x,
      reached.map((entry) => ({ x: column.x - RUN.air, y: entry.end.y })),
    );
    leaving.forEach((entry, a) => {
      reached.forEach((next, b) => {
        lines.push({
          id: `step:${digest(entry.row.part.id)}:${digest(next.row.part.id)}`,
          kind: "part",
          source: entry.last,
          target: next.row.part.id,
          points: [...merged[a]!, ...forked[b]!.slice(1)],
          from: { x: entry.lastRect.width + RUN.air, y: entry.end.y - entry.lastRect.y },
          to: { x: -RUN.air, y: RUN.labelMid },
          laid: ORIGIN,
        });
      });
    });
  });
  if (resultId) {
    const result = rects[resultId]!;
    const into = { x: resultX - RUN.air, y: resultTop + RUN.resultMid };
    const to = { x: -RUN.air, y: into.y - result.y };
    // The last column's work reaches the result; earlier columns reach it through the later ones.
    const leaving = lastColumn?.ends.filter((entry) => !entry.carried) ?? [];
    const last = leaving.filter((entry) => entry.row.feeds);
    // A line never runs level through a column to reach an earlier one's work, so a last column that feeds nothing stands for the run's end.
    const feeding = last.length ? last : leaving;
    const ends = feeding.map((entry) => entry.end);
    merge(ends, resultX - RUN.feed / 2, into).forEach((points, index) => {
      const entry = feeding[index]!;
      lines.push({
        id: `feed:${entry.row.part.id}`,
        kind: "feed",
        source: entry.last,
        target: resultId,
        points,
        from: {
          x: entry.lastRect.width + RUN.air,
          y: entry.end.y - entry.lastRect.y,
        },
        to,
        laid: ORIGIN,
      });
    });
    if (!run.rows.length)
      lines.push({
        id: `line:${run.request.id}:${digest(resultId)}`,
        kind: "part",
        source: run.request.id,
        target: resultId,
        points: [start, into],
        from: { x: requestEnd + RUN.air, y: RUN.spine },
        to,
        laid: ORIGIN,
      });
  }

  let sourcesBottom = resultBottom;
  if (run.sources) {
    // Under the result; a run that came to no result keeps them under its parts.
    const bare = !resultId && run.rows.length;
    const x = bare ? partsX : resultX;
    const y = snap((bare ? partsBottom : resultBottom) + RUN.sources);
    rects[run.sources.id] = { x, y, width: run.sources.width, height: run.sources.height };
    sourcesBottom = y + run.sources.height;
    resultRight = Math.max(resultRight, x + run.sources.width);
  }
  let extent = Math.max(
    request.y + run.request.height,
    inputsBottom,
    partsBottom,
    resultBottom,
    sourcesBottom,
  );
  for (const [id, pin] of run.pins ?? []) {
    rects[id] = { ...pin, x: resultX + pin.x, y: resultTop + pin.y };
    extent = Math.max(extent, resultTop + pin.y + pin.height);
    resultRight = Math.max(resultRight, resultX + pin.x + pin.width);
  }
  for (const line of lines) {
    const a = rects[line.source]!;
    const b = rects[line.target]!;
    line.laid = { source: { x: a.x, y: a.y }, target: { x: b.x, y: b.y } };
  }
  const left = inputs.length ? -(RUN.inputs + RUN.gutter) : 0;
  return {
    rects,
    requestEnd,
    board: { x: resultX, y: blocks, width: run.board.width, height: run.board.height },
    corner: { x: resultX, y: resultTop },
    extent,
    box: { x: left, y: top, width: resultRight - left, height: extent - top },
    lines,
  };
}
