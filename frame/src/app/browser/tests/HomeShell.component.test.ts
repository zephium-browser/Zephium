import "$styles/global.css";
import { afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { ItemsState, SidebarNodeView, TabView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, spaceFixture, tabFixture } from "$shared/testing/fixtures";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { init as showing } from "$session/home-show.svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as fixture from "$shared/testing/home-scenes";
import Shell from "../Shell.svelte";

const native = vi.hoisted(() => ({
  searches: [] as unknown[],
  calls: [] as unknown[][],
}));

// The new tab is drawn here as on macOS; NewTabMaterial covers Windows' caption band.
vi.mock("$shared/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));
vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { snapshot } = await import("$shared/testing/home-wire");
  const record =
    (name: string) =>
    async (...args: unknown[]) => {
      native.calls.push([name, ...args]);
      return { accepted: true, operation_id: null } as never;
    };
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    tabsActivate: record("activate"),
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async () => null,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    timeCall: async () => ({ kind: "focus_days" as const, days: [] }),
    blockerStats: async () => ({
      status: "ok" as const,
      data: { today: 1284, last7Days: 9120, days: [1100, 1250, 1400, 1310, 1380, 1396, 1284] },
    }),
    homeSnapshot: async () => snapshot(native.searches as never) as never,
    homeOpenTarget: record("open"),
    homeOpenResults: record("results"),
    homeFocusSearch: record("focus"),
    homeForgetSearch: record("forget"),
  });
});

beforeAll(async () => {
  await showing();
});
beforeEach(() => {
  native.searches = [];
  native.calls = [];
});
afterEach(() => {
  sidebar.setMode("default");
  surface.dispose();
  tabs.dispose();
  document.documentElement.dataset.theme = "dark";
});

const PAYMENTS = { id: "q-payments", text: "stripe pricing", results: false };
const GRINDERS = { id: "q-grinders", text: "best espresso grinder for beginners", results: false };

const TABS: TabView[] = [
  tabFixture({ id: "home", title: "New Tab", url: null, home: true }),
  tabFixture({ id: "wiki", title: "Wikipedia", url: "https://wikipedia.example/" }),
  tabFixture({
    id: "tab-pricing",
    title: "Pricing & fees | Stripe",
    url: "https://stripe.example/pricing",
    search: PAYMENTS,
  }),
  tabFixture({
    id: "tab-docs",
    title: "Stripe Docs",
    url: "https://stripe.example/docs",
    search: PAYMENTS,
  }),
  tabFixture({
    id: "tab-results",
    title: "stripe pricing - Search",
    url: "https://search.example/?q",
    search: { ...PAYMENTS, results: true },
  }),
  tabFixture({
    id: "tab-g1",
    title: "The best espresso grinders, tested",
    url: "https://seriouseats.example/",
    search: GRINDERS,
  }),
  tabFixture({
    id: "tab-g2",
    title: "Grinder advice for a first machine",
    url: "https://reddit.example/",
    search: GRINDERS,
  }),
];

let projection = 100;

async function open(
  options: {
    active?: string;
    home?: boolean;
    incognito?: boolean;
    tabs?: TabView[];
  } = {},
) {
  await surface.init();
  await tabs.init();
  projection += 1;
  // Tab views are kept across a test and admit only newer revisions.
  const all = (options.tabs ?? TABS).map((tab) => ({
    ...tab,
    projection_revision: revision(projection),
  }));
  const nodes: SidebarNodeView[] = all.map((tab) => ({
    id: tab.id,
    parent_id: null,
    section: "today",
    kind: { type: "tab", tab_id: tab.id },
  }));
  const items: ItemsState = {
    projection_revision: revision(projection),
    profile: options.incognito
      ? { id: "private", name: "Private", kind: "incognito" }
      : { id: "profile-a", name: "Personal", kind: "default" },
    spaces: [spaceFixture("space-a", "Personal")],
    active_space_id: "space-a",
    nodes,
    tabs: all,
    active: options.active ?? "home",
    split_group: null,
    home: options.home ?? true,
  };
  emitNativeEvent("itemsChanged", items);
  return render(Shell);
}

const one = (selector: string) => document.querySelectorAll(selector);
const row = (id: string) => document.querySelector<HTMLElement>(`[data-zephium-tab-id="${id}"]`);
const heads = () =>
  [...document.querySelectorAll<HTMLElement>(".search-head button")].map(
    (head) => head.textContent,
  );
const canvasDrawn = () =>
  expect
    .poll(() => one(".home-canvas .svelte-flow__node").length, { timeout: 10_000 })
    .toBeGreaterThan(2);

test("the left side heads with Home, gathers pages under their search and keeps plain pages plain", async () => {
  await page.viewport(1440, 900);
  native.searches = fixture.todayWire();
  await open();
  await expect.poll(() => row("home")).not.toBeNull();
  expect(one('[data-zephium-tab-id="home"]')).toHaveLength(1);
  const home = row("home")!;
  expect(home.closest(".sidebar-head")).not.toBeNull();
  expect(home.querySelector("[data-zephium-tab-label]")!.textContent).toBe("New Tab");
  expect(home.querySelector('[aria-current="page"]')).not.toBeNull();
  expect(one("[data-zephium-new-tab]")).toHaveLength(1);
  expect(document.querySelector('[aria-label="Switch between Browse and Work"]')).toBeNull();

  expect(heads()).toEqual(["stripe pricing", "best espresso grinder for beginners"]);
  for (const head of document.querySelectorAll(".search-head")) {
    expect(head.querySelector("[aria-expanded], [draggable='true'], svg")).toBeNull();
  }
  const list = [
    ...document.querySelectorAll<HTMLElement>("nav [data-zephium-tab-id], .search-head"),
  ];
  expect(
    list.map((element) => element.dataset.zephiumTabId ?? `head:${element.textContent?.trim()}`),
  ).toEqual([
    "wiki",
    "head:stripe pricing",
    "tab-pricing",
    "tab-docs",
    "head:best espresso grinder for beginners",
    "tab-g1",
    "tab-g2",
  ]);
  // The results page is listed only while it is the page in front.
  expect(row("tab-results")).toBeNull();
  await canvasDrawn();
  await expect.element(page.getByRole("button", { name: "Work", exact: true })).toBeVisible();
});

test("a group's header shows Home at its search, and the Home entry goes back to Home", async () => {
  await page.viewport(1440, 900);
  native.searches = fixture.todayWire();
  await open({ active: "tab-results" });
  await expect.poll(() => row("tab-results")).not.toBeNull();
  expect(row("home")!.querySelector('[aria-current="page"]')).toBeNull();
  expect(one("[data-zephium-new-tab]")).toHaveLength(0);

  document.querySelector<HTMLElement>('[aria-label="Pages from stripe pricing"]')!.click();
  await expect.poll(() => native.calls).toContainEqual(["focus", "q-payments"]);
  row("home")!.querySelector<HTMLElement>(".tab-open")!.click();
  await expect.poll(() => native.calls).toContainEqual(["activate", "home"]);
});

test("in the compact side the Home tab leads the rail and each group starts on a hairline", async () => {
  await page.viewport(1440, 900);
  native.searches = fixture.todayWire();
  sidebar.setMode("compact");
  await open();
  await expect.poll(() => row("home")).not.toBeNull();
  const rail = [...document.querySelectorAll<HTMLElement>("[data-zephium-tab-id]")].map(
    (element) => element.dataset.zephiumTabId,
  );
  expect(rail[0]).toBe("home");
  expect(rail).not.toContain("tab-results");
  expect(one("[data-zephium-tab-id='home']")).toHaveLength(1);
  expect(row("tab-pricing")!.classList).toContain("rail-group");
  expect(row("tab-g1")!.classList).toContain("rail-group");
  expect(row("tab-docs")!.classList).not.toContain("rail-group");
});

test("classic mode draws today's new tab and the Browse and Work switch", async () => {
  await page.viewport(1440, 900);
  native.searches = fixture.todayWire();
  const classic = TABS.map((tab) => (tab.home ? { ...tab, home: false } : tab));
  await open({ home: false, tabs: classic });
  await expect
    .poll(() => document.querySelector('[aria-label="Switch between Browse and Work"]'))
    .not.toBeNull();
  expect(document.querySelector(".search-head")).toBeNull();
  expect(document.querySelector(".sidebar-head [data-zephium-tab-id]")).toBeNull();
  expect(one("[data-zephium-new-tab]")).toHaveLength(1);
  await new Promise((done) => setTimeout(done, 500));
  expect(document.querySelector(".home-canvas")).toBeNull();
  expect(document.querySelector('[aria-label="Work"][aria-pressed]')).toBeNull();
});

test("a private window draws Home with a quiet line that nothing is remembered", async () => {
  await page.viewport(1440, 900);
  await open({ incognito: true, tabs: [TABS[0]!] });
  await expect
    .poll(() => document.body.textContent ?? "")
    .toContain("Nothing you search in a private window is remembered.");
  expect(one("[data-zephium-new-tab]")).toHaveLength(1);
});

test("the keyboard goes from the field down into the searches and back", async () => {
  await page.viewport(1440, 900);
  native.searches = fixture.todayWire();
  await open();
  await canvasDrawn();
  const field = document.querySelector<HTMLInputElement>("[data-zephium-new-tab] input")!;
  const stop = () => document.activeElement?.getAttribute("data-canvas-stop");
  field.focus();
  await userEvent.keyboard("{ArrowDown}");
  await expect.poll(stop).toBe("p:q-payments/0/pricing");
  await userEvent.keyboard("{ArrowDown}");
  await expect.poll(stop).toMatch(/^[pa]:q-payments\/0\/(?!pricing$)/u);
  await userEvent.keyboard("{Escape}");
  await expect.poll(() => document.activeElement).toBe(field);
  // Nothing still on its way into the canvas takes the keyboard back.
  await new Promise((done) => setTimeout(done, 600));
  expect(document.activeElement).toBe(field);
});

test("a search native showed is not shown again when Home is drawn anew", async () => {
  await page.viewport(1440, 900);
  native.searches = fixture.todayWire();
  const screen = await open();
  await canvasDrawn();
  const marked = () =>
    document
      .querySelector('[data-canvas-stop="p:q-raycast/0/teams"]')
      ?.getAttribute("aria-current");
  emitNativeEvent("homeShow", { search: "q-raycast", slot: 0, entry: "teams" });
  await expect.poll(marked).toBe("true");
  screen.unmount();
  tabs.dispose();
  surface.dispose();

  await open();
  await canvasDrawn();
  await new Promise((done) => setTimeout(done, 300));
  expect(marked()).not.toBe("true");
});

const shots = "../../../../../target/home-app-look";
const SIZES = [
  [1440, 900],
  [1100, 700],
] as const;

async function picture(name: string, theme: string, width: number, height: number) {
  await new Promise((done) => setTimeout(done, 800));
  await page.screenshot({ path: `${shots}/${name}-${theme}-${width}.png` });
  void height;
}

for (const theme of ["light", "dark"])
  for (const [width, height] of SIZES) {
    test(`Home pictures, ${theme}, ${width}`, async () => {
      document.documentElement.dataset.theme = theme;
      await page.viewport(width, height);

      let screen = await open({ tabs: [TABS[0]!, TABS[1]!] });
      await expect.poll(() => row("home")).not.toBeNull();
      await picture("rest", theme, width, height);
      screen.unmount();
      tabs.dispose();
      surface.dispose();

      native.searches = fixture.todayWire();
      screen = await open();
      await canvasDrawn();
      await picture("place-search", theme, width, height);

      emitNativeEvent("homeShow", { search: "q-raycast", slot: 0, entry: "teams" });
      await expect
        .poll(() =>
          document
            .querySelector('[data-canvas-stop="p:q-raycast/0/teams"]')
            ?.getAttribute("aria-current"),
        )
        .toBe("true");
      await picture("return", theme, width, height);
      screen.unmount();
      tabs.dispose();
      surface.dispose();

      screen = await open({ active: "tab-results" });
      await expect.poll(() => row("tab-results")).not.toBeNull();
      await picture("two-groups", theme, width, height);
      screen.unmount();
      tabs.dispose();
      surface.dispose();

      await open({ home: false, tabs: TABS.map((tab) => ({ ...tab, home: false })) });
      await expect
        .poll(() => document.querySelector('[aria-label="Switch between Browse and Work"]'))
        .not.toBeNull();
      await picture("classic", theme, width, height);
    }, 60_000);
  }
