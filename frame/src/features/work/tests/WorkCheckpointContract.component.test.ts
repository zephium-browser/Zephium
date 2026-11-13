import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import { WORK_PLACEMENT_PIN } from "$shared/ipc/work-constants";
import type {
  WorkArtifactDataV1,
  WorkCallV1,
  WorkEnvironmentSnapshot,
  WorkEnvironmentView,
} from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { projection } from "./environment-fixtures";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    faviconProbe: async () => true,
    workCall: native.call,
    workPaneHide: vi.fn(),
    workPaneSetRect: vi.fn(),
  });
});

/** The bounds `WorkEnvironmentView::validate` holds a checkpoint to. */
function faults(view: WorkEnvironmentView, elements: ReadonlySet<string>): string[] {
  const coordinate = (value: number) =>
    Number.isInteger(value) && value >= -1_000_000 && value <= 1_000_000;
  const within = (value: number, min: number, max: number) =>
    Number.isInteger(value) && value >= min && value <= max;
  const found: string[] = [];
  if (!coordinate(view.x) || !coordinate(view.y)) found.push("viewport");
  if (!within(view.zoom_milli, 100, 4000)) found.push("zoom");
  const seen = new Set<string>();
  for (const place of view.placements) {
    const name = `${place.element} ${place.x},${place.y} ${place.width}x${place.height}`;
    if (seen.has(place.element)) found.push(`duplicate ${name}`);
    seen.add(place.element);
    if (!elements.has(place.element)) found.push(`unknown ${name}`);
    if (!coordinate(place.x) || !coordinate(place.y)) found.push(`coordinate ${name}`);
    if (!within(place.width, 120, 4096) || !within(place.height, 80, 4096))
      found.push(`size ${name}`);
    if (![undefined, 0, 2, WORK_PLACEMENT_PIN].includes(place.revision ?? undefined))
      found.push(`revision ${name}`);
  }
  for (const area of view.areas ?? [])
    if (!within(area.width, 240, 8192) || !within(area.height, 160, 8192))
      found.push(`area ${area.area} ${area.width}x${area.height}`);
  return found;
}

const DIAGRAM: WorkArtifactDataV1 = {
  kind: "diagram",
  nodes: [
    { id: "web", name: "Browser", kind: "client" },
    { id: "api", name: "API", kind: "service", note: "Rust, axum" },
    { id: "db", name: "Postgres", kind: "store" },
  ],
  edges: [
    { from: "web", to: "api", label: "HTTPS" },
    { from: "api", to: "db", label: "SQL" },
  ],
};

test("every placement a canvas publishes holds to the checkpoint contract", async () => {
  await page.viewport(1400, 900);
  const profile = "contract-profile";
  const state = structuredClone(projection);
  state.work.profile = profile;
  const run = state.executions[0]!;
  const base = run.artifacts[0]!;
  run.user_artifacts = [];
  run.artifacts = [
    { ...base, id: "diagram", title: "Checkout system", data: DIAGRAM },
    {
      ...base,
      id: "steps",
      title: "Launch steps",
      data: {
        kind: "checklist",
        items: ["Book", "Pack", "Fly"].map((text) => ({ text, completed: false })),
      },
    },
    {
      ...base,
      id: "listings",
      title: "Listings",
      data: {
        kind: "findings",
        subjects: [{ name: "Flat on Valencia" }],
        items: [{ claim: "Quiet street", evidence: [], confidence: "supported" }],
      },
    },
  ];
  const artifact = (id: string) => ({
    id: `${id}-card`,
    area: null,
    reference: {
      kind: "artifact" as const,
      objective: "objective",
      execution: "execution",
      artifact: id,
    },
  });
  const initial: WorkEnvironmentSnapshot = {
    version: 1,
    profile,
    id: "work",
    space: "space",
    title: "Contract",
    revision: "1",
    lifecycle: "active",
    areas: [],
    view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
    elements: [
      {
        id: "objective-card",
        area: null,
        reference: { kind: "objective", objective: "objective" },
      },
      artifact("diagram"),
      artifact("steps"),
      artifact("listings"),
      {
        id: "subject-card",
        area: null,
        reference: {
          kind: "subject",
          objective: "objective",
          execution: "execution",
          artifact: "listings",
          index: 0,
        },
      },
    ],
  };
  const elements = new Set(initial.elements.map((element) => element.id));
  const environment = new WorkEnvironmentSession(profile, initial.space);
  environment.snapshot = initial;
  environment.selected = initial.id;
  const views: WorkEnvironmentView[] = [];
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind === "query")
      return { version: 1, profile, reply: { kind: "projection", projection: state } };
    if (call.kind === "environment" && call.request.kind === "checkpoint") {
      const request = call.request;
      views.push(request.view);
      const snapshot = {
        ...environment.snapshot!,
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
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot: environment.snapshot! } },
    };
  });
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "800px";
  root.style.width = "1300px";
  await expect.element(screen.getByText("Checkout system").first()).toBeVisible();
  await expect.element(screen.getByText("Flat on Valencia").first()).toBeVisible();
  await environment.flushView();
  await expect.poll(() => views.length).toBeGreaterThan(0);
  const published = views.at(-1)!;
  expect(published.placements.map((place) => place.element).sort()).toEqual([...elements].sort());
  for (const view of views) expect(faults(view, elements)).toEqual([]);
  await screen.unmount();
  environment.dispose();
});
