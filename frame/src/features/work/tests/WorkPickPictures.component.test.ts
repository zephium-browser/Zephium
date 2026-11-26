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
// Admitted pictures resolve to a real one-pixel image here, each naming its digest.
vi.mock("$domain/resources", async (original) => ({
  ...(await original<typeof import("$domain/resources")>()),
  pictureUrl: (picture: { digest: string }) =>
    `data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7#${picture.digest}`,
}));
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

const profile = "01ARZ3NDEKTSV4RRFFQ69G5FA2";
const homes = ["Loft", "Studio", "Garden flat", "Plain room"];
/** The Studio's first photo is refused for good; the Plain room has none to fetch. */
const photos = (name: string) =>
  name === "Studio"
    ? ["https://a0.muscache.test/studio-broken.jpg", "https://a0.muscache.test/studio.jpg"]
    : name === "Plain room"
      ? []
      : [`https://a0.muscache.test/${name.toLowerCase().replace(" ", "-")}.jpg`];
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
          output: "Stays",
          title: "Stays in San Francisco",
          review: "source_mapped_needs_review",
          presentation: "automatic",
          evidence: [],
          data: {
            kind: "picks",
            facet: "stay",
            items: homes.map((name) => ({
              name,
              image_candidates: photos(name),
              logo_host: "airbnb.com",
              facts: [],
              tags: [],
              recommended: false,
            })),
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
    objective: "Find a stay",
    objective_revision: "1",
    context_revision: "1",
    objective_author: "user",
    questions: [],
    status: "plan_ready",
    plan: null,
  },
};
const digest = (url: string) =>
  [...url]
    .reduce((sum, char) => (sum * 31 + char.charCodeAt(0)) >>> 0, 7)
    .toString(16)
    .padStart(64, "0");

test("each pick with photo candidates is admitted its own photo, and draws it", async () => {
  await page.viewport(1400, 900);
  const snapshot: WorkEnvironmentSnapshot = {
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
      {
        id: "picks",
        area: null,
        reference: {
          kind: "artifact",
          objective: "objective",
          execution: "execution",
          artifact: "artifact",
        },
      },
    ],
  };
  const environment = new WorkEnvironmentSession(profile, snapshot.space);
  environment.snapshot = snapshot;
  environment.selected = snapshot.id;
  /** Each admitted photo's resource, by resource id: the address it was fetched from. */
  const fetched = new Map<string, string>();
  native.resource.mockImplementation(
    async (_profile: string, call: { kind: string; id?: string }) => {
      const url = call.kind === "get" && call.id ? fetched.get(call.id) : undefined;
      if (!url) return { profile, response: { kind: "error", error: "not_found" } };
      return {
        profile,
        response: {
          kind: "record",
          record: {
            id: call.id,
            revision: "1",
            created_at: "0",
            updated_at: "0",
            trashed: false,
            draft: {
              title: "photo.jpg",
              pinned: false,
              related: [],
              content: {
                kind: "media",
                asset: {
                  version: 1,
                  kind: "image",
                  mime: "image/jpeg",
                  bytes: 10,
                  digest: digest(url),
                  name: "photo.jpg",
                  origin: { kind: "fetched", url, observed_at: "0" },
                },
              },
            },
          },
        },
      };
    },
  );
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind === "query")
      return { version: 1, profile, reply: { kind: "projection", projection } };
    return {
      version: 1,
      profile,
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot: environment.snapshot! } },
    };
  });
  native.admit.mockImplementation(
    async (_profile: string, _environment: string, element: string, url: string) => {
      if (url.includes("broken"))
        return { status: "ok", data: { kind: "refused", error: "unavailable" } };
      const index = fetched.size;
      const resource = `resource-${index}`;
      fetched.set(resource, url);
      const media = `media-${index}`;
      const current = environment.snapshot!;
      environment.snapshot = {
        ...current,
        revision: String(BigInt(current.revision) + 1n),
        elements: [
          ...current.elements,
          { id: media, area: null, reference: { kind: "resource", resource } },
        ],
        relations: [
          ...(current.relations ?? []),
          { id: `uses-${index}`, from: element, to: media, kind: "uses", origin: { kind: "user" } },
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
  await expect.poll(() => fetched.size, { timeout: 10_000 }).toBe(3);
  const tried = native.admit.mock.calls.map((call) => call[3] as string);
  expect(tried.filter((url) => url.includes("studio-broken"))).toHaveLength(1);
  expect(tried).toContain("https://a0.muscache.test/studio.jpg");
  expect(native.admit.mock.calls.every((call) => call[2] === "picks")).toBe(true);
  // The photos are the picks' own: none stands as a card of its own.
  await expect
    .poll(
      () => screen.container.querySelectorAll(`[data-picture*="${digest(tried.at(-1)!)}"]`).length,
      {
        timeout: 10_000,
      },
    )
    .toBeGreaterThan(0);
  expect(screen.container.querySelectorAll("[data-card-id^='media-']")).toHaveLength(0);
  await screen.unmount();
  environment.dispose();
});
