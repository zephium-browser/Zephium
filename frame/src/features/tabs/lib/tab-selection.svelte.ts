import { SvelteSet } from "svelte/reactivity";
import {
  emptySelection,
  plainSelection,
  pruneSelection,
  rangeSelection,
  toggleSelection,
} from "./tab-selection";

/** Owned by the mounted sidebar, never persisted or mirrored into the domain. */
export class TabSelection {
  value = $state.raw(emptySelection());
  #scope: string | undefined;

  clear() {
    this.value = emptySelection();
  }

  plain(id: string) {
    this.value = plainSelection(id);
  }

  toggle(id: string, active: string | null) {
    this.value = toggleSelection(this.value, id, active);
  }

  range(id: string, active: string | null, order: readonly string[]) {
    this.value = rangeSelection(this.value, id, active, order);
  }

  reconcile(scope: string, ids: readonly string[]) {
    if (scope !== this.#scope) this.clear();
    this.#scope = scope;
    this.value = pruneSelection(this.value, new SvelteSet(ids));
  }
}
