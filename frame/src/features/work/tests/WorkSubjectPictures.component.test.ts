import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type {
  WorkCallV1,
  WorkEnvironmentSnapshot,
  WorkRuntimeProjection,
} from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";

const native = vi.hoisted(() => ({ call: vi.fn(), admit: vi.fn(), resource: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    workCall: native.call,
    mediaAdmitRemote: native.admit,
    resourceCall: native.resource,
    workPaneHide: vi.fn(),
    workPaneSetRect: vi.fn(),
  });
});

const profile = "picture-profile";
const names = ["One", "Two", "Three", "Four", "Five", "Six", "Seven"];
/** Three refuses its first picture for good; Five has one picture, refused once. */
const candidates = (name: string) =>
  name === "Three"
    ? ["https://cdn.example.test/three-broken.jpg", "https://cdn.example.test/three.jpg"]
    : name === "Five"
      ? ["https://cdn.example.test/five-flaky.jpg"]
      : [`https://cdn.example.test/${name.toLowerCase()}.jpg`];

const limits = {
  model_tokens: 10,
  cost_micro_usd: 0,
  operations: 1,
  timeout_seconds: 60,
  max_workers: 1,
};
const projection: WorkRuntimeProjection = {
  version: 1,
  interrupted: [],
  executions: [
    {
      id: "execution",
      approved_revision: "2",
      status: "completed",
      attempts: [],
      spec: { plan_revision: "2", limits, nodes: [] },
      artifacts: [
        {
          version: 1,
          id: "artifact",
          execution: "execution",
          node: "node",
          attempt: "attempt",
          output: "Listings",
          title: "Listings",
          review: "source_mapped_needs_review",
          presentation: "automatic",
          evidence: [],
          data: {
            kind: "findings",
            subjects: names.map((name) => ({ name, image_candidates: candidates(name) })),
            items: [],
          },
        },
      ],
    },
  ],
  work: {
    schema_version: 2,
    profile,
    id: "objective",
    revision: "4",
    lifecycle: "active",
    objective: "Compare listings",
    objective_revision: "1",
    context_revision: "1",
    objective_author: "user",
    questions: [],
    status: "plan_ready",
    plan: null,
  },
};

test("seven subjects with picture candidates all end up with a picture", async () => {
  await page.viewport(1200, 800);
  let snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    profile,
    id: "work",
    space: "space",
    title: "Trip",
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
      ...names.map((_, index) => ({
        id: `subject-${index}`,
        area: null,
        reference: {
          kind: "subject" as const,
          objective: "objective",
          execution: "execution",
          artifact: "artifact",
          index,
        },
      })),
    ],
  };
  const environment = new WorkEnvironmentSession(profile, snapshot.space);
  environment.snapshot = snapshot;
  environment.selected = snapshot.id;
  native.resource.mockResolvedValue({ profile, response: { kind: "error", error: "not_found" } });
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind === "query")
      return { version: 1, profile, reply: { kind: "projection", projection } };
    if (call.kind === "environment" && call.request.kind === "checkpoint") {
      const request = call.request;
      snapshot = {
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
  const flaky = new Set<string>();
  native.admit.mockImplementation(
    async (_profile: string, _environment: string, element: string, url: string) => {
      const refused = { status: "ok", data: { kind: "refused", error: "unavailable" } };
      if (url.includes("broken")) return refused;
      if (url.includes("flaky") && !flaky.has(url)) {
        flaky.add(url);
        return refused;
      }
      const media = `media-${element}`;
      const current = environment.snapshot!;
      environment.snapshot = {
        ...current,
        revision: String(BigInt(current.revision) + 1n),
        elements: [
          ...current.elements,
          {
            id: media,
            area: null,
            reference: { kind: "resource", resource: `resource-${element}` },
          },
        ],
        relations: [
          ...(current.relations ?? []),
          {
            id: `uses-${element}`,
            from: element,
            to: media,
            kind: "uses",
            origin: { kind: "user" },
          },
        ],
      };
      return { status: "ok", data: { kind: "admitted", element: media } };
    },
  );
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const pictured = () =>
    names.filter((_, index) =>
      (environment.snapshot?.relations ?? []).some(
        (relation) => relation.from === `subject-${index}` && relation.kind === "uses",
      ),
    ).length;
  await expect.poll(pictured, { timeout: 10_000 }).toBe(7);
  const tried = native.admit.mock.calls.map((call) => call[3] as string);
  expect(tried.filter((url) => url.includes("three-broken"))).toHaveLength(1);
  expect(tried).toContain("https://cdn.example.test/three.jpg");
  expect(tried.filter((url) => url.includes("five-flaky"))).toHaveLength(2);
  await screen.unmount();
  environment.dispose();
});
