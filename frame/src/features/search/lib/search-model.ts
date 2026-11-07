import { matchRange as titleMatchRange } from "$shared/lib/match-range";
import type { SearchResult, SearchContext } from "$shared/ipc/bindings";

/** Canonical identity of the action a row runs. Selection follows this, not
 *  list position, so an incremental update cannot move the highlight. */
export function resultIdentity(result: SearchResult): string {
  const action = result.action;
  if (action.type === "OpenNote") return `note:${action.id}`;
  if (action.type === "ActivateTab") return `tab:${action.id}`;
  if (action.type === "OpenUrl") return `url:${action.url}`;
  return `command:${action.id}`;
}

export function sameSearch(expected: SearchContext | null, actual: SearchContext | null): boolean {
  return (
    !!expected &&
    !!actual &&
    expected.session_id === actual.session_id &&
    expected.request_id === actual.request_id &&
    expected.window_id === actual.window_id &&
    expected.profile_id === actual.profile_id &&
    expected.space_id === actual.space_id
  );
}

export type ResultSection =
  | "search"
  | "tabs"
  | "history"
  | "notes"
  | "commands"
  /** Browser destinations the launcher hands over, such as Notes or Tasks. */
  | "destinations"
  /** Arithmetic typed into the field, answered locally. */
  | "calculator";

/** Mirrors `zephium_core::search::result_section`. Native already emits results
 *  in section order, so this only names the run a row belongs to. */
export function resultSection(result: SearchResult): ResultSection {
  if (result.kind === "tab") return "tabs";
  if (result.kind === "note") return "notes";
  if (result.kind === "history" || result.kind === "bookmark" || result.kind === "search_history")
    return "history";
  if (result.kind === "command") return "commands";
  return "search";
}

/** Groups adjacent rows. Native owns the order, so grouping never reorders:
 *  a late note or suggestion extends its own run instead of promoting its
 *  section above rows the user is already reading. */
export function groupRows<T extends { section: ResultSection }>(rows: readonly T[]) {
  const groups: { section: ResultSection; rows: T[] }[] = [];
  for (const row of rows) {
    const open = groups.at(-1);
    if (open?.section === row.section) open.rows.push(row);
    else groups.push({ section: row.section, rows: [row] });
  }
  return groups;
}

/** Secondary text: the host, never the whole address. Search rows carry the
 *  engine name instead, and a note the line that explains why it matched. */
export function resultDetail(result: SearchResult): string {
  if (result.kind === "search" || result.kind === "suggestion" || result.kind === "note")
    return result.detail;
  if (
    result.kind !== "tab" &&
    result.kind !== "history" &&
    result.kind !== "bookmark" &&
    result.kind !== "search_history"
  )
    return "";
  return hostOf(result.detail);
}

const lowercase = (value: string) => value.toLowerCase();

/** The one contiguous span of `title` matching the query, for emphasis. A
 *  scattered per-token match is deliberately not highlighted: fragmenting a
 *  title into alternating weights reads as noise at this size. */
export function matchRange(title: string, query: string): [number, number] | null {
  return titleMatchRange(title, query, lowercase);
}

/** Native sends a tab its bare host and a visit its full address, so accept
 *  either spelling rather than depending on which one a source happened to
 *  use. "www." is dropped: it is never the part a person typed. */
function hostOf(value: string): string {
  try {
    return new URL(value).host.replace(/^www\./u, "");
  } catch {
    return value.replace(/^www\./u, "");
  }
}

/** The bare host a row leads to, or "" when it leads somewhere without one. */
export function resultHost(result: SearchResult): string {
  if (result.kind === "tab") return hostOf(result.detail);
  if (result.kind !== "history" && result.kind !== "url") return "";
  return result.action.type === "OpenUrl" ? hostOf(result.action.url) : "";
}

/** The row a visible completion stands for. Once the field reads "notion.so",
 *  that is the destination the user is looking at, so it is what Enter must
 *  open — searching the fragment they typed instead is the wrong answer. */
export function completionTarget<T extends { result: SearchResult | null }>(
  rows: readonly T[],
  completion: string | null,
): T | null {
  if (!completion) return null;
  return rows.find((row) => !!row.result && resultHost(row.result) === completion) ?? null;
}

/** Inline completion the field may apply. Native offers a host; the field only
 *  accepts one that strictly extends exactly what is already typed, so a late
 *  answer can never rewrite text the user has moved past. */
export function completionSuffix(typed: string, completion: string | null): string | null {
  if (!completion || !typed.trim()) return null;
  const lower = completion.toLowerCase();
  const start = typed.toLowerCase();
  if (!lower.startsWith(start) || completion.length <= typed.length) return null;
  return completion.slice(typed.length);
}
