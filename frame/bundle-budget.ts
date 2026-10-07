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
