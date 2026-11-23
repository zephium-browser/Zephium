import type {
  IconRef,
  MapAlsoKind,
  MapFailure,
  MapNetwork,
  SearchEngine,
} from "$shared/ipc/bindings";

/**
 * Home as it is drawn, projected from native state. Everything here is
 * page-derived and untrusted: it is shown as text and never acted on. Nothing
 * holds an address; opening anything names ids, which native resolves.
 */

/** A section named by its page, and nothing more. */
export type Section = { id: string; label: string };

/**
 * Why a page or a place could not be read, said to the person: the engine's
 * reason, or "gone" when the search's map has ended and it has to be asked
 * again, or "busy" when too many reads already wait.
 */
export type Refusal = MapFailure | "gone" | "busy";

/** A row's inside: being read, read, or refused. Absent until the person opens it. */
export type Inside =
  | { state: "opening" }
  | { state: "open"; rows: readonly Row[]; more: boolean }
  | { state: "refused"; reason: Refusal };

export type Row = {
  id: string;
  label: string;
  /** Shown beside the label when the page is on another host. */
  host: string | null;
  /** Its own links already held, shown on hover only. */
  known: readonly Section[];
  /** Its inside can be read on demand. */
  opens: boolean;
  /** A site's menu with no page of its own: its inside is what is already held. */
  menu: boolean;
  inside: Inside | null;
  /** The person opened this page from Home. */
  opened: boolean;
};

export type Place = {
  /** The place's slot in its search: "0", or "1" for a comparison's second. */
  id: string;
  title: string;
  host: string;
  /** Arriving while the map still reads it; failed when nothing could be read. */
  state: "arriving" | "settled" | "failed";
  /** Too little to read for a card, as the engine judged it: drawn as one line. */
  thin: boolean;
  failure: MapFailure | null;
  rows: readonly Row[];
  /** The rest of the known sections, shown without a request. */
  more: readonly Section[];
  actions: readonly Section[];
  social: readonly MapNetwork[];
  /** The row the person asked for, selected when the card draws. */
  asked: string | null;
  /** The icon of this place's page still open in a tab; else its letter is drawn. */
  icon: IconRef | null;
};

export type Also = { id: string; title: string; host: string; kind: MapAlsoKind };

/** A page opened from a search; `tab` names it while it is open. */
export type Opened = { id: string; title: string; host: string; tab: string | null };

type Pair = { left: string | null; right: string | null };

type Asked = {
  id: string;
  text: string;
  /** Minutes after the day's midnight, in the person's own time. */
  at: number;
  engine: SearchEngine | null;
  /** The icon of a page of this search still open, for its place's mark. */
  icon: IconRef | null;
  also: readonly Also[];
};

export type Search =
  | (Asked & { kind: "place"; place: Place })
  | (Asked & { kind: "compare"; places: readonly [Place, Place]; pairs: readonly Pair[] })
  | (Asked & { kind: "pages"; pages: readonly Opened[] });

export type Address = {
  kind: "address";
  id: string;
  at: number;
  host: string;
  title: string;
  icon: IconRef | null;
  /** The page's tab while it is open. */
  tab: string | null;
};

export type Entry = Search | Address;

/** A day of searches, newest first, keyed by its local date. */
export type Day = { key: string; entries: readonly Entry[] };

export type HomeModel = { today: Day; earlier: readonly Day[] };

export const MAX_SEARCHES = 32;
export const MAX_ROWS = 12;
export const MAX_MORE = 20;
export const MAX_CHILDREN = 12;
export const MAX_ACTIONS = 4;
export const MAX_ALSO = 8;
export const MAX_SOCIAL = 10;
export const MAX_PAGES = 24;
export const MAX_KNOWN = 8;
/** Text past this is cut; a row's label is cut shorter still. */
export const MAX_TEXT = 300;
export const MAX_LABEL = 120;
const MAX_ID = 64;

// Controls and the characters that reorder text around them are never shown.
const HIDDEN = /[\p{Cc}؜‎‏‪-‮⁦-⁩]/gu;

/** Page text made fit to show: no control or reordering characters, one line, bounded. */
export function clean(text: string, limit = MAX_TEXT): string {
  const flat = String(text).replace(HIDDEN, " ").replace(/\s+/gu, " ").trim();
  const chars = Array.from(flat);
  return chars.length > limit
    ? `${chars
        .slice(0, limit - 1)
        .join("")
        .trimEnd()}…`
    : flat;
}

/** An id fit to key by: bounded, printable, and never an address. */
export const fitId = (id: unknown): string | null =>
  typeof id === "string" && id.length > 0 && id.length <= MAX_ID && /^[\w.:-]+$/u.test(id)
    ? id
    : null;
