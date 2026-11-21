import type { FolderColor } from "$shared/ipc/bindings";

/** A folder's colour as a token, or none for a plain folder. */
export const folderHue = (color: FolderColor | null | undefined) =>
  color ? `var(--color-soft-${color})` : undefined;

// The middle of a folder's glyph at its own depth, where its spine runs.
const spineAt = (depth: number) => 15.5 + depth * 13;

/** One hairline per folder around a row, outermost first, as background layers. */
export function spineLayers(spines: readonly (FolderColor | null)[]): string {
  return spines
    .map((color, depth) => {
      const ink = color
        ? `color-mix(in oklab, var(--color-soft-${color}) 55%, transparent)`
        : "var(--color-border)";
      return `linear-gradient(${ink}, ${ink}) ${spineAt(depth)}px 0 / 1px 100% no-repeat`;
    })
    .join(", ");
}
