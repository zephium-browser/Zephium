/**
 * Where a row dropped among nested rows lands. The gap it waits in bounds how
 * deep it can go: no shallower than the row below (it would split that row's
 * folder) and no deeper than inside the row above, when that is a folder.
 * Within those bounds the pointer's sideways travel picks the level, one
 * indent at a time, so the same gap can mean "after this folder" or "at the
 * end of it".
 */

export const INDENT = 13;
/** As the shell's MAX_FOLDER_DEPTH: folder levels, the outermost counted. */
export const FOLDER_LEVELS = 3;

export type TreeRow = { id: string; depth: number; folder: boolean };

export type TreeBlock = {
  depth: number;
  folder: boolean;
  /** For a folder, the levels it holds, itself counted. */
  levels: number;
  /** Whether it may go into a folder at all. */
  fileable: boolean;
};

export type TreePlace = { depth: number; parent: string | null; before: string | null };

/** `rows` are the rows shown, the dragged ones left out; `index` the gap among them. */
export function treePlace(
  rows: readonly TreeRow[],
  index: number,
  block: TreeBlock,
  dx: number,
): TreePlace | null {
  const above = rows[index - 1];
  const below = rows[index];
  const min = below?.depth ?? 0;
  const deepest = above === undefined ? 0 : above.folder ? above.depth + 1 : above.depth;
  const cap = block.folder ? FOLDER_LEVELS - block.levels : block.fileable ? FOLDER_LEVELS : 0;
  const max = Math.min(deepest, cap);
  if (max < min) return null;
  const depth = Math.max(min, Math.min(max, block.depth + Math.round(dx / INDENT)));
  let parent: string | null = null;
  if (depth > 0) {
    let at = index - 1;
    while (at >= 0 && rows[at]!.depth !== depth - 1) at -= 1;
    const holder = rows[at];
    if (holder === undefined || !holder.folder) return null;
    parent = holder.id;
  }
  return { depth, parent, before: below !== undefined && below.depth === depth ? below.id : null };
}

/** The rows a reorder passes, read from the list's own markup. */
export function treeRows(elements: readonly HTMLElement[]): TreeRow[] {
  return elements.map((element) => ({
    id: element.dataset.nodeId ?? "",
    depth: Number(element.dataset.depth ?? 0),
    folder: element.dataset.folder !== undefined,
  }));
}
