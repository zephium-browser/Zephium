import type { HomeModel, Search } from "./model";

export type HomeSuggestion =
  | { kind: "row"; search: string; entry: string; label: string; place: string }
  | { kind: "search"; search: string; text: string };

/** What the field asks of Home while Home is drawn: what it holds that answers the words, and running one. */
export type HomeField = {
  suggest(query: string): HomeSuggestion[];
  run(suggestion: HomeSuggestion): void;
};

export const MAX_ROW_SUGGESTIONS = 4;
export const MAX_SEARCH_SUGGESTIONS = 3;
const MAX_QUERY = 200;

const fold = (text: string) => text.normalize("NFKC").toLocaleLowerCase().trim();

/**
 * What Home already holds that answers what is typed: rows of the search in
 * front, then earlier searches. Matched locally, asked of nothing, bounded.
 */
export function suggest(home: HomeModel, front: string | null, query: string): HomeSuggestion[] {
  const wanted = fold(query.slice(0, MAX_QUERY));
  if (!wanted) return [];
  const searches = [home.today, ...home.earlier].flatMap((day) =>
    day.entries.filter((entry): entry is Search => entry.kind !== "address"),
  );
  const shown = searches.find((search) => search.id === front) ?? null;
  const rows: HomeSuggestion[] = [];
  if (shown && shown.kind !== "pages") {
    const places = shown.kind === "place" ? [shown.place] : shown.places;
    for (const place of places)
      for (const row of place.rows) {
        if (row.menu || !fold(row.label).includes(wanted)) continue;
        if (rows.length === MAX_ROW_SUGGESTIONS) break;
        rows.push({
          kind: "row",
          search: shown.id,
          entry: row.id,
          label: row.label,
          place: place.title,
        });
      }
  }
  const earlier = searches
    .filter((search) => search.id !== front && fold(search.text).includes(wanted))
    .slice(0, MAX_SEARCH_SUGGESTIONS)
    .map((search): HomeSuggestion => ({ kind: "search", search: search.id, text: search.text }));
  return [...rows, ...earlier];
}
