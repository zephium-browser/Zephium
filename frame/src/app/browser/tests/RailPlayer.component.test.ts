import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { MediaPlayerView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { favicons } from "$domain/favicons";
import { mediaPlayer } from "$domain/media-player";
import { preferences } from "$domain/preferences";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, settle } from "./look-bench";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    settingSet: async () => ({ accepted: true, operation_id: null }),
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async () => null,
    keymapEntries: async () => [],
    mediaPlayer: async () => ({ accepted: true, operation_id: "player" }),
  });
});

afterEach(() => {
  sidebar.setMode("default");
  mediaPlayer.dispose();
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
});

function view(over: Partial<MediaPlayerView> = {}): MediaPlayerView {
  return {
    tab_id: "tab-1",
    title: "Night Drive",
    artist: "Neon Coast",
    icon: null,
    duration: 200,
    position: 50,
    rate: 1,
    playing: true,
    waiting: false,
    can_pip: true,
    in_pip: false,
    muted: false,
    can_mute: true,
    ...over,
  };
}

async function rail() {
  await mediaPlayer.init();
  const screen = await render(Shell);
  sidebar.setMode("compact");
  // Paused, so the position is the one native sent and nothing moves.
  emitNativeEvent("mediaPlayerChanged", view({ playing: false }));
  await expect
    .poll(() => screen.container.querySelector(".media-disc"), { timeout: 5000 })
    .not.toBeNull();
  await settle();
  const query = <T extends Element>(selector: string) =>
    screen.container.querySelector<T>(selector)!;
  return { screen, query };
}

const box = (element: Element) => element.getBoundingClientRect();
const centre = (element: Element) => {
  const { left, top, width, height } = box(element);
  return { x: left + width / 2, y: top + height / 2 };
};
const near = (actual: number, expected: number) =>
  expect(Math.abs(actual - expected)).toBeLessThan(0.5);

test("the rail's player stands on the column's axis, as big as the profile button, with its icon and ring centred", async () => {
  await page.viewport(900, 640);
  await paint();
  const { query } = await rail();
  const disc = query<HTMLElement>(".media-disc");
  const profile = query<HTMLElement>(".shelf .case");
  const column = query<HTMLElement>(".sidebar-browser-column");

  near(centre(disc).x, centre(column).x);
  near(centre(disc).x, centre(profile).x);
  expect([box(disc).width, box(disc).height]).toEqual([box(profile).width, box(profile).height]);
  expect(box(disc).left).toBeGreaterThan(box(column).left);
  expect(box(disc).right).toBeLessThan(box(column).right);

  // The site's icon sits in the middle of the disc, and so does the ring.
  const icon = query(".media-disc-icon .favicon");
  near(centre(icon).x, centre(disc).x);
  near(centre(icon).y, centre(disc).y);
  const ring = query<SVGSVGElement>(".media-ring");
  expect(box(ring)).toEqual(box(disc));
  const track = query<SVGCircleElement>(".media-ring-track");
  near(centre(track).x, centre(disc).x);
  near(centre(track).y, centre(disc).y);
  near(box(track).width, box(track).height);
  expect(box(track).width).toBeLessThanOrEqual(box(disc).width);
});

test("the ring is one circle, and what has played is drawn clockwise from 12 o'clock", async () => {
  const { query } = await rail();
  const disc = query<HTMLElement>(".media-disc");
  const track = query<SVGCircleElement>(".media-ring-track");
  const fill = query<SVGCircleElement>(".media-ring-fill");
  expect(fill.getAttribute("r")).toBe(track.getAttribute("r"));
  expect(fill.getAttribute("stroke-dasharray")).toBe("25 100");

  const start = fill.getPointAtLength(0);
  const top = new DOMPoint(start.x, start.y).matrixTransform(fill.getScreenCTM()!);
  near(top.x, centre(disc).x);
  near(top.y, box(track).top);
  // A quarter on, it has reached 3 o'clock.
  const quarter = fill.getPointAtLength(fill.getTotalLength() / 4);
  const right = new DOMPoint(quarter.x, quarter.y).matrixTransform(fill.getScreenCTM()!);
  near(right.x, box(track).right);
  near(right.y, centre(disc).y);
});

test("a player with nothing played has no ring fill, not a dot, and a live one has none either", async () => {
  const { screen, query } = await rail();
  emitNativeEvent("mediaPlayerChanged", view({ playing: false, position: 0 }));
  await expect.poll(() => query(".media-ring-fill")).toBeNull();
  emitNativeEvent("mediaPlayerChanged", view({ playing: false, duration: null }));
  await expect.poll(() => screen.container.querySelectorAll(".media-ring circle").length).toBe(1);
});

test("with kept sites it stands a gap above them, and the profile a gap below the rule", async () => {
  await page.viewport(900, 640);
  await paint();
  const { query } = await rail();
  const disc = query(".media-disc");
  expect(box(query(".rail-sites")).top - box(disc).bottom).toBe(8);
  const rule = query(".dock-rule");
  expect(box(query(".shelf .case")).top - box(rule).bottom).toBe(8);
});

test("with none kept it stands one gap above the rule, as the profile does below it", async () => {
  await page.viewport(900, 640);
  await surface.init();
  await tabs.init();
  const tab = tabFixture({ projection_revision: revision(1001), url: "https://night.example/" });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(1002),
    profile: { id: "p", name: "Personal", kind: "default" },
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  });
  const { query } = await rail();
  const disc = query(".media-disc");
  const rule = query(".dock-rule");
  const profile = query(".shelf .case");
  expect(query<HTMLElement>(".rail-sites").hidden).toBe(true);
  expect(box(rule).top - box(disc).bottom).toBe(8);
  expect(box(profile).top - box(rule).bottom).toBe(8);
  near(centre(rule).x, centre(disc).x);
});

test("it plays or pauses, with a ring on the keyboard", async () => {
  const { screen, query } = await rail();
  const disc = query<HTMLElement>(".media-disc");
  disc.focus({ focusVisible: true } as FocusOptions);
  expect(disc.matches(":focus-visible")).toBe(true);
  expect(getComputedStyle(disc).outlineStyle).toBe("solid");
  await screen.getByRole("button", { name: "Play: Night Drive" }).click();
});
