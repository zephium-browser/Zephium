import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { wakeupCounter } from "$shared/testing/idle";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import Shell from "../Shell.svelte";
import { loadSettings } from "$features/settings";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
  });
});

// Prepare the lazy screen before measuring its settled wakeups.
await loadSettings();
const counter = wakeupCounter();
afterEach(() => {
  surface.dispose();
  tabs.dispose();
});

/** A window someone has been browsing in all day: many tabs, a few pinned, none loading. */
function day(count: number) {
  const list = Array.from({ length: count }, (_, index) =>
    tabFixture({
      id: `tab-${index}`,
      projection_revision: revision(index + 1),
      title: `Page ${index}`,
      url: `https://site-${index % 12}.example/${index}`,
    }),
  );
  return {
    projection_revision: revision(1000),
    profile: { id: "profile", name: "Personal", kind: "default" as const },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    tabs: list,
    active: list[0]!.id,
    split_group: null,
  };
}

test.each(["browse", "settings"] as const)(
  "the window at rest on %s wakes for nothing",
  async (where) => {
    await surface.init();
    await tabs.init();
    emitNativeEvent("itemsChanged", day(60));
    await render(Shell);
    if (where === "settings") emitNativeEvent("uiCommand", "browser.settings");
    // Past every arrival and settle motion.
    await new Promise((done) => setTimeout(done, 1500));
    const rest = await counter.rest(3000);
    console.warn("shell-idle", where, JSON.stringify(rest));
    expect(rest.frames).toBe(0);
    expect(rest.timers).toBe(0);
    expect(rest.animations).toEqual([]);
  },
);
