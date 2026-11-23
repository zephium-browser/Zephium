import type { HomeIntent, Place, Row } from "./model";

/** What Home's cards share with the canvas that draws them. */
export const homeTop = Symbol("home-top");

export type HomeTop = {
  intent(intent: HomeIntent): void;
  /** Shows or hides a place's full list of known sections. */
  toggle(place: string): void;
  expanded(place: string): boolean;
  /** A row the pointer rests on, to show what is already known of it; null as it leaves. */
  hover(row: Row | null, stop?: string): void;
  /** Where a kept picture is drawn from, if it still is. */
  picture(key: string): string | null;
  mark(place: Place): ImageData | null;
};
