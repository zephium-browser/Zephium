import type { Place, Row } from "$domain/home";
import type { HomeIntent } from "./intent";

/** What Home's cards share with the canvas that draws them. */
export const homeTop = Symbol("home-top");

export type HomeTop = {
  intent(intent: HomeIntent): void;
  /** Opens a search that is drawn as a line; a newer one arriving leaves it open. */
  open(search: string): void;
  /** Shows or hides what a menu row holds; it asks for nothing. */
  menu(search: string, row: string): void;
  shown(search: string, row: string): boolean;
  /** Shows the rest of what a menu row holds. */
  menuAll(search: string, row: string): void;
  /** Shows a search's other results past the first few. */
  all(search: string): void;
  /** Shows or hides a place's full list of known sections. */
  toggle(place: string): void;
  expanded(place: string): boolean;
  /** The row a page was opened from, marked on return to its search. */
  marked(search: string, row: string): boolean;
  /** A row the pointer rests on, to show what is already known of it; null as it leaves. */
  hover(row: Row | null, stop?: string): void;
  /** The icon of the place's page open in a tab, if one is. */
  mark(place: Place): ImageData | null;
};
