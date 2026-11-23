import type {
  Action,
  Also,
  Day,
  Entry,
  HomeModel,
  Inside,
  Opened,
  Place,
  Role,
  Row,
  Search,
  Section,
  Social,
} from "../lib/model";

const TODAY = "2026-11-23";

type Kind<K extends Search["kind"]> = Extract<Search, { kind: K }>;

type RowShape = Partial<Omit<Row, "id" | "label">>;

export const row = (id: string, label: string, shape: RowShape = {}): Row => ({
  id,
  label,
  role: null,
  host: null,
  note: null,
  known: [],
  opens: false,
  inside: null,
  picture: null,
  opened: false,
  ...shape,
});

const sections = (prefix: string, labels: readonly string[]): Section[] =>
  labels.map((label, index) => ({ id: `${prefix}${index}`, label }));

const roled = (id: string, label: string, role: Role, shape: RowShape = {}) =>
  row(id, label, { role, ...shape });

export function place(id: string, shape: Partial<Place> & Pick<Place, "title" | "host">): Place {
  return {
    id,
    state: "settled",
    failure: null,
    rows: [],
    more: [],
    actions: [],
    social: [],
    asked: null,
    ...shape,
  };
}

const also = (id: string, title: string, host: string, kind: Also["kind"] = "page"): Also => ({
  id,
  title,
  host,
  kind,
});

const social = (...networks: [Social["network"], string][]) =>
  networks.map(([network, name]) => ({ network, name }));

const actions = (...list: [Action["kind"], string][]): Action[] =>
  list.map(([kind, label], index) => ({ id: `act${index}`, kind, label }));

const docs: Inside = {
  state: "open",
  more: true,
  rows: [
    "Get started",
    "Payments",
    "Checkout",
    "Billing",
    "Connect",
    "Terminal",
    "Webhooks",
    "Testing",
    "API keys",
    "Error codes",
    "Libraries and SDKs",
    "Changelog",
  ].map((label, index) => row(`docs${index}`, label)),
};

/** A large site: twelve rows, twenty more, actions, profiles, and one inside opened. */
export const stripe = (inside: Inside | null = docs): Kind<"place"> => ({
  kind: "place",
  id: "q-stripe",
  text: "stripe pricing",
  at: 14 * 60 + 32,
  results: "results:stripe",
  also: [
    also("a1", "Stripe fees explained: every charge in one place", "nerdwallet.com"),
    also("a2", "Is Stripe worth it for a small shop?", "reddit.com", "discussion"),
    also("a3", "Stripe vs PayPal vs Square in 2026", "forbes.com"),
    also("a4", "How Stripe pricing actually works", "youtube.com", "video"),
    also("a5", "Stripe Billing pricing calculator", "chargebee.com"),
    also("a6", "Negotiating custom Stripe rates", "news.ycombinator.com", "discussion"),
    also("a7", "Stripe pricing and fees", "en.wikipedia.org"),
    also("a8", "Stripe alternatives with lower fees", "g2.com"),
  ],
  place: place("stripe.com", {
    title: "Stripe",
    host: "stripe.com",
    asked: "pricing",
    rows: [
      roled("payments", "Payments", "product", {
        opens: true,
        note: "Accept payments online, in person and around the world.",
        known: sections("pk", ["Online payments", "In-person", "Payment methods", "Link"]),
      }),
      roled("billing", "Billing", "product", { opens: true }),
      roled("connect", "Connect", "product", { opens: true }),
      roled("terminal", "Terminal", "product", { opens: true }),
      roled("radar", "Radar", "product"),
      row("atlas", "Atlas", { opens: true }),
      roled("pricing", "Pricing", "pricing", {
        opens: true,
        note: "Integrated per-transaction pricing with no hidden fees.",
        known: sections("prk", ["Standard", "Custom", "Payments", "Billing", "Tax"]),
        opened: true,
        picture: "page:stripe-pricing",
      }),
      roled("documentation", "Documentation", "docs", {
        opens: true,
        host: "docs.stripe.com",
        inside,
      }),
      roled("api", "API reference", "developers", { host: "docs.stripe.com" }),
      row("customers", "Customers", { opens: true }),
      roled("blog", "Blog", "blog", { opens: true }),
      roled("support", "Support", "help", { host: "support.stripe.com", opens: true }),
    ],
    more: sections("m", [
      "Issuing",
      "Treasury",
      "Capital",
      "Climate",
      "Identity",
      "Tax",
      "Revenue Recognition",
      "Sigma",
      "Data Pipeline",
      "Invoicing",
      "Checkout",
      "Elements",
      "Link",
      "Payment Links",
      "Financial Connections",
      "Crypto",
      "Startups",
      "Enterprise",
      "Partners",
      "Jobs",
    ]),
    actions: actions(["signin", "Sign in"], ["sales", "Contact sales"], ["trial", "Start now"]),
    social: social(
      ["x", "@stripe"],
      ["github", "stripe"],
      ["youtube", "Stripe"],
      ["linkedin", "Stripe"],
      ["discord", "Stripe Developers"],
    ),
  }),
});

/** A small site: five rows, nothing more to know. */
export const raycast: Kind<"place"> = {
  kind: "place",
  id: "q-raycast",
  text: "raycast pro",
  at: 13 * 60 + 5,
  results: "results:raycast",
  also: [
    also("r1", "Raycast Pro: is it worth paying for?", "reddit.com", "discussion"),
    also("r2", "Raycast vs Alfred", "zapier.com"),
    also("r3", "Raycast AI in five minutes", "youtube.com", "video"),
  ],
  place: place("raycast.com", {
    title: "Raycast",
    host: "raycast.com",
    asked: "pro",
    rows: [
      roled("store", "Store", "product", { opens: true }),
      roled("pro", "Pro", "pricing", { note: "AI, cloud sync, custom themes and more." }),
      row("teams", "Teams"),
      roled("developers", "Developers", "developers", {
        host: "developers.raycast.com",
        opens: true,
      }),
      roled("changelog", "Changelog", "changelog"),
    ],
    actions: actions(["download", "Download"], ["signin", "Log in"]),
    social: social(["x", "@raycast"], ["github", "raycast"], ["youtube", "Raycast"]),
  }),
};

const page = (id: string, title: string, host: string, picture: string | null): Opened => ({
  id,
  title,
  host,
  picture,
});

/** A search that is not a place, and the three pages opened from it. */
export const grinders: Kind<"pages"> = {
  kind: "pages",
  id: "q-grinders",
  text: "best espresso grinder for beginners",
  at: 11 * 60 + 48,
  results: "results:grinders",
  also: [],
  pages: [
    page("g1", "The best espresso grinders, tested", "seriouseats.com", "page:grinder-1"),
    page("g2", "Grinder advice for a first machine", "reddit.com", "page:grinder-2"),
    page("g3", "Niche Zero, two years on", "youtube.com", "page:grinder-3"),
  ],
};

/** Two places side by side, paired row for row. */
export const compare: Kind<"compare"> = {
  kind: "compare",
  id: "q-compare",
  text: "notion vs obsidian",
  at: 10 * 60 + 20,
  results: "results:compare",
  also: [
    also("c1", "Notion vs Obsidian: which one in 2026?", "zapier.com"),
    also("c2", "I moved my notes from Notion to Obsidian", "reddit.com", "discussion"),
  ],
  places: [
    place("notion.com", {
      title: "Notion",
      host: "notion.com",
      asked: "n-pricing",
      rows: [
        roled("n-product", "Product", "product", { opens: true }),
        roled("n-pricing", "Pricing", "pricing", { opens: true }),
        row("n-download", "Download"),
        row("n-templates", "Templates", { opens: true }),
        roled("n-help", "Help center", "help", { opens: true }),
        row("n-enterprise", "Enterprise"),
        roled("n-blog", "Blog", "blog"),
      ],
      actions: actions(["signin", "Log in"], ["trial", "Get Notion free"]),
      social: social(["x", "@NotionHQ"], ["youtube", "Notion"]),
    }),
    place("obsidian.md", {
      title: "Obsidian",
      host: "obsidian.md",
      asked: "o-pricing",
      rows: [
        roled("o-pricing", "Pricing", "pricing"),
        row("o-download", "Download"),
        row("o-plugins", "Plugins", { opens: true }),
        roled("o-help", "Help", "help", { host: "help.obsidian.md", opens: true }),
        row("o-sync", "Sync"),
        row("o-publish", "Publish"),
        roled("o-blog", "Blog", "blog"),
      ],
      actions: actions(["signin", "Log in"]),
      social: social(["discord", "Obsidian"], ["reddit", "r/ObsidianMD"]),
    }),
  ],
  pairs: [
    { left: "n-pricing", right: "o-pricing" },
    { left: "n-download", right: "o-download" },
    { left: "n-templates", right: "o-plugins" },
    { left: "n-help", right: "o-help" },
    { left: "n-product", right: null },
    { left: null, right: "o-sync" },
    { left: null, right: "o-publish" },
    { left: "n-enterprise", right: null },
    { left: "n-blog", right: "o-blog" },
  ],
};

/** A row being read, and a row the site would not let be read. */
export const figma = (insides: { dev?: Inside; plugins?: Inside } = {}): Kind<"place"> => ({
  kind: "place",
  id: "q-figma",
  text: "figma dev mode",
  at: 9 * 60 + 41,
  results: "results:figma",
  also: [also("f1", "Dev Mode: everything that changed", "youtube.com", "video")],
  place: place("figma.com", {
    title: "Figma",
    host: "figma.com",
    asked: "dev",
    rows: [
      roled("design", "Design", "product", { opens: true }),
      roled("dev", "Dev Mode", "product", {
        opens: true,
        inside: insides.dev ?? { state: "refused", reason: "challenge" },
      }),
      row("community", "Community", {
        role: "community",
        opens: true,
        inside: insides.plugins ?? { state: "opening" },
      }),
      row("figjam", "FigJam", { opens: true }),
      row("slides", "Slides", { opens: true }),
      roled("pricing", "Pricing", "pricing"),
    ],
    actions: actions(
      ["signin", "Log in"],
      ["signup", "Get started for free"],
      ["sales", "Contact sales"],
    ),
  }),
});

/** A place whose first rows are in and the rest are on their way. */
export const arriving = (rows = 3): Kind<"place"> => ({
  kind: "place",
  id: "q-vercel",
  text: "vercel",
  at: 15 * 60 + 2,
  results: null,
  also: [],
  place: place("vercel.com", {
    title: "Vercel",
    host: "vercel.com",
    state: "arriving",
    asked: null,
    rows: [
      roled("v-products", "Products", "product", { opens: true }),
      roled("v-docs", "Docs", "docs", { opens: true }),
      roled("v-pricing", "Pricing", "pricing"),
      row("v-templates", "Templates", { opens: true }),
      roled("v-blog", "Blog", "blog"),
      roled("v-changelog", "Changelog", "changelog"),
      row("v-enterprise", "Enterprise"),
    ].slice(0, rows),
  }),
});

/** A place nothing could be read from. */
export const failed: Kind<"place"> = {
  kind: "place",
  id: "q-failed",
  text: "northwind bank login",
  at: 8 * 60 + 15,
  results: "results:bank",
  also: [also("b1", "Northwind Bank customer service", "northwindbank.example")],
  place: place("northwindbank.example", {
    title: "Northwind Bank",
    host: "northwindbank.example",
    state: "failed",
    failure: "blocked",
  }),
};

const LONG =
  "Enterprise-grade observability for distributed systems with end-to-end tracing across every region";

/** Text past any sensible length, everywhere text is shown. */
const long: Kind<"place"> = {
  kind: "place",
  id: "q-long",
  text: "the international association of independent observability platform vendors annual report",
  at: 12 * 60 + 30,
  results: "results:long",
  also: [
    also(
      "l1",
      `${LONG} and a title that keeps going past its line`,
      "a-very-long-hostname-for-a-site.example.co.uk",
    ),
  ],
  place: place("observability.example", {
    title: "The International Association of Independent Observability Platform Vendors",
    host: "members.international-association-of-observability-vendors.example.org",
    asked: "l-report",
    rows: [
      row(
        "l-report",
        "Annual report on the state of distributed tracing and continuous profiling",
        {
          opens: true,
          note: LONG,
        },
      ),
      row("l-members", LONG, {
        host: "directory.international-association-of-observability-vendors.example.org",
      }),
      roled("l-pricing", "Membership", "pricing"),
      row("l-events", "Events"),
    ],
    actions: actions(
      ["signin", "Sign in to the members area with your organisation account"],
      ["sales", "Talk to our membership team about enterprise tiers"],
      ["trial", "Start a free trial"],
      ["download", "Download the report"],
    ),
    social: social([
      "linkedin",
      "International Association of Independent Observability Platform Vendors",
    ]),
  }),
};

/** A right-to-left site, asked in its own script. */
const rtl: Kind<"place"> = {
  kind: "place",
  id: "q-rtl",
  text: "الجزيرة أخبار",
  at: 7 * 60 + 55,
  results: "results:rtl",
  also: [also("w1", "قناة الجزيرة - ويكيبيديا", "ar.wikipedia.org")],
  place: place("aljazeera.net", {
    title: "الجزيرة نت",
    host: "aljazeera.net",
    asked: "w-news",
    rows: [
      row("w-news", "أخبار", { opens: true, note: "آخر الأخبار من العالم العربي والعالم" }),
      row("w-economy", "اقتصاد", { opens: true }),
      row("w-sport", "رياضة", { opens: true }),
      row("w-culture", "ثقافة"),
      row("w-live", "البث المباشر"),
      row("w-programs", "برامج", { opens: true }),
    ],
    social: social(["youtube", "الجزيرة"], ["x", "@AJArabic"]),
  }),
};

const address = (id: string, at: number, host: string, title: string): Entry => ({
  kind: "address",
  id,
  at,
  host,
  title,
});

const day = (key: string, entries: Entry[] | null, count = entries?.length ?? 0): Day => ({
  key,
  count,
  entries,
});

export const home = (today: Entry[], earlier: Day[] = []): HomeModel => ({
  today: day(TODAY, today),
  earlier,
});

const QUERIES = [
  "linear changelog",
  "rust async book",
  "github actions cache",
  "tailwind container queries",
  "postgres jsonb index",
  "figma variables",
  "sqlite wal mode",
  "svelte 5 runes",
  "weather lisbon",
  "cheap flights porto",
  "vite library mode",
  "docker compose watch",
];

/** A filler search: a small place for even ones, a page or two for odd ones. */
function filler(index: number, prefix: string, at: number): Entry {
  const text = QUERIES[index % QUERIES.length]!;
  const id = `${prefix}-${index}`;
  if (index % 5 === 4) return address(id, at, "news.ycombinator.com", "Hacker News");
  if (index % 2 === 0)
    return {
      kind: "place",
      id,
      text,
      at,
      results: `results:${id}`,
      also: [],
      place: place(`${id}.example`, {
        title: text.split(" ")[0]!.replace(/^./u, (c) => c.toUpperCase()),
        host: `${text.split(" ")[0]}.example`,
        asked: `${id}-r1`,
        rows: ["Overview", "Docs", "Pricing", "Changelog"].map((label, n) =>
          row(`${id}-r${n}`, label, { opens: n === 1 }),
        ),
      }),
    };
  return {
    kind: "pages",
    id,
    text,
    at,
    results: `results:${id}`,
    also: [],
    pages: Array.from({ length: 1 + (index % 3) }, (_, n) =>
      page(`${id}-p${n}`, `${text} — result ${n + 1}`, "example.org", null),
    ),
  };
}

/** A day with thirty searches, and three earlier days, the oldest not yet loaded. */
export const busy = (): HomeModel => ({
  today: day(
    TODAY,
    Array.from({ length: 30 }, (_, index) =>
      index === 0
        ? stripe(null)
        : index === 1
          ? grinders
          : filler(index, "t", 18 * 60 - index * 20),
    ),
  ),
  earlier: [
    day(
      "2026-11-22",
      Array.from({ length: 6 }, (_, index) => filler(index, "y", 20 * 60 - index * 45)),
    ),
    day("2026-11-21", [
      raycast,
      ...Array.from({ length: 3 }, (_, i) => filler(i + 1, "s", 600 - i * 30)),
    ]),
    day("2026-11-20", null, 14),
  ],
});

/** The day the pictures show first: a large place opened up, and older searches under it. */
export const today = (): HomeModel =>
  home(
    [stripe(), grinders, raycast, address("t-gh", 10 * 60 + 2, "github.com", "Pull requests")],
    [day("2026-11-22", [compare])],
  );

export const states = (): HomeModel => home([arriving(), figma(), failed]);

export const edges = (): HomeModel => home([long, rtl]);

export const comparing = (): HomeModel => home([compare, raycast]);
