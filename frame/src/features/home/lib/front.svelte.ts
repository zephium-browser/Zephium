/**
 * Which searches Home draws open. The newest opens of itself and closes
 * again when a newer one arrives; one the person opened stays open until
 * they open another. Showing a search centres it and opens it.
 */
export class Front {
  /** The newest search, open because it is the newest. */
  auto = $state<string | null>(null);
  /** The search the person opened, or native showed. */
  manual = $state<string | null>(null);
  /** The search the view last went to, counted so the same one can be asked for again. */
  centre = $state.raw<{ search: string; serial: number } | null>(null);
  private newest: string | null | undefined = undefined;
  private serial = 0;

  open = $derived([this.auto, this.manual].filter((id): id is string => id !== null));
  /** Where the keyboard and the first view go. */
  lead = $derived(this.manual ?? this.auto);

  /** The newest search as native lists it now; `held` names every search still drawn. */
  follow(newest: string | null, held: ReadonlySet<string>) {
    if (this.manual !== null && !held.has(this.manual)) this.manual = null;
    if (newest === this.newest) {
      if (this.auto !== null && !held.has(this.auto)) this.auto = newest;
      return false;
    }
    const first = this.newest === undefined;
    this.newest = newest;
    this.auto = newest;
    return !first && newest !== null;
  }

  /** The person opened a search: it is the one open, with the newest only while it stays newest. */
  choose(search: string) {
    this.manual = search;
    if (this.auto !== search) this.auto = null;
  }

  /** Native asked for a search to be shown: open it and go to it. */
  show(search: string) {
    if (search !== this.newest) this.manual = search;
    else this.auto = search;
    this.centre = { search, serial: ++this.serial };
  }
}
