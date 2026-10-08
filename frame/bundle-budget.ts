export type BundleSize = { js: number; css: number };

/**
 * Growth allowed over a recorded size before the build fails. Small enough to
 * catch a new dependency or an eagerly imported feature, large enough that
 * ordinary work (strings, bindings, a component) does not need a new baseline.
 */
export const GROWTH_ALLOWANCE: BundleSize = { js: 16 * 1024, css: 8 * 1024 };

/** What a lazy destination may add before it needs a recorded size of its own. */
export const UNRECORDED_LAZY_LIMIT: BundleSize = { js: 48 * 1024, css: 16 * 1024 };

/**
 * `size` is a page's whole startup graph, or what a lazy destination adds on
 * top of the page that opens it.
 */
export function checkBundleBudget(
  name: string,
  size: BundleSize,
  recorded: BundleSize | undefined,
): string | null {
  const lazy = name.startsWith("lazy:");
  if (!recorded && !lazy) return `${name} has no recorded size; run pnpm run budgets`;
  for (const kind of ["js", "css"] as const) {
    const limit = recorded ? recorded[kind] + GROWTH_ALLOWANCE[kind] : UNRECORDED_LAZY_LIMIT[kind];
    if (
      !Number.isSafeInteger(limit) ||
      limit < 0 ||
      !Number.isSafeInteger(size[kind]) ||
      size[kind] < 0
    )
      return `${name} has an invalid ${kind.toUpperCase()} size`;
    if (size[kind] > limit)
      return recorded
        ? `${name} ${kind.toUpperCase()} grew to ${size[kind]} bytes, over ${recorded[kind]} recorded plus ${GROWTH_ALLOWANCE[kind]} allowed`
        : `${name} adds ${size[kind]} ${kind.toUpperCase()} bytes with no recorded size (limit ${limit})`;
  }
  return null;
}

type Chunk = { imports: readonly string[]; dynamicImports: readonly string[] };
type Root = {
  name: string;
  file: string;
  surface: boolean;
  /** Chunks in the root's static graph. */
  visited: ReadonlySet<string>;
  /** Those chunks and their stylesheets. */
  files: ReadonlySet<string>;
};

/**
 * The files certainly loaded before each root runs. A page starts with
 * nothing. A lazy destination has whatever every way of opening it has
 * loaded: the root holding the chunk that imports it, and what that root
 * had before it. Destinations no page can reach get nothing.
 */
export function loadedBeforeOpening(
  chunks: Readonly<Record<string, Chunk>>,
  roots: readonly Root[],
): Map<string, Set<string>> {
  const openable = new Set<string>();
  const open = (file: string) => {
    if (openable.has(file)) return;
    openable.add(file);
    const chunk = chunks[file];
    if (chunk) for (const child of [...chunk.imports, ...chunk.dynamicImports]) open(child);
  };
  for (const root of roots) if (root.surface) open(root.file);
  const holders = new Map<string, Root[]>();
  for (const root of roots)
    for (const file of root.visited) holders.set(file, [...(holders.get(file) ?? []), root]);
  const everything = new Set(Object.keys(chunks).concat(roots.flatMap((root) => [...root.files])));
  const before = new Map<string, Set<string>>(
    roots.map((root) => [
      root.name,
      root.surface || !openable.has(root.file) ? new Set<string>() : everything,
    ]),
  );
  // Greatest fixpoint: start from everything and narrow to what every way
  // of opening a destination has certainly loaded.
  const lazy = roots.filter((root) => !root.surface && openable.has(root.file));
  for (let changed = true; changed;) {
    changed = false;
    for (const root of lazy) {
      let loaded: Set<string> | undefined;
      for (const [file, chunk] of Object.entries(chunks)) {
        if (!chunk.dynamicImports.includes(root.file)) continue;
        for (const holder of holders.get(file) ?? []) {
          const available = new Set([...(before.get(holder.name) ?? []), ...holder.files]);
          loaded = loaded
            ? new Set([...loaded].filter((entry) => available.has(entry)))
            : available;
        }
      }
      loaded ??= new Set();
      if (loaded.size !== before.get(root.name)?.size) {
        before.set(root.name, loaded);
        changed = true;
      }
    }
  }
  return before;
}
