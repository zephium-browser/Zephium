import { afterEach, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { SidebarNodeView, TabView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as launch from "$session/motion.svelte";
import Shell from "../Shell.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});

afterEach(() => {
  sidebar.setMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
});

type Mark = (context: CanvasRenderingContext2D) => void;

/** A site's mark, drawn into the same fixed raster native would deliver. */
function raster(draw: Mark): string {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 32;
  const context = canvas.getContext("2d")!;
  draw(context);
  const { data } = context.getImageData(0, 0, 32, 32);
  let binary = "";
  for (const byte of data) binary += String.fromCharCode(byte);
  return btoa(binary);
}

const plate =
  (fill: string, glyph?: { text: string; color: string; font?: string }): Mark =>
  (context) => {
    context.fillStyle = fill;
    context.beginPath();
    context.roundRect(0, 0, 32, 32, 7);
    context.fill();
    if (!glyph) return;
    context.fillStyle = glyph.color;
    context.font = glyph.font ?? "bold 20px -apple-system";
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.fillText(glyph.text, 16, 17);
  };
const disc =
  (fill: string): Mark =>
  (context) => {
    context.fillStyle = fill;
    context.beginPath();
    context.arc(16, 16, 15, 0, Math.PI * 2);
    context.fill();
  };

const SITES: Record<string, { title: string; mark: Mark }> = {
  hugeicons: { title: "Pricing Plans | Hugeicons", mark: plate("#3fb45f") },
  github: { title: "GitHub - crynta/terax-ai: Lightweight AI", mark: disc("#e6e6e6") },
  x: { title: "X. It's what's happening / X", mark: plate("#000", { text: "X", color: "#fff" }) },
  wikipedia: {
    title: "Wikipedia",
    mark: plate("#fff", { text: "W", color: "#000", font: "22px Times" }),
  },
  youtube: { title: "Page failed to open", mark: plate("#ff0033", { text: "▶", color: "#fff" }) },
  telegram: { title: "Telegram Desktop", mark: disc("#2aa3df") },
  notion: {
    title: "The AI workspace that works for you",
    mark: plate("#fff", { text: "N", color: "#000" }),
  },
  proton: {
    title: "Download VPN | Proton VPN",
    mark: plate("#6d4aff", { text: "▼", color: "#fff" }),
  },
};

function tab(key: string, index: number): TabView {
  const site = SITES[key]!;
  return tabFixture({
    id: `${key}-${index}`,
    title: site.title,
    url: `https://${key}.example/`,
    icon: { origin: `https://${key}.example`, revision: "a" },
    // One page mid-load, so the loading mark is part of the reference.
    loading: key === "github",
  });
}

// The tab model outlives a test and admits only newer projections.
let projection = 10;

async function paint(options: { kept: string[]; open: string[]; active: string }) {
  // A reference of the list at rest, not of a launch waiting for its window.
  launch.dispose();
  await surface.init();
  await tabs.init();
  await favicons.init();
  const kept = options.kept.map(tab);
  const open = options.open.map(tab);
  const node = (id: string, section: "favorites" | "today"): SidebarNodeView => ({
    id,
    parent_id: null,
    section,
    kind: { type: "tab", tab_id: id },
  });
  emitNativeEvent("favicons", {
    surface: "chrome",
    profile_id: "p",
    entries: Object.entries(SITES).map(([key, site]) => ({
      origin: `https://${key}.example`,
      revision: "a",
      rgba: raster(site.mark),
    })),
  });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision((projection += 1)),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [
      ...kept.map((entry) => node(entry.id, "favorites")),
      ...open.map((entry) => node(entry.id, "today")),
    ],
    tabs: [...kept, ...open],
    active: [...kept, ...open].find((entry) => entry.id.startsWith(options.active))!.id,
    split_group: null,
  });
}

const shot = (name: string) => page.screenshot({ path: `../../../../../target/look/${name}.png` });

// A rendered reference for the whole window's chrome, at Retina scale, so
// shape and rhythm can be judged the way they will actually be seen.
test("the window at rest", async () => {
  await page.viewport(1100, 720);
  document.documentElement.dataset.theme = "dark";
  document.body.style.background = "#232326";
  await paint({
    kept: ["telegram", "notion", "proton"],
    open: ["hugeicons", "github", "x", "wikipedia", "youtube", "wikipedia"],
    active: "wikipedia",
  });
  const screen = await render(Shell);
  await shot("window-dark");

  await screen.getByRole("button", { name: "X. It's what's happening / X", exact: true }).hover();
  await shot("window-dark-hover");

  await screen.getByRole("button", { name: "Tools", exact: true }).click();
  await settle();
  await shot("window-dark-shelf");
  await page.getByRole("button", { name: "Tools", exact: true }).click();

  sidebar.setMode("compact");
  await settle();
  await shot("window-dark-compact");
  await screen.getByRole("button", { name: "Tools", exact: true }).click();
  await settle();
  await shot("window-dark-compact-shelf");
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 450));

test("the dock, crowded and empty", async () => {
  await page.viewport(1100, 720);
  document.documentElement.dataset.theme = "dark";
  document.body.style.background = "#232326";
  await paint({
    kept: ["telegram", "notion", "proton", "github", "x", "youtube"],
    open: ["hugeicons", "wikipedia"],
    active: "wikipedia",
  });
  const screen = await render(Shell);
  await settle();
  await shot("dock-crowded");
  screen.unmount();
  tabs.dispose();
  favicons.dispose();
  surface.dispose();

  await paint({ kept: [], open: ["hugeicons", "wikipedia"], active: "wikipedia" });
  await render(Shell);
  await settle();
  await shot("dock-empty");
});
