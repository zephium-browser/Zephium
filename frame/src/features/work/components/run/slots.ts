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

/** How long a failed load waits before each retry; after the last it gives up. */
const RETRY_MS = [500, 2000] as const;

/**
 * Stylesheets that failed while a view's module was importing, by view. Vite
 * never requests one again, so a retried view would load without its styles;
 * its retry requests its own failed stylesheets first.
 */
const failedSheets = new Map<() => Promise<unknown>, Set<HTMLLinkElement>>();
const importing = new Set<() => Promise<unknown>>();
let watchingSheets = false;

function watchSheets(): void {
  if (watchingSheets || typeof document === "undefined") return;
  watchingSheets = true;
  document.addEventListener(
    "error",
    (event) => {
      const link = event.target;
      if (!(link instanceof HTMLLinkElement) || link.rel !== "stylesheet") return;
      for (const loader of importing) {
        const sheets = failedSheets.get(loader) ?? new Set();
        sheets.add(link);
        failedSheets.set(loader, sheets);
      }
    },
    true,
  );
}

function reloadSheets(loader: () => Promise<unknown>): Promise<unknown> {
  const sheets = failedSheets.get(loader);
  if (!sheets) return Promise.resolve();
  return Promise.all(
    [...sheets].map(
      (failed) =>
        new Promise<void>((resolve, reject) => {
          const link = failed.cloneNode() as HTMLLinkElement;
          link.addEventListener(
            "load",
            () => {
              for (const [owner, owned] of failedSheets) {
                owned.delete(failed);
                if (owned.size === 0) failedSheets.delete(owner);
              }
              failed.remove();
              resolve();
            },
            { once: true },
          );
          link.addEventListener(
            "error",
            () => {
              link.remove();
              reject(new Error(`Unable to load ${link.href}`));
            },
            { once: true },
          );
          document.head.append(link);
        }),
    ),
  );
}

/**
 * Starts loading a view, once. A failed load is retried on its own, since a
 * mounted row asks again only when its loader changes; after the last retry a
 * later row can try again.
 */
export function loadView<P extends Record<string, unknown>>(loader: Loader<P> | null): void {
  if (!loader || loaded.has(loader) || loading.has(loader)) return;
  watchSheets();
  loading.add(loader);
  const attempt = async (tries: number): Promise<void> => {
    try {
      importing.add(loader);
      const module = await loader().finally(() => importing.delete(loader));
      if (tries > 0) await reloadSheets(loader);
      loading.delete(loader);
      loaded.set(loader, module.default);
    } catch (error) {
      const wait = RETRY_MS[tries];
      if (wait === undefined) {
        loading.delete(loader);
        throw error;
      }
      setTimeout(() => void attempt(tries + 1), wait);
    }
  };
  void attempt(0);
}
