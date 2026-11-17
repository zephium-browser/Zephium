import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkCallV1, WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import { wakeupCounter } from "$shared/testing/idle";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    faviconProbe: async () => true,
    workCall: native.call,
    workPaneShow: async () => ({ accepted: true, operation_id: null }),
    workPaneHide: async () => ({ accepted: true, operation_id: null }),
    workPaneSetRect: async () => {},
  });
});

const counter = wakeupCounter();
const profile = "00000000000000000000000001";
const space = "00000000000000000000000002";

function work(elements: WorkEnvironmentSnapshot["elements"]): WorkEnvironmentSnapshot {
  return {
    version: 1,
    id: "00000000000000000000000003",
    profile,
    space,
    title: "Research",
    lifecycle: "active",
    revision: "1",
    elements,
    areas: [],
    view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
  };
}

test.each([
  ["a new work", work([])],
  [
    "a work of tabs",
    work(
      Array.from({ length: 12 }, (_, index) => ({
        id: `0000000000000000000000001${index.toString(16)}`,
        area: null,
        reference: { kind: "browser" as const, tab: `tab-${index}` },
      })),
    ),
  ],
])("Work open on %s, left alone, wakes for nothing", async (name, snapshot) => {
  await page.viewport(1200, 800);
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("no runs here");
    return {
      version: 1,
      profile,
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
    };
  });
  const session = new WorkEnvironmentSession(profile, space);
  session.snapshot = snapshot;
  session.selected = snapshot.id;
  const screen = await render(WorkEnvironmentWorkspace, {
    session,
    tabs: Array.from({ length: 12 }, (_, index) =>
      tabFixture({ id: `tab-${index}`, title: `Tab ${index}` }),
    ),
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  // Past every arrival, the camera settling and the canvas letting its layer go.
  await new Promise((done) => setTimeout(done, 3000));
  const rest = await counter.rest(3000);
  console.warn("work-idle", name, JSON.stringify(rest));
  expect(rest.frames).toBe(0);
  expect(rest.timers).toBe(0);
  expect(rest.animations).toEqual([]);
  await screen.unmount();
});
