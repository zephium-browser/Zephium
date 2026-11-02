import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { ProfileEntryView, ProfilesView, SpaceView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, spaceFixture, tabFixture } from "$shared/testing/fixtures";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import { keymap } from "$domain/keymap";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, settle, themed, wallpaper } from "./look-bench";

const native = vi.hoisted(() => ({
  settings: { appearance: "light" } as Record<string, string>,
  profiles: null as ProfilesView | null,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const ok = async () => ({ accepted: true, operation_id: null });
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async (key: string) => native.settings[key] ?? null,
    settingSet: ok,
    spacesSwitch: ok,
    spacesStep: ok,
    spacesUpdate: ok,
    blockerStatus: async () => ({
      status: "ok" as const,
      data: {
        site: {
          context: { profile: "p", tab: "github", site: "github.example", revision: "a" },
          paused: false,
          private_session: false,
          ready: true,
          busy: false,
          hides: [],
        },
        projection_revision: "0000000000000000000000000000000a",
        protection: "active" as const,
        phase: "ready" as const,
        preference: "authoritative" as const,
        config_revision: null,
        desired_enabled: true,
        applied_enabled: true,
        desired_generation: null,
        retained_generation: null,
        failure: null,
        retryable: false,
        retries_remaining: 0,
        applied_coverage: null,
        runtime_diagnostics: null,
        source_phase: "not_configured" as const,
        source_failure: null,
        source_package_revision: null,
        source_installed_revision: null,
        source_package_provenance: null,
        source_installed_provenance: null,
        source_identities: null,
        source_package_created_unix: null,
        source_package_expires_unix: null,
        source_package_stale: null,
        source_refresh_due: false,
        source_count: null,
        source_bytes: null,
        source_activation_pending: false,
        source_material_repair_pending: false,
        source_material_repair_retry_pending: false,
        source_repair_retry_pending: false,
        source_last_refresh_attempt_unix: null,
        source_refresh_operation: null,
        can_enable: true,
        can_refresh_sources: false,
      },
    }),
    blockerStats: async () => ({
      status: "ok" as const,
      data: { today: 128, last7Days: 1460, days: [] },
    }),
    spacesCreate: ok,
    tabDragOver: async () => undefined,
    tabDrop: ok,
    profilesGet: async () => ({ status: "ok" as const, data: native.profiles }),
    keymapEntries: async () =>
      [
        ["tool.notes", "Cmd+Shift+E"],
        ["tool.tasks", "Cmd+Shift+K"],
        ["tool.time", "Cmd+Shift+U"],
        ["tool.history", "Cmd+Y"],
        ["tool.downloads", "Cmd+Shift+J"],
        ["tool.bookmarks", "Cmd+Shift+B"],
        ["browser.settings", "Cmd+,"],
        ["space.select.1", "Ctrl+1"],
        ["space.select.2", "Ctrl+2"],
        ["space.select.3", "Ctrl+3"],
        ["space.select.4", "Ctrl+4"],
      ].map(([id, accelerator]) => ({
        id: id!,
        title: id!,
        group: "window" as const,
        accelerator: accelerator!,
        default_accelerator: accelerator!,
        customizable: true,
        customized: false,
      })),
  });
});

const mark = (key: string) => ({ origin: `https://${key}.example`, revision: "a" });
const SPACES: SpaceView[] = [
  spaceFixture("home", "Personal", {
    color: 1,
    tabs: 4,
    previews: [mark("github"), mark("spotify"), mark("ddg")],
  }),
  spaceFixture("work", "Studio", {
    icon: { kind: "emoji", text: "🎨" },
    color: 4,
    theme: { kind: "preset", id: "rose-pine" },
    tabs: 6,
    previews: [mark("slack"), mark("github"), mark("ddg")],
  }),
  spaceFixture("read", "Reading", {
    icon: { kind: "symbol", index: 5 },
    color: 6,
    tabs: 2,
    previews: [mark("spotify"), mark("ddg")],
  }),
  spaceFixture("side", "Side project", { icon: { kind: "symbol", index: 3 }, color: 8 }),
];

let projection = 50_000;
/** The bench's sidebar with real spaces; `empty` shows a space with no tabs. */
function spaced(active: string, empty = false) {
  const open = ["github", "spotify", "ddg", "slack"].map((key) =>
    tabFixture({
      id: key,
      title: key,
      url: `https://${key}.example/`,
      icon: { origin: `https://${key}.example`, revision: "a" },
    }),
  );
  const current = tabs.tabs();
  const kept = current.filter((tab) => ["telegram", "proton", "notion"].includes(tab.id));
  const titled = open.map((tab) => current.find((candidate) => candidate.id === tab.id) ?? tab);
  emitNativeEvent("itemsChanged", {
    projection_revision: revision((projection += 1)),
    profile: { id: "p", name: "Personal", kind: "default" },
    spaces: SPACES,
    active_space_id: active,
    nodes: [
      ...kept.map((tab) => ({
        id: tab.id,
        parent_id: null,
        section: "favorites" as const,
        kind: { type: "tab" as const, tab_id: tab.id },
      })),
      ...(empty ? [] : titled).map((tab) => ({
        id: tab.id,
        parent_id: null,
        section: "today" as const,
        kind: { type: "tab" as const, tab_id: tab.id },
      })),
    ],
    tabs: [...kept, ...(empty ? [] : titled)],
    active: empty ? null : "github",
    split_group: null,
  });
}

const shot = (name: string) =>
  page.screenshot({ path: `../../../../../target/look/spaces/${name}.png` });
const crop = (name: string) =>
  page.screenshot({
    path: `../../../../../target/look/spaces/${name}.png`,
    clip: { x: 0, y: 0, width: 300, height: 720 },
  } as Parameters<typeof page.screenshot>[0]);

const wide = (name: string) =>
  page.screenshot({
    path: `../../../../../target/look/spaces/${name}.png`,
    clip: { x: 0, y: 0, width: 420, height: 720 },
  } as Parameters<typeof page.screenshot>[0]);

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));

const ID = (n: number) => `01J${String(n).padStart(23, "0")}`;
const person = (n: number, name: string, color: number, revision = 1): ProfileEntryView => ({
  id: ID(n),
  name,
  kind: n === 1 ? "default" : "named",
  avatar: { style: n === 3 ? "symbol" : "initials", color, symbol: 4, revision },
});
const ONE: ProfilesView = {
  current: ID(1),
  private: false,
  profiles: [person(1, "Personal", 0, 0)],
};
const THREE: ProfilesView = {
  current: ID(1),
  private: false,
  profiles: [person(1, "Personal", 0, 0), person(2, "Work", 1), person(3, "Side Project", 8)],
};

afterEach(() => {
  sidebar.setMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  themed("zephium");
  document.getElementById("wallpaper")?.remove();
});

async function stage(theme: "light" | "dark") {
  await page.viewport(1100, 720);
  native.settings = { appearance: theme };
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.material = "liquid_glass";
  document.body.style.background = "transparent";
  await paint();
  await keymap.init();
  spaced("home");
  const screen = await render(Shell);
  wallpaper("violet", theme, theme === "light" ? "#ffffff" : "#0d1117");
  await settle();
  return screen;
}

const away = () => document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));

type Stage = Awaited<ReturnType<typeof stage>>;
type ColumnLook = {
  name: string;
  prepare?: (screen: Stage) => void | Promise<void>;
  capture?: typeof shot;
};

function rail(active = "read") {
  spaced(active);
  sidebar.setMode("compact");
}
async function railEditor(screen: Stage) {
  rail();
  await settle();
  await screen.getByRole("button", { name: "Spaces — Reading" }).click();
  await screen.getByRole("menuitem", { name: "Edit Reading" }).click();
}
const columnLooks: readonly ColumnLook[] = [
  { name: "expanded", capture: shot },
  { name: "expanded-icon", prepare: () => spaced("read") },
  {
    name: "switcher",
    prepare: async (screen) => {
      await screen.getByRole("button", { name: "Spaces — Personal" }).click();
    },
  },
  {
    name: "editor",
    prepare: async (screen) => {
      await screen.getByRole("button", { name: "Spaces — Personal" }).click();
      await screen.getByRole("button", { name: "Edit Studio" }).click();
    },
  },
  {
    name: "themed-space",
    capture: shot,
    prepare: () => {
      themed("rose-pine");
      spaced("work");
    },
  },
  {
    name: "themed-switcher",
    prepare: async (screen) => {
      themed("rose-pine");
      spaced("work");
      await screen.getByRole("button", { name: "Spaces — Studio" }).click();
    },
  },
  { name: "empty", prepare: () => spaced("side", true) },
  { name: "rail", prepare: () => rail() },
  {
    name: "rail-stack",
    prepare: async (screen) => {
      rail();
      await screen.getByRole("button", { name: "Spaces — Reading" }).click();
    },
  },
  { name: "rail-editor", capture: wide, prepare: railEditor },
  {
    name: "rail-after",
    prepare: async (screen) => {
      await railEditor(screen);
      await screen.getByRole("button", { name: "Done" }).click();
    },
  },
  { name: "rail-no-icon", prepare: () => rail("home") },
  {
    name: "site-info",
    prepare: async (screen) => {
      await screen.getByRole("button", { name: "Site information" }).click();
    },
  },
  {
    name: "beside-settings",
    capture: shot,
    prepare: async (screen) => {
      emitNativeEvent("uiCommand", "browser.settings");
      await expect
        .element(screen.getByRole("region", { name: "Settings", exact: true }))
        .toBeVisible();
    },
  },
];

for (const theme of ["light", "dark"] as const) {
  // Each retained image gets its own budget. The former fourteen-image walk
  // exceeded 15 seconds on CI and cancelled its final screenshot.
  for (const look of columnLooks) {
    test(`spaces in the column, ${look.name}, ${theme}`, async () => {
      const screen = await stage(theme);
      try {
        if (look.prepare) {
          await look.prepare(screen);
          await settle();
        }
        await (look.capture ?? crop)(`${theme}-${look.name}`);
      } finally {
        await screen.unmount();
      }
    });
  }

  test(`the spaces as one sheet mid-swipe, ${theme}`, async () => {
    await stage(theme);
    spaced("work");
    await settle();
    spaced("read");
    await settle();
    const list = document.querySelector<HTMLElement>(".tab-scroller")!;
    const wheel = (deltaX: number) => {
      const event = new WheelEvent("wheel", { deltaX, bubbles: true });
      Object.defineProperty(event, "webkitDirectionInvertedFromDevice", { value: true });
      list.dispatchEvent(event);
    };
    for (let step = 0; step < 14; step += 1) wheel(-8);
    await frame();
    await frame();
    await crop(`${theme}-swipe-mid`);
    for (let step = 0; step < 24; step += 1) wheel(8);
    await frame();
    await frame();
    await crop(`${theme}-swipe-back`);
  });

  test(`a tab carried across the column, ${theme}`, async () => {
    await stage(theme);
    const row = document.querySelector<HTMLElement>(`[data-zephium-tab-id="spotify"] .tab-open`)!;
    const from = row.getBoundingClientRect();
    const init = { bubbles: true, pointerId: 1, button: 0, isPrimary: true };
    const at = (x: number, y: number) => ({ ...init, clientX: x, clientY: y });
    row.dispatchEvent(new PointerEvent("pointerdown", at(from.left + 60, from.top + 20)));
    row.dispatchEvent(new PointerEvent("pointermove", at(from.left + 70, from.top + 30)));
    row.dispatchEvent(new PointerEvent("pointermove", at(from.left + 120, 600)));
    await frame();
    await settle();
    await crop(`${theme}-drag-below`);
    row.dispatchEvent(new PointerEvent("pointermove", at(800, 300)));
    await frame();
    await settle();
    await crop(`${theme}-drag-page`);
    row.dispatchEvent(new PointerEvent("pointercancel", at(800, 300)));
    row.dispatchEvent(new PointerEvent("pointerdown", at(from.left + 60, from.top + 20)));
    row.dispatchEvent(new PointerEvent("pointermove", at(from.left + 60, from.top + 30)));
    row.dispatchEvent(new PointerEvent("pointermove", at(from.left + 60, from.top + 100)));
    await frame();
    await settle();
    await crop(`${theme}-drag-reorder`);
    row.dispatchEvent(new PointerEvent("pointercancel", at(from.left + 60, from.top + 100)));
  });

  test(`the profile menu, ${theme}`, async () => {
    for (const [count, roster] of [
      [1, ONE],
      [3, THREE],
    ] as const) {
      native.profiles = roster;
      const screen = await stage(theme);
      emitNativeEvent("profilesChanged", roster);
      await screen.getByRole("button", { name: /^Tools/ }).click();
      await settle();
      await crop(`${theme}-profile-menu-${count}`);
      away();

      sidebar.setMode("compact");
      await settle();
      emitNativeEvent("profilesChanged", roster);
      await screen.getByRole("button", { name: /^Tools/ }).click();
      await settle();
      await crop(`${theme}-profile-rail-${count}`);
      away();

      // Back from the rail, the stack opens whole again.
      sidebar.setMode("default");
      await settle();
      await screen.getByRole("button", { name: /^Tools/ }).click();
      await settle();
      await crop(`${theme}-profile-after-rail-${count}`);
      away();
      await screen.unmount();
      sidebar.setMode("default");
      favicons.dispose();
      surface.dispose();
      tabs.dispose();
      preferences.dispose();
    }
  });
}
