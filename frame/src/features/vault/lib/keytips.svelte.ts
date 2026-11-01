/**
 * Excel-style key tips: tap Alt and every control marked `data-keytip` shows
 * its letter; press the letter to use it, Escape or Alt again to put them away.
 */
export type Tip = { key: string; left: number; top: number; element: HTMLElement };

export class KeyTips {
  tips = $state.raw<Tip[]>([]);
  #altAlone = false;

  get active() {
    return this.tips.length > 0;
  }

  show(root: HTMLElement) {
    const origin = root.getBoundingClientRect();
    const focusedPane = root.querySelector(".pane.focused");
    const tips: Tip[] = [];
    const seen: Record<string, true> = {};
    for (const element of root.querySelectorAll<HTMLElement>("[data-keytip]")) {
      const pane = element.closest(".pane");
      if (pane && focusedPane && pane !== focusedPane) continue;
      const box = element.getBoundingClientRect();
      if (box.width === 0 || box.height === 0) continue;
      const key = (element.dataset.keytip ?? "").toUpperCase();
      if (!key || seen[key]) continue;
      seen[key] = true;
      tips.push({
        key,
        left: box.left - origin.left + 2,
        top: box.bottom - origin.top - 8,
        element,
      });
    }
    this.tips = tips;
  }

  hide() {
    this.tips = [];
  }

  /** Handles a key press; true when the key was a key-tip key. */
  keydown(event: KeyboardEvent, root: HTMLElement | null): boolean {
    if (event.key === "Alt") {
      this.#altAlone = !event.repeat;
      return false;
    }
    this.#altAlone = false;
    if (!this.active) return false;
    if (event.key === "Escape") {
      this.hide();
      return true;
    }
    const tip = this.tips.find((candidate) => candidate.key === event.key.toUpperCase());
    this.hide();
    if (!tip) return true;
    if (tip.element instanceof HTMLInputElement) tip.element.focus();
    else tip.element.click();
    void root;
    return true;
  }

  /** Alt released without another key in between toggles the tips. */
  keyup(event: KeyboardEvent, root: HTMLElement | null): boolean {
    if (event.key !== "Alt" || !this.#altAlone || !root) return false;
    this.#altAlone = false;
    if (this.active) this.hide();
    else this.show(root);
    return true;
  }
}
