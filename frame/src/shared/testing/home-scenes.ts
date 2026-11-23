/** Home scenes as native would hold them, for the canvas, the frame and the app. */
import type { MapChange_Serialize, MapNetwork } from "$shared/ipc/bindings";
import {
  also,
  children,
  comparison,
  ended,
  entry,
  failedAt,
  first,
  menu,
  off,
  opened,
  opens,
  placed,
  refusedInside,
  roled,
  tab,
  wirePlace,
  wireSearch,
  type Wire,
} from "./home-wire";

export type { Wire };

/** Five in the afternoon of the day the pictures show, in the person's own time. */
export const NOW = new Date(2026, 10, 23, 17, 0).getTime();
const at = (hour: number, minute: number, daysAgo = 0) =>
  new Date(2026, 10, 23 - daysAgo, hour, minute).getTime();

const STRIPE_ALSO = [
  also("Stripe fees explained: every charge in one place", "nerdwallet.com"),
  also("Is Stripe worth it for a small shop?", "reddit.com", "discussion"),
  also("Stripe vs PayPal vs Square in 2026", "forbes.com"),
  also("How Stripe pricing actually works", "youtube.com", "video"),
  also("Stripe Billing pricing calculator", "chargebee.com"),
  also("Negotiating custom Stripe rates", "news.ycombinator.com", "discussion"),
  also("Stripe pricing and fees", "en.wikipedia.org"),
  also("Stripe alternatives with lower fees", "g2.com"),
];

const STRIPE = { title: "Stripe", host: "stripe.com" };
const STRIPE_SOCIAL: MapNetwork[] = ["twitter", "git_hub", "you_tube", "linked_in"];

/** A large site as it now arrives: its menus, and the row that was asked for. */
export const stripe = (): Wire =>
  wireSearch({
    id: "q-payments",
    text: "stripe pricing",
    at: at(14, 32),
    changes: [
      first(1, STRIPE, STRIPE_ALSO, STRIPE_SOCIAL),
      placed(
        2,
        wirePlace({
          ...STRIPE,
          focus: "pricing",
          rows: [
            menu("products", "Products", [
              "Payments",
              "Billing",
              "Connect",
              "Terminal",
              "Radar",
              "Atlas",
              "Issuing",
              "Tax",
            ]),
            menu("solutions", "Solutions", [
              "Startups",
              "Enterprises",
              "SaaS",
              "Platforms",
              "Ecommerce",
              "Marketplaces",
              "Embedded finance",
            ]),
            menu("developers", "Developers", [
              "Documentation",
              "API reference",
              "API status",
              "Changelog",
              "Libraries and SDKs",
              "Stripe Apps",
              "Sample projects",
              "Testing",
            ]),
            menu("resources", "Resources", [
              "Guides",
              "Customer stories",
              "Blog",
              "Sessions",
              "Partners",
              "Support center",
              "Jobs",
            ]),
            roled("pricing", "Pricing", "pricing", {
              children: children("prk", ["Standard", "Custom", "Payments", "Billing", "Tax"]),
            }),
          ],
          more: ["Privacy", "Terms", "Sitemap", "Status", "Press", "Security"],
          actions: ["Sign in", "Contact sales", "Start now"],
          social: STRIPE_SOCIAL,
        }),
      ),
      ended(3),
    ],
    pages: [opened("pricing", "Pricing & fees | Stripe", "stripe.com", "tab-pricing")],
  });

const DOCS = [
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
];

/** A large flat site: twelve rows, twenty more, actions, profiles, and one inside read. */
export const large = (): Wire =>
  wireSearch({
    id: "q-stripe",
    text: "stripe pricing",
    at: at(14, 32),
    live: true,
    changes: [
      first(1, STRIPE, STRIPE_ALSO, [...STRIPE_SOCIAL, "discord"]),
      placed(
        2,
        wirePlace({
          ...STRIPE,
          focus: "pricing",
          rows: [
            roled("payments", "Payments", "product", {
              open: true,
              children: children("pk", ["Online payments", "In-person", "Payment methods", "Link"]),
            }),
            roled("billing", "Billing", "product", { open: true }),
            roled("connect", "Connect", "product", { open: true }),
            roled("terminal", "Terminal", "product", { open: true }),
            roled("radar", "Radar", "product"),
            opens("atlas", "Atlas"),
            roled("pricing", "Pricing", "pricing", {
              open: true,
              children: children("prk", ["Standard", "Custom", "Payments", "Billing", "Tax"]),
            }),
            roled("documentation", "Documentation", "documentation", {
              open: true,
              ...off("docs.stripe.com"),
              read: "read",
              more: true,
              children: children("docs", DOCS),
            }),
            roled("api", "API reference", "api", off("docs.stripe.com")),
            opens("customers", "Customers"),
            roled("blog", "Blog", "blog", { open: true }),
            roled("support", "Support", "support", { open: true, ...off("support.stripe.com") }),
          ],
          more: [
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
          ],
          actions: ["Sign in", "Contact sales", "Start now"],
          social: [...STRIPE_SOCIAL, "discord"],
        }),
      ),
      ended(3),
    ],
    pages: [opened("pricing", "Pricing & fees | Stripe", "stripe.com", "tab-pricing")],
  });

/** A flat site: six plain rows, nothing more to know. */
export const raycast = (): Wire =>
  wireSearch({
    id: "q-raycast",
    text: "raycast pro",
    at: at(13, 5),
    engine: "duckduckgo",
    changes: [
      first(1, { title: "Raycast", host: "raycast.com" }, [
        also("Raycast Pro: is it worth paying for?", "reddit.com", "discussion"),
        also("Raycast vs Alfred", "zapier.com"),
        also("Raycast AI in five minutes", "youtube.com", "video"),
      ]),
      placed(
        2,
        wirePlace({
          title: "Raycast",
          host: "raycast.com",
          focus: "pro",
          rows: [
            roled("store", "Store", "product"),
            roled("pro", "Pro", "pricing"),
            entry("teams", "Teams"),
            roled("developers", "Developers", "api", off("developers.raycast.com")),
            roled("changelog", "Changelog", "changelog"),
            entry("blog", "Blog"),
          ],
          actions: ["Download", "Log in"],
          social: ["twitter", "git_hub", "you_tube"],
        }),
      ),
      ended(3),
    ],
  });

/** A search that is not a place, and the three pages opened from it. */
export const grinders = (): Wire =>
  wireSearch({
    id: "q-grinders",
    text: "best espresso grinder for beginners",
    at: at(11, 48),
    kind: "other",
    pages: [
      opened(null, "The best espresso grinders, tested", "seriouseats.com", "tab-g1"),
      opened(null, "Grinder advice for a first machine", "reddit.com", "tab-g2"),
      opened(null, "Niche Zero, two years on", "youtube.com"),
    ],
  });

/** Two places side by side, paired row for row. */
export const compare = (daysAgo = 1): Wire =>
  wireSearch({
    id: "q-compare",
    text: "notion vs obsidian",
    at: at(10, 20, daysAgo),
    kind: "compare",
    changes: [
      first(1, { title: "Notion", host: "notion.com" }, [
        also("Notion vs Obsidian: which one in 2026?", "zapier.com"),
        also("I moved my notes from Notion to Obsidian", "reddit.com", "discussion"),
      ]),
      placed(
        2,
        wirePlace({
          title: "Notion",
          host: "notion.com",
          focus: "n-pricing",
          rows: [
            roled("n-product", "Product", "product", { open: true }),
            roled("n-pricing", "Pricing", "pricing", { open: true }),
            entry("n-download", "Download"),
            opens("n-templates", "Templates"),
            roled("n-help", "Help center", "support", { open: true }),
            entry("n-enterprise", "Enterprise"),
            roled("n-blog", "Blog", "blog"),
          ],
          actions: ["Log in", "Get Notion free"],
          social: ["twitter", "you_tube"],
        }),
        0,
      ),
      placed(
        3,
        wirePlace({
          title: "Obsidian",
          host: "obsidian.md",
          focus: "o-pricing",
          rows: [
            roled("o-pricing", "Pricing", "pricing"),
            entry("o-download", "Download"),
            opens("o-plugins", "Plugins"),
            roled("o-help", "Help", "support", { open: true, ...off("help.obsidian.md") }),
            entry("o-sync", "Sync"),
            entry("o-publish", "Publish"),
            roled("o-blog", "Blog", "blog"),
          ],
          actions: ["Log in"],
          social: ["discord", "reddit"],
        }),
        1,
      ),
      comparison(4, [
        { label: "Pricing", left: "n-pricing", right: "o-pricing" },
        { label: "Download", left: "n-download", right: "o-download" },
        { label: "Extend", left: "n-templates", right: "o-plugins" },
        { label: "Help", left: "n-help", right: "o-help" },
        { label: "Product", left: "n-product", right: null },
        { label: "Sync", left: null, right: "o-sync" },
        { label: "Publish", left: null, right: "o-publish" },
        { label: "Enterprise", left: "n-enterprise", right: null },
        { label: "Blog", left: "n-blog", right: "o-blog" },
      ]),
      ended(5),
    ],
  });

/** A row being read, and a row the site would not let be read. */
export const figma = (): Wire =>
  wireSearch({
    id: "q-figma",
    text: "figma dev mode",
    at: at(9, 41),
    live: true,
    changes: [
      first(1, { title: "Figma", host: "figma.com" }, [
        also("Dev Mode: everything that changed", "youtube.com", "video"),
      ]),
      placed(
        2,
        wirePlace({
          title: "Figma",
          host: "figma.com",
          focus: "dev",
          rows: [
            roled("design", "Design", "product", { open: true }),
            roled("dev", "Dev Mode", "product", { open: true, read: "refused" }),
            roled("community", "Community", "community", { open: true }),
            opens("figjam", "FigJam"),
            opens("slides", "Slides"),
            roled("pricing", "Pricing", "pricing"),
          ],
          actions: ["Log in", "Get started for free", "Contact sales"],
        }),
      ),
      refusedInside(3, "dev", "challenge"),
    ],
  });

/** What the frame holds beside figma: both insides opened, one still being read. */
export const figmaLocal = {
  unfolded: new Set(["q-figma/dev", "q-figma/community"]),
  waiting: new Set(["q-figma/community"]),
};

const VERCEL_ROWS = [
  roled("v-products", "Products", "product", { open: true }),
  roled("v-docs", "Docs", "documentation", { open: true }),
  roled("v-pricing", "Pricing", "pricing"),
  opens("v-templates", "Templates"),
  roled("v-blog", "Blog", "blog"),
  roled("v-changelog", "Changelog", "changelog"),
  entry("v-enterprise", "Enterprise"),
];

/** A place whose first rows are in and the rest are on their way. */
export const arriving = (rows = 3): Wire =>
  wireSearch({
    id: "q-vercel",
    text: "vercel",
    at: at(15, 2),
    live: true,
    changes: [
      first(1, { title: "Vercel", host: "vercel.com" }),
      placed(
        2,
        wirePlace({ title: "Vercel", host: "vercel.com", rows: VERCEL_ROWS.slice(0, rows) }),
        0,
        false,
      ),
    ],
  });

/** The vercel map as its stream tells it, a row at a time. */
export const vercelStream = (): MapChange_Serialize[] => [
  first(1, { title: "Vercel", host: "vercel.com" }),
  ...VERCEL_ROWS.map((_, index) =>
    placed(
      index + 2,
      wirePlace({ title: "Vercel", host: "vercel.com", rows: VERCEL_ROWS.slice(0, index + 1) }),
      0,
      index === VERCEL_ROWS.length - 1,
    ),
  ),
  ended(VERCEL_ROWS.length + 2),
];

/** A place nothing could be read from. */
export const failed = (): Wire =>
  wireSearch({
    id: "q-failed",
    text: "northwind bank login",
    at: at(8, 15),
    changes: [
      first(1, { title: "Northwind Bank", host: "northwindbank.example" }, [
        also("Northwind Bank customer service", "northwindbank.example"),
      ]),
      failedAt(2, "blocked"),
      ended(3),
    ],
  });

/** A place with too little to read for a card: two rows and a booking link. */
export const studio = (): Wire =>
  wireSearch({
    id: "q-studio",
    text: "ana clay studio hours",
    at: at(16, 10),
    changes: [
      first(1, { title: "Ana Clay Studio", host: "anaclay.example" }, [
        also(
          "Ana Clay Studio: reviews and opening hours for the pottery studio on Rua da Rosa",
          "maps.example",
        ),
        also("Pottery classes, Lisbon", "timeout.com"),
      ]),
      placed(
        2,
        wirePlace({
          title: "Ana Clay Studio",
          host: "anaclay.example",
          thin: true,
          focus: "s-visit",
          rows: [entry("s-classes", "Classes"), entry("s-visit", "Visit")],
          actions: ["Book a class"],
          social: ["instagram"],
        }),
      ),
      ended(3),
    ],
  });

/** A place that is only its name. */
export const bare = (): Wire =>
  wireSearch({
    id: "q-bare",
    text: "mira okafor portfolio",
    at: at(15, 40),
    changes: [
      first(1, { title: "Mira Okafor", host: "miraokafor.example" }),
      placed(
        2,
        wirePlace({ title: "Mira Okafor", host: "miraokafor.example", thin: true, rows: [] }),
      ),
      ended(3),
    ],
  });

const LONG =
  "Enterprise-grade observability for distributed systems with end-to-end tracing across every region";

/** Text past any sensible length, everywhere text is shown. */
export const long = (): Wire =>
  wireSearch({
    id: "q-long",
    text: "the international association of independent observability platform vendors annual report",
    at: at(12, 30),
    changes: [
      first(
        1,
        {
          title: "The International Association of Independent Observability Platform Vendors",
          host: "members.international-association-of-observability-vendors.example.org",
        },
        [
          also(
            `${LONG} and a title that keeps going past its line`,
            "a-very-long-hostname-for-a-site.example.co.uk",
          ),
        ],
      ),
      placed(
        2,
        wirePlace({
          title: "The International Association of Independent Observability Platform Vendors",
          host: "members.international-association-of-observability-vendors.example.org",
          focus: "l-report",
          rows: [
            opens(
              "l-report",
              "Annual report on the state of distributed tracing and continuous profiling",
            ),
            entry(
              "l-members",
              LONG,
              off("directory.international-association-of-observability-vendors.example.org"),
            ),
            roled("l-pricing", "Membership", "pricing"),
            entry("l-events", "Events"),
          ],
          actions: [
            "Sign in to the members area with your organisation account",
            "Talk to our membership team about enterprise tiers",
            "Start a free trial",
            "Download the report",
          ],
          social: ["linked_in"],
        }),
      ),
      ended(3),
    ],
  });

/** A right-to-left site, asked in its own script. */
export const rtl = (): Wire =>
  wireSearch({
    id: "q-rtl",
    text: "الجزيرة أخبار",
    at: at(7, 55),
    changes: [
      first(1, { title: "الجزيرة نت", host: "aljazeera.net" }, [
        also("قناة الجزيرة - ويكيبيديا", "ar.wikipedia.org"),
      ]),
      placed(
        2,
        wirePlace({
          title: "الجزيرة نت",
          host: "aljazeera.net",
          focus: "w-news",
          rows: [
            opens("w-news", "أخبار"),
            opens("w-economy", "اقتصاد"),
            opens("w-sport", "رياضة"),
            entry("w-culture", "ثقافة"),
            entry("w-live", "البث المباشر"),
            opens("w-programs", "برامج"),
          ],
          social: ["you_tube", "twitter"],
        }),
      ),
      ended(3),
    ],
  });

const address = (id: string, when: number, host: string, title: string, tab: string | null) =>
  wireSearch({
    id,
    text: host,
    at: when,
    kind: "address",
    pages: [opened(null, title, host, tab)],
  });

const raycastOn = (daysAgo: number) => ({ ...raycast(), at_ms: at(13, 5, daysAgo) });

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
function filler(index: number, prefix: string, when: number): Wire {
  const text = QUERIES[index % QUERIES.length]!;
  const id = `${prefix}-${index}`;
  if (index % 5 === 4) return address(id, when, "news.ycombinator.com", "Hacker News", null);
  if (index % 2 === 0) {
    const name = text.split(" ")[0]!;
    const place = { title: name.replace(/^./u, (c) => c.toUpperCase()), host: `${name}.example` };
    return wireSearch({
      id,
      text,
      at: when,
      changes: [
        first(1, place),
        placed(
          2,
          wirePlace({
            ...place,
            focus: `${id}-r1`,
            rows: ["Overview", "Docs", "Pricing", "Changelog"].map((label, n) =>
              entry(`${id}-r${n}`, label, { open: n === 1 }),
            ),
          }),
        ),
        ended(3),
      ],
    });
  }
  return wireSearch({
    id,
    text,
    at: when,
    kind: "other",
    pages: Array.from({ length: 1 + (index % 3) }, (_, n) =>
      opened(null, `${text} — result ${n + 1}`, "example.org"),
    ),
  });
}

const minutes = (total: number, daysAgo = 0) => at(Math.floor(total / 60), total % 60, daysAgo);

/** A full Home: a day of twenty-two searches and two days before it, thirty-two in all. */
export const busyWire = (): Wire[] => [
  ...Array.from({ length: 22 }, (_, index) =>
    index === 0
      ? stripe()
      : index === 1
        ? grinders()
        : filler(index, "t", minutes(17 * 60 - index * 20)),
  ),
  ...Array.from({ length: 6 }, (_, index) => filler(index, "y", minutes(20 * 60 - index * 45, 1))),
  raycastOn(2),
  ...Array.from({ length: 3 }, (_, i) => filler(i + 1, "s", minutes(600 - i * 30, 2))),
];

/** The day the pictures show first: a large place opened up, and older searches under it. */
export const todayWire = (): Wire[] => [
  stripe(),
  grinders(),
  raycast(),
  address("t-gh", at(10, 2), "github.com", "Pull requests", "tab-gh"),
  compare(),
];

/** The tabs the scenes' pages are open in, as the tabs projection draws them. */
export const HOME_TABS = new Map(
  [
    tab("tab-pricing", "Pricing & fees | Stripe"),
    tab("tab-g1", "The best espresso grinders, tested"),
    tab("tab-g2", "Grinder advice for a first machine"),
    tab("tab-gh", "Pull requests · zephium"),
  ].map((one) => [one.id, one] as const),
);
