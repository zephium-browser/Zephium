import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import type { FocusStatus, SiteTimeView, TimeCall } from "$shared/ipc/bindings";
import { preferences } from "$domain/preferences";
import { focus } from "$domain/time";
import TimeLook from "./TimeLook.svelte";

const native = vi.hoisted(() => ({
  profile: "00000000000000000000000901",
  settings: {} as Record<string, string>,
  control: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

// A working day: mornings on code and planning, a long lunch on video.
const DAY = [
  0, 0, 0, 0, 0, 0, 0, 0, 840, 2950, 3300, 2100, 1500, 2600, 3120, 1830, 0, 0, 0, 0, 0, 0, 0, 0,
];
const WEEK = [5.2, 6.1, 4.4, 6.8, 5.9, 1.2, 0].map((hours) => hours * 3600);
const series = (share: number, source: number[]) =>
  source.map((value) => Math.round(value * share));
const site = (
  name: string,
  seconds: number,
  opens: number,
  share: number | null,
  source: number[],
): SiteTimeView => ({
  site: name,
  icon: null,
  seconds,
  opens,
  series: share === null ? [] : series(share, source),
});

function report(call: TimeCall) {
  if (call.kind === "focus_days") {
    return {
      kind: "focus_days" as const,
      days: [{ day: call.from_day, seconds: 3000, sessions: 2, completed: 1 }],
    };
  }
  const source = call.bucket_hours === 1 ? DAY : WEEK;
  const total = source.reduce((sum, value) => sum + value, 0);
  const sites = [
    site("github.com", Math.round(total * 0.34), 14, 0.34, source),
    site("linear.app", Math.round(total * 0.21), 9, 0.21, source),
    site("youtube.com", Math.round(total * 0.16), 4, 0.16, source),
    site("figma.com", Math.round(total * 0.11), 6, 0.11, source),
    site("x.com", Math.round(total * 0.07), 11, null, source),
    site("notion.so", Math.round(total * 0.05), 3, null, source),
    site("vercel.com", Math.round(total * 0.03), 5, null, source),
  ];
  const narrowed = call.site ? sites.filter((entry) => entry.site === call.site) : sites;
  return {
    kind: "report" as const,
    buckets: source.map((value) => ({
      browse: Math.round(value * 0.82),
      work: Math.round(value * 0.18),
    })),
    previous: { browse: Math.round(total * 0.95), work: 600 },
    sites: narrowed,
  };
}

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key: string) => native.settings[key] ?? null,
    timeCall: async (_profile: string, call: TimeCall) => report(call),
    focusControl: native.control,
  });
});
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: native.profile, name: "Alex", kind: "default" }) },
}));

afterEach(() => {
  vi.restoreAllMocks();
  preferences.dispose();
  focus.dispose();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.reduceMotion;
});

const settled = () =>
  Promise.all(
    document.getAnimations().map((animation) => animation.finished.catch(() => undefined)),
  );

async function capture(container: HTMLElement, name: string) {
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    container.style.background = "var(--color-canvas)";
    await settled();
    await page
      .elementLocator(container)
      .screenshot({ path: `../../../../../target/time/${name}-${theme}.png` });
  }
}

async function setup(view: "panel" | "page" | "cover", span: "day" | "week" = "day") {
  native.settings = { "focus.blocked": "youtube.com\nx.com", "focus.minutes": "25" };
  await preferences.init();
  await focus.init();
  await page.viewport(view === "panel" ? 360 : 1240, 900);
  const screen = await render(TimeLook, { profile: native.profile, view, span });
  screen.container.style.cssText =
    view === "panel" ? "inline-size:336px;block-size:880px" : "inline-size:1220px;block-size:880px";
  return screen;
}

const running: FocusStatus = {
  session: {
    phase: "focus",
    started_at: Date.now() - 7 * 60_000,
    phase_started_at: Date.now() - 7 * 60_000,
    phase_ends_at: Date.now() + 18 * 60_000 + 4_000,
    minutes: 25,
    breaks: true,
    break_minutes: 5,
    long_break_minutes: 15,
    rounds: 1,
    focused: 1500,
    allowed: [],
  },
  shut: [
    { site: "youtube.com", icon: null },
    { site: "x.com", icon: null },
  ],
};

test("the sidebar shows the day, focus and where the time went", async () => {
  const screen = await setup("panel");
  await expect.element(screen.getByText("github.com")).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Start focus" })).toBeVisible();
  await capture(screen.container, "panel-day");
  await screen.getByRole("button", { name: "Shuts 2 sites" }).click();
  await expect.element(screen.getByText("From your time")).toBeVisible();
  await capture(screen.container, "panel-shut");
  await screen.getByRole("button", { name: "Shuts 2 sites" }).click();
  emitNativeEvent("focusChanged", running);
  await expect.element(screen.getByText(/Round 2/u)).toBeVisible();
  await capture(screen.container, "panel-focusing");
});

test("the sidebar week reads by day", async () => {
  const screen = await setup("panel", "week");
  await expect.element(screen.getByText("github.com")).toBeVisible();
  await capture(screen.container, "panel-week");
});

test("the page shows a period, a site in depth and the sites focus shuts", async () => {
  const screen = await setup("page");
  await expect.element(screen.getByRole("button", { name: /github\.com/u }).first()).toBeVisible();
  await capture(screen.container, "page-day");
  await screen
    .getByRole("button", { name: /linear\.app/u })
    .first()
    .click();
  await expect.element(screen.getByRole("heading", { name: "linear.app" })).toBeVisible();
  await capture(screen.container, "page-site");
});

test("a shut page is covered with the round's time and a way out", async () => {
  const screen = await setup("cover");
  emitNativeEvent("focusChanged", running);
  await expect.element(screen.getByRole("heading", { name: "Paused for focus" })).toBeVisible();
  await screen.getByRole("button", { name: "Allow 5 minutes" }).click();
  expect(native.control).toHaveBeenCalledWith({ kind: "allow", site: "youtube.com" });
  await capture(screen.container, "cover");
});

test.each([
  [false, false],
  [true, false],
  [false, true],
  [true, true],
])("site management honors OS reduced motion %s and app reduced motion %s", async (os, app) => {
  const matchMedia = window.matchMedia.bind(window);
  vi.spyOn(window, "matchMedia").mockImplementation((query) =>
    query === "(prefers-reduced-motion: reduce)"
      ? { ...matchMedia(query), matches: os }
      : matchMedia(query),
  );
  const scroll = vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => {});
  const screen = await setup("page");
  document.documentElement.dataset.reduceMotion = String(app);
  await screen.getByRole("button", { name: "Shuts 2 sites" }).click();
  expect(scroll).toHaveBeenCalledWith({
    block: "start",
    behavior: os || app ? "instant" : "smooth",
  });
});
