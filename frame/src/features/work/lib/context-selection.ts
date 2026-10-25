import type {
  TabView,
  WorkContextSelectionV1,
  WorkEnvironmentSnapshot,
  WorkRuntimeProjection,
  NoteSummary,
} from "$shared/ipc/bindings";

const MAX_CONTEXT_ITEMS = 16;

/**
 * The selection as displayed, with the identity token Rust will recompute:
 * resource revision, URL digest, Work revision, or artifact id. Items whose token
 * is not yet known locally are left out rather than sent with a guess.
 */
export function contextSelection(
  snapshot: WorkEnvironmentSnapshot | null,
  selected: readonly string[],
  tabs: readonly TabView[],
  known: {
    notes: readonly NoteSummary[];
    objectives: ReadonlyMap<string, WorkRuntimeProjection>;
    /** Media record revisions: an image or a document is context like a note. */
    media?: ReadonlyMap<string, string>;
    objects?: ReadonlyMap<string, string>;
    urls?: ReadonlyMap<string, string>;
  },
  /** Consent, for this request, to list the window's open tabs: titles and addresses only. */
  openTabs = false,
  canvasBlocks = false,
): WorkContextSelectionV1 | null {
  canvasBlocks = canvasBlocks && selected.length === 0;
  if (!snapshot || (!selected.length && !openTabs && !canvasBlocks)) return null;
  const items: WorkContextSelectionV1["items"] = [];
  for (const id of selected) {
    if (items.length >= MAX_CONTEXT_ITEMS) break;
    const element = snapshot.elements.find((element) => element.id === id);
    if (!element) continue;
    const reference = element.reference;
    let revision: string | null = null;
    switch (reference.kind) {
      case "resource":
        revision =
          known.notes.find((note) => note.id === reference.resource)?.revision ??
          known.media?.get(reference.resource) ??
          known.objects?.get(reference.resource) ??
          null;
        break;
      case "browser": {
        const tab = tabs.find((tab) => tab.id === reference.tab);
        revision = tab ? (known.urls?.get(tab.url ?? "") ?? null) : null;
        break;
      }
      case "link":
        revision = known.urls?.get(reference.url) ?? null;
        break;
      case "objective":
        revision = known.objectives.get(reference.objective)?.work.revision ?? null;
        break;
      case "artifact":
      case "subject":
      case "finding":
      case "source":
        revision = reference.artifact;
        break;
    }
    if (revision !== null) items.push({ element: id, revision });
  }
  if (!items.length && !openTabs && !canvasBlocks) return null;
  return {
    environment: snapshot.id,
    items,
    ...(openTabs ? { tabs: true } : {}),
    ...(canvasBlocks ? { blocks: true } : {}),
  };
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
}

/** Concrete references only. Rust independently checks the request and kind. */
export function referencesCanvasBlock(request: string): boolean {
  const kinds = new Set(
    [...request.toLowerCase().matchAll(/\b(?:this|my|the)\s+(checklist|table|calendar)\b/g)].map(
      (match) => match[1],
    ),
  );
  return kinds.size === 1;
}
