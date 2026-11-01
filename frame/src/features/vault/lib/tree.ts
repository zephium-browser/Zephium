import type { VaultEntry } from "$shared/ipc/bindings";

export type Row = { entry: VaultEntry; depth: number; expanded: boolean; loading: boolean };

/** The rows a tree shows: every listed folder's children under each expanded folder. */
export function visibleRows(
  listings: ReadonlyMap<string, readonly VaultEntry[]>,
  expanded: ReadonlySet<string>,
): Row[] {
  const rows: Row[] = [];
  const walk = (dir: string, depth: number) => {
    for (const entry of listings.get(dir) ?? []) {
      const folder = entry.kind === "folder";
      const open = folder && expanded.has(entry.path);
      rows.push({ entry, depth, expanded: open, loading: open && !listings.has(entry.path) });
      if (open) walk(entry.path, depth + 1);
    }
  };
  walk("", 0);
  return rows;
}

/** Every ancestor folder of a path, nearest last ("a", "a/b" for "a/b/c.pdf"). */
export function ancestors(path: string): string[] {
  const parts = path.split("/");
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join("/"));
}

/** A name that keeps a file's extension when only the stem was edited. */
export function renamedWithExtension(original: string, typed: string): string {
  const trimmed = typed.trim();
  const at = original.lastIndexOf(".");
  if (at <= 0 || trimmed.includes(".")) return trimmed;
  return trimmed + original.slice(at);
}
