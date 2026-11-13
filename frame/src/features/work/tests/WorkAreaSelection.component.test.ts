import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkCallV1, WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { marquee } from "./marquee";
const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({ faviconProbe: async () => true, workCall: native.call });
});

test("Area from a selection creates the area, moves every selected element in, and places it around them", async () => {
  await page.viewport(1200, 800);
  const profile = "00000000000000000000000001";
  const space = "00000000000000000000000002";
  const first = "00000000000000000000000004";
  const second = "00000000000000000000000005";
  let snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    id: "00000000000000000000000003",
    profile,
    space,
    title: "Keyboards",
    lifecycle: "active",
    revision: "1",
    elements: [
      { id: first, area: null, reference: { kind: "browser", tab: "tab-1" } },
      { id: second, area: null, reference: { kind: "browser", tab: "tab-2" } },
    ],
    areas: [],
    view: {
      revision: "1",
      x: 0,
      y: 0,
      zoom_milli: 1000,
      placements: [
        { element: first, x: 80, y: 120, width: 280, height: 96 },
        { element: second, x: 420, y: 120, width: 280, height: 96 },
      ],
    },
  };
  const edits: unknown[] = [];
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("unexpected call");
    const request = call.request;
    if (request.kind === "command" && request.intent.kind === "edit") {
      const edit = request.intent.edit;
      edits.push(edit);
      const revision = String(Number(snapshot.revision) + 1);
      if (edit.kind === "create_area")
        snapshot = { ...snapshot, revision, areas: [{ id: "area-1", title: edit.title }] };
      if (edit.kind === "assign_area")
        snapshot = {
          ...snapshot,
          revision,
          elements: snapshot.elements.map((element) =>
            element.id === edit.element ? { ...element, area: edit.area } : element,
          ),
        };
      return {
        version: 1,
        profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "applied",
            command: request.command,
            applied_revision: snapshot.revision,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot,
          },
        },
      };
    }
    if (request.kind === "checkpoint") {
      snapshot = {
        ...snapshot,
        view: { ...request.view, revision: String(BigInt(request.expected) + 1n) },
      };
      return {
        version: 1,
        profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "checkpointed",
            expected: request.expected,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot,
          },
        },
      };
    }
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
    tabs: [
      tabFixture({ id: "tab-1", title: "Keychron K2" }),
      tabFixture({ id: "tab-2", title: "Nuphy Air75" }),
    ],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  await expect.poll(() => screen.container.querySelectorAll(".work-drag-handle").length).toBe(2);
  const flow = screen.container.querySelector(".svelte-flow")!.getBoundingClientRect();
  await userEvent.keyboard("{Shift>}");
  await marquee(
    screen.container,
    [flow.left + 60, flow.top + 100],
    [flow.left + 720, flow.top + 240],
  );
  await userEvent.keyboard("{/Shift}");
  await screen
    .getByRole("toolbar", { name: "2 selected" })
    .getByRole("button", { name: "Area", exact: true })
    .click();
  await expect
    .poll(() => edits)
    .toEqual([
      { kind: "create_area", title: "Keychron K2" },
      { kind: "assign_area", element: first, area: "area-1" },
      { kind: "assign_area", element: second, area: "area-1" },
    ]);
  await session.flushView();
  await expect
    .poll(() => snapshot.view.areas)
    .toEqual([{ area: "area-1", x: 48, y: 52, width: 684, height: 196 }]);
  await screen.unmount();
  session.dispose();
});
