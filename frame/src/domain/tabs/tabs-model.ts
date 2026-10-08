import type { ItemsState, TabView } from "$shared/ipc/bindings";

export const ZERO_PROJECTION_REVISION = "00000000000000000000000000000000";

export type PresentationTab = {
  tab: TabView;
  active: string | null;
};

export function initialItemsState(): ItemsState {
  return {
    projection_revision: ZERO_PROJECTION_REVISION,
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [],
    active: null,
    split_group: null,
  };
}

/**
 * Framework-free projection admission for privileged browser chrome.
 *
 * Rust emits fixed-width lowercase hexadecimal revisions from one monotonic
 * process-local sequence. Lexicographic ordering is therefore exact. Keep
 * this admission layer independent of Svelte so rendering migrations cannot
 * weaken stale-projection rejection.
 */
export class TabProjectionModel {
  #state: ItemsState;
  #appliedGlobalRevision: string;
  readonly #appliedTabRevisions = new Map<string, string>();

  constructor(initial: ItemsState = initialItemsState()) {
    this.#state = initial;
    this.#appliedGlobalRevision = initial.projection_revision;
    for (const tab of initial.tabs) {
      this.#appliedTabRevisions.set(tab.id, tab.projection_revision);
    }
  }

  get value(): ItemsState {
    return this.#state;
  }

  applySnapshot(candidate: ItemsState): boolean {
    if (candidate.projection_revision <= this.#appliedGlobalRevision) return false;

    this.#appliedGlobalRevision = candidate.projection_revision;
    for (const tab of candidate.tabs) {
      const previous = this.#appliedTabRevisions.get(tab.id);
      if (previous === undefined || tab.projection_revision > previous) {
        this.#appliedTabRevisions.set(tab.id, tab.projection_revision);
      }
    }
    // Native keeps an unchanged tab's revision; keeping its object too means
    // a tab switch re-renders only the rows that changed.
    const kept = new Map(this.#state.tabs.map((tab) => [tab.id, tab]));
    const tabs = candidate.tabs.map((tab) => {
      const previous = kept.get(tab.id);
      return previous?.projection_revision === tab.projection_revision ? previous : tab;
    });
    this.#state = { ...candidate, tabs };
    return true;
  }

  applyTab(tab: TabView): boolean {
    if (!this.#admitTabRevision(tab)) return false;

    const index = this.#state.tabs.findIndex((candidate) => candidate.id === tab.id);
    if (index !== -1) {
      const tabs = this.#state.tabs.slice();
      tabs[index] = tab;
      this.#state = { ...this.#state, tabs };
    }
    return true;
  }

  applyPresentation({ tab, active }: PresentationTab): boolean {
    if (!this.#admitTabRevision(tab)) return false;

    const index = this.#state.tabs.findIndex((candidate) => candidate.id === tab.id);
    const tabs = this.#state.tabs.slice();
    if (index !== -1) tabs[index] = tab;
    this.#state = { ...this.#state, tabs, active };
    return true;
  }

  #admitTabRevision(tab: TabView): boolean {
    const previous = this.#appliedTabRevisions.get(tab.id) ?? ZERO_PROJECTION_REVISION;
    if (tab.projection_revision <= previous) return false;

    this.#appliedTabRevisions.set(tab.id, tab.projection_revision);
    if (tab.projection_revision > this.#appliedGlobalRevision) {
      this.#appliedGlobalRevision = tab.projection_revision;
    }
    return true;
  }
}
