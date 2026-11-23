/**
 * Home's top in its own terms. Everything here is page-derived and untrusted:
 * it is shown as text and never acted on. Nothing holds an address; opening
 * anything is an intent naming ids, which the owner of the state resolves.
 */

export type Role =
  | "pricing"
  | "docs"
  | "developers"
  | "blog"
  | "help"
  | "about"
  | "careers"
  | "contact"
  | "product"
  | "community"
  | "changelog"
  | "status";

export type Network =
  | "x"
  | "github"
  | "youtube"
  | "linkedin"
  | "discord"
  | "reddit"
  | "mastodon"
  | "bluesky"
  | "instagram"
  | "facebook";

/** Why a page or a place could not be read, said to the person. */
export type Refusal = "blocked" | "challenge" | "unreadable" | "unavailable";

/** A section named by its page, and nothing more. */
export type Section = { id: string; label: string };

/** A row's inside: being read, read, or refused. Absent until it is asked for. */
export type Inside =
  | { state: "opening" }
  | { state: "open"; rows: readonly Row[]; more: boolean }
  | { state: "refused"; reason: Refusal };

export type Row = {
  id: string;
  label: string;
  role: Role | null;
  /** Shown beside the label when the page is on another host. */
  host: string | null;
  /** One line about the page, from what is already held. */
  note: string | null;
  /** Its own links already known, shown on hover only. */
  known: readonly Section[];
  /** Its inside can be read on demand. */
  opens: boolean;
  /**
   * A site's menu with no page of its own: its inside is the children already
   * held, shown and hidden here without asking for anything.
   */
  menu: boolean;
  inside: Inside | null;
  /** The person opened this page: its small picture's key, if one was kept. */
  picture: string | null;
  opened: boolean;
};

export type Action = {
  id: string;
  kind: "signin" | "signup" | "trial" | "sales" | "download";
  label: string;
};

export type Social = { network: Network; name: string };

export type Place = {
  id: string;
  title: string;
  host: string;
  /** Arriving while the first rows come in; failed when nothing could be read. */
  state: "arriving" | "settled" | "failed";
  /** Too little to read for a card, as the engine judged it: drawn as one line. */
  thin: boolean;
  failure: Refusal | null;
  rows: readonly Row[];
  /** The rest of the known sections, shown without a request. */
  more: readonly Section[];
  actions: readonly Action[];
  social: readonly Social[];
  /** The row the person asked for, selected when the card draws. */
  asked: string | null;
};

export type Also = {
  id: string;
  title: string;
  host: string;
  kind: "page" | "discussion" | "video";
};

export type Opened = { id: string; title: string; host: string; picture: string | null };

/** Rows of two places side by side, each pair on one line. */
type Pair = { left: string | null; right: string | null };

type Asked = {
  id: string;
  text: string;
  /** Minutes after the day's midnight, in the person's own time. */
  at: number;
  /** The results page's small picture, once it has loaded. */
  results: string | null;
  also: readonly Also[];
};

export type Search =
  | (Asked & { kind: "place"; place: Place })
  | (Asked & { kind: "compare"; places: readonly [Place, Place]; pairs: readonly Pair[] })
  | (Asked & { kind: "pages"; pages: readonly Opened[] });

type Address = { kind: "address"; id: string; at: number; host: string; title: string };

export type Entry = Search | Address;

/** A day of searches, newest first, keyed by its date; earlier days arrive when they are reached. */
export type Day = { key: string; count: number; entries: readonly Entry[] | null };

export type HomeModel = { today: Day; earlier: readonly Day[] };

export type HomeIntent =
  | { kind: "row"; search: string; row: string }
  | { kind: "inside"; search: string; row: string }
  | { kind: "fold"; search: string; row: string }
  | { kind: "site"; search: string; place: string }
  | { kind: "social"; search: string; place: string; network: Network }
  | { kind: "result"; search: string; result: string }
  | { kind: "page"; search: string; page: string }
  | { kind: "results"; search: string }
  | { kind: "address"; entry: string }
  | { kind: "field" }
  | { kind: "reach"; day: string };

export const MAX_ROWS = 12;
export const MAX_MORE = 20;
export const MAX_CHILDREN = 12;
export const MAX_ACTIONS = 4;
export const MAX_ALSO = 8;
export const MAX_SOCIAL = 10;
export const MAX_PAGES = 12;
export const MAX_KNOWN = 8;
export const MAX_ENTRIES = 200;
export const MAX_DAYS = 30;
/** Text past this is cut; a row's label is cut shorter still. */
export const MAX_TEXT = 300;
export const MAX_LABEL = 120;

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

const cleanNote = (note: string | null) => (note === null ? null : clean(note) || null);
const sections = (list: readonly Section[], limit: number) =>
  list
    .slice(0, limit)
    .map((section) => ({ id: section.id, label: clean(section.label, MAX_LABEL) }));

function admitRow(row: Row, depth: number): Row {
  const inside: Inside | null =
    depth > 0 || !row.inside
      ? null
      : row.inside.state === "open"
        ? {
            state: "open",
            rows: row.inside.rows.slice(0, MAX_CHILDREN).map((child) => admitRow(child, 1)),
            more: row.inside.more || row.inside.rows.length > MAX_CHILDREN,
          }
        : row.inside;
  const menu = depth === 0 && row.menu;
  return {
    ...row,
    label: clean(row.label, MAX_LABEL),
    // A menu has no page: nothing of its own to open, to show on another host or to picture.
    host: menu || row.host === null ? null : clean(row.host, MAX_LABEL) || null,
    note: cleanNote(row.note),
    known: sections(row.known, MAX_KNOWN),
    opens: depth === 0 && !menu && row.opens,
    menu,
    inside: menu && inside?.state !== "open" ? null : inside,
    picture: menu ? null : row.picture,
    opened: !menu && row.opened,
  };
}

/** A menu with nothing held under it leads nowhere, and is not drawn. */
const leads = (row: Row) =>
  !row.menu || (row.inside?.state === "open" && row.inside.rows.length > 0);

function admitPlace(place: Place): Place {
  const rows = place.rows
    .slice(0, MAX_ROWS)
    .map((row) => admitRow(row, 0))
    .filter(leads);
  return {
    ...place,
    title: clean(place.title, MAX_LABEL),
    host: clean(place.host, MAX_LABEL),
    rows,
    more: sections(place.more, MAX_MORE),
    actions: place.actions
      .slice(0, MAX_ACTIONS)
      .map((action) => ({ ...action, label: clean(action.label, MAX_LABEL) })),
    social: place.social
      .slice(0, MAX_SOCIAL)
      .map((social) => ({ ...social, name: clean(social.name, MAX_LABEL) })),
    asked: rows.some((row) => row.id === place.asked) ? place.asked : null,
  };
}

function admitEntry(entry: Entry): Entry {
  if (entry.kind === "address")
    return { ...entry, host: clean(entry.host, MAX_LABEL), title: clean(entry.title) };
  const asked = {
    text: clean(entry.text),
    also: entry.also
      .slice(0, MAX_ALSO)
      .map((also) => ({ ...also, title: clean(also.title), host: clean(also.host, MAX_LABEL) })),
  };
  if (entry.kind === "place") return { ...entry, ...asked, place: admitPlace(entry.place) };
  if (entry.kind === "compare") {
    const places = [admitPlace(entry.places[0]), admitPlace(entry.places[1])] as const;
    const left = new Set(places[0].rows.map((row) => row.id));
    const right = new Set(places[1].rows.map((row) => row.id));
    const pairs = entry.pairs
      .map((pair) => ({
        left: pair.left !== null && left.has(pair.left) ? pair.left : null,
        right: pair.right !== null && right.has(pair.right) ? pair.right : null,
      }))
      .filter((pair) => pair.left !== null || pair.right !== null)
      .slice(0, MAX_ROWS);
    return { ...entry, ...asked, places, pairs };
  }
  return {
    ...entry,
    ...asked,
    pages: entry.pages
      .slice(0, MAX_PAGES)
      .map((page) => ({ ...page, title: clean(page.title), host: clean(page.host, MAX_LABEL) })),
  };
}

const admitDay = (day: Day): Day => ({
  ...day,
  entries: day.entries === null ? null : day.entries.slice(0, MAX_ENTRIES).map(admitEntry),
});

/** The model as Home shows it: every list within its limit and every text fit to show. */
export const admit = (home: HomeModel): HomeModel => ({
  today: admitDay(home.today),
  earlier: home.earlier.slice(0, MAX_DAYS).map(admitDay),
});
