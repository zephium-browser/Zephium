import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { WorkEnvironmentSnapshot, WorkRuntimeProjection } from "$shared/ipc/bindings";
import { WorkEnvironmentSession } from "$domain/work-environment";
import { dinnerScene } from "./board-fixtures";

const native = vi.hoisted(() => ({
  snapshot: null as unknown,
  projection: null as unknown,
  edits: 0,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  const reply = (profile: string, body: unknown) => ({ version: 1, profile, reply: body });
  return mockComposerBindings({
    faviconProbe: async () => true,
    workActivity: (async (profile: string, work: string) => ({
      version: 1,
      profile,
      work,
      signals: [],
      pages: [],
      error: null,
    })) as never,
    workHumanPages: (async (profile: string, work: string) => ({
      version: 1,
      profile,
      work,
      pages: [],
      error: null,
    })) as never,
    workCall: (async (profile: string, call: { kind: string; request: { kind: string } }) => {
      if (call.kind === "environment" && call.request.kind === "command") {
        native.edits += 1;
        // A macrotask per refusal: a retry loop shows as a count, not a frozen page.
        await new Promise((done) => setTimeout(done, 0));
        return reply(profile, { kind: "error", error: "conflict" });
      }
      if (call.kind === "environment")
        return reply(profile, {
          kind: "environment",
          reply: { kind: "snapshot", snapshot: native.snapshot },
        });
      const query = (call.request as { query?: { kind: string } }).query;
      if (query?.kind === "projection")
        return reply(profile, { kind: "projection", projection: native.projection });
      return reply(profile, { kind: "error", error: "not_found" });
    }) as never,
  });
});

test("a run whose placing is refused is not asked again until the work changes", async () => {
  await page.viewport(1200, 800);
  const scene = dinnerScene();
  const projection = [...scene.objectives.values()][0]! as WorkRuntimeProjection;
  // Only the request stands: everything the run made is still to be placed.
  const snapshot: WorkEnvironmentSnapshot = {
    ...scene.snapshot,
    elements: scene.snapshot.elements.filter((element) => element.reference.kind === "objective"),
  };
  native.snapshot = snapshot;
  native.projection = projection;
  const session = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
  session.snapshot = snapshot;
  session.selected = snapshot.id;
  const { default: WorkEnvironmentWorkspace } =
    await import("../components/WorkEnvironmentWorkspace.svelte");
  const screen = await render(WorkEnvironmentWorkspace, {
    session,
    tabs: [],
    spaceName: "Personal",
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  await expect.poll(() => native.edits, { timeout: 5000 }).toBeGreaterThan(0);
  await new Promise((done) => setTimeout(done, 1500));
  expect(native.edits).toBe(1);
  await screen.unmount();
});
