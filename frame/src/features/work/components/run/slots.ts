import type { Component } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import type { PartView } from "../../lib/canvas-model";

/**
 * What a helper's own view of its work receives in a part's row: the part
 * and where its steps are (the facts come from the `canvasWork` context).
 */
export type PartContentProps = {
  id: string;
  part: PartView;
  objective: string;
  steps: readonly string[];
};

type Loader<P extends Record<string, unknown>> = () => Promise<{ default: Component<P> }>;
// Found at build time; a view that doesn't exist yet leaves its slot to the fallback.
const contents = import.meta.glob<{ default: Component<PartContentProps> }>([
  "../parts/ComputerPart.svelte",
  "../parts/ConnectionPart.svelte",
]);
const asks = import.meta.glob<{ default: Component<Record<string, unknown>> }>(
  "../asks/AskCard.svelte",
);

const CONTENT: Partial<Record<PartView["helper"], string>> = {
  computer: "../parts/ComputerPart.svelte",
  connection: "../parts/ConnectionPart.svelte",
};

export function partContent(helper: PartView["helper"]): Loader<PartContentProps> | null {
  const path = CONTENT[helper];
  return (path && contents[path]) || null;
}

/** The ask card, once its stream has built it; it takes the ask's own props. */
export function askCard(): Loader<Record<string, unknown>> | null {
  return asks["../asks/AskCard.svelte"] ?? null;
}

/**
 * Each view, once its module has loaded, so a row draws it at once. A row that
 * awaited the import itself awaited a fresh promise each time the board measured
 * it, and lost its view meanwhile: the view was built again every time, and the
 * row's height flickered with it.
 */
const loaded = new SvelteMap<() => Promise<unknown>, unknown>();
const loading = new Set<() => Promise<unknown>>();

/** The view a loader gives, once it has loaded; null until then. */
export function loadedView<P extends Record<string, unknown>>(
  loader: Loader<P> | null,
): Component<P> | null {
  return loader ? ((loaded.get(loader) as Component<P> | undefined) ?? null) : null;
}

/** Starts loading a view, once; a failed load can be tried again. */
export function loadView<P extends Record<string, unknown>>(loader: Loader<P> | null): void {
  if (!loader || loaded.has(loader) || loading.has(loader)) return;
  loading.add(loader);
  void loader().then(
    (module) => {
      loading.delete(loader);
      loaded.set(loader, module.default);
    },
    (error: unknown) => {
      loading.delete(loader);
      throw error;
    },
  );
}
