import type { HomeIntent, Place, Row } from "./model";

/** What Home's cards share with the canvas that draws them. */
export const homeTop = Symbol("home-top");

export type HomeTop = {
  intent(intent: HomeIntent): void;
  /** Brings a search to the front, drawn open; the one there before becomes a line. */
  open(search: string): void;
  /** Shows or hides what a menu row holds; it asks for nothing. */
  menu(search: string, row: string): void;
  shown(search: string, row: string): boolean;
  /** Shows or hides a place's full list of known sections. */
  toggle(place: string): void;
  expanded(place: string): boolean;
  /** A row the pointer rests on, to show what is already known of it; null as it leaves. */
  hover(row: Row | null, stop?: string): void;
  /** Where a kept picture is drawn from, if it still is. */
  picture(key: string): string | null;
  mark(place: Place): ImageData | null;
};
