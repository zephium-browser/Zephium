import { expect, test, vi } from "vitest";
import type { WorkEnvironmentSnapshot, WorkResponseV1, WorkCallV1 } from "$shared/ipc/bindings";
const native = vi.hoisted(() => ({
  call: vi.fn(),
  resources: vi.fn(),
  notes: vi.fn(),
  activity: vi.fn(),
  changed: null as null | ((event: { payload: { profile: string; work: string } }) => void),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    workCall: native.call,
    resourceCall: native.resources,
    noteCall: native.notes,
    workActivity: native.activity,
  });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    workChanged: {
      listen: async (handler: NonNullable<typeof native.changed>) => {
        native.changed = handler;
        return () => {};
      },
    },
    resourceChanged: { listen: async () => () => {} },
    notesChanged: { listen: async () => () => {} },
  },
}));
const profile = "00000000000000000000000001";
const limits = {
  model_tokens: 100,
  cost_micro_usd: 0,
  operations: 1,
  timeout_seconds: 60,
  max_workers: 1,
};
const snapshot: WorkEnvironmentSnapshot = {
  version: 1,
  profile,
  id: "environment",
  space: "space",
  title: "Work",
  revision: "1",
  lifecycle: "active",
  areas: [],
  elements: [],
  view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
};

test("slow page frames cannot delay Stop or undo same-revision interruption", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  const projection = {
    version: 1,
    interrupted: [] as string[],
    executions: [
      {
        id: "execution",
        approved_revision: "1",
        status: "running",
        attempts: [],
        spec: { plan_revision: "1", limits, nodes: [] },
        artifacts: [],
        user_artifacts: [],
      },
    ],
    work: {
      schema_version: 2,
      profile,
      id: "objective",
      revision: "1",
      lifecycle: "active",
      objective: "Study plan",
      objective_revision: "1",
      context_revision: "1",
      objective_author: "user",
      questions: [],
      status: "plan_ready",
      plan: null,
    },
  };
  native.call.mockReset();
  native.activity.mockReset();
  native.call.mockImplementation(async () => ({
    version: 1,
    profile,
    reply: { kind: "projection", projection: structuredClone(projection) },
  }));
  let release!: () => void;
  native.activity.mockImplementation(
    () =>
      new Promise((resolve) => {
        release = () =>
          resolve({ version: 1, profile, work: "objective", signals: [], pages: [], error: null });
      }),
  );
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: [
      { id: "element", area: null, reference: { kind: "objective", objective: "objective" } },
    ],
  });
  await context.start();
  await vi.waitFor(() => expect(context.objectives.get("objective")?.interrupted).toEqual([]));
  expect(native.activity).toHaveBeenCalledTimes(1);
  projection.interrupted = ["execution"];
  native.changed!({ payload: { profile, work: "objective" } });
  await vi.waitFor(() =>
    expect(context.objectives.get("objective")?.interrupted).toEqual(["execution"]),
  );
  expect(native.activity).toHaveBeenCalledTimes(1);
  release();
  await vi.waitFor(() => expect(native.activity).toHaveBeenCalledTimes(2));
  expect(context.objectives.get("objective")?.interrupted).toEqual(["execution"]);
  context.dispose();
});

test("projection refreshes for one work coalesce without overlapping reads", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  native.call.mockReset();
  let release!: (response: WorkResponseV1) => void;
  native.call.mockImplementation(
    () =>
      new Promise<WorkResponseV1>((resolve) => {
        release = resolve;
      }),
  );
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: [
      { id: "element", area: null, reference: { kind: "objective", objective: "objective" } },
    ],
  });
  await context.start();
  await vi.waitFor(() => expect(native.call).toHaveBeenCalledTimes(1));
  for (let i = 0; i < 5; i++) native.changed!({ payload: { profile, work: "objective" } });
  expect(native.call).toHaveBeenCalledTimes(1);
  release({ version: 1, profile, reply: { kind: "error", error: "not_found" } });
  await vi.waitFor(() => expect(native.call).toHaveBeenCalledTimes(2));
  context.dispose();
});

test("attached context reads are bounded to two, use queries only, and discard late results after hide", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  const resolvers: ((value: WorkResponseV1) => void)[] = [];
  native.call.mockReset();
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    expect(call.kind).toBe("query");
    return new Promise<WorkResponseV1>((resolve) => resolvers.push(resolve));
  });
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: Array.from({ length: 40 }, (_, index) => ({
      id: `element-${index}`,
      area: null,
      reference: { kind: "objective" as const, objective: `objective-${index}` },
    })),
  });
  await context.start();
  await vi.waitFor(() => expect(native.call).toHaveBeenCalledTimes(2));
  resolvers[0]!({ version: 1, profile, reply: { kind: "error", error: "not_found" } });
  await vi.waitFor(() => expect(native.call).toHaveBeenCalledTimes(3));
  context.dispose();
  resolvers[1]!({ version: 1, profile, reply: { kind: "error", error: "not_found" } });
  await Promise.resolve();
  expect(context.objectives.size).toBe(0);
  expect(native.call).toHaveBeenCalledTimes(3);
});

test("note labels resolve only attached IDs without starting an objective", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  native.call.mockReset();
  native.resources.mockReset();
  native.notes.mockReset();
  native.notes.mockResolvedValue({
    profile,
    response: {
      kind: "record",
      record: {
        summary: {
          id: "note",
          revision: "1",
          title: "Saved findings",
          preview: "",
          pinned: false,
          trashed: false,
          editable: true,
          created_at: "1",
          modified_at: "1",
          path: "Saved findings.md",
        },
        markdown: "# Saved findings\n",
      },
    },
  });
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: [{ id: "element", area: null, reference: { kind: "resource", resource: "note" } }],
  });
  await context.start();
  await vi.waitFor(() => expect(context.notes[0]?.title).toBe("Saved findings"));
  expect(native.notes).toHaveBeenCalledExactlyOnceWith(profile, { kind: "get", id: "note" });
  expect(native.resources).not.toHaveBeenCalled();
  expect(native.call).not.toHaveBeenCalled();
  context.dispose();
});

test("an attached URL's revision is the SHA-256 Rust recomputes, and leaves with the URL", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  const url = "https://outlook.live.com/mail/0/inbox/id/AQQkADAwATM0MDAAMS1hYmQ5";
  const context = new WorkEnvironmentContext(profile);
  context.trackUrls([url]);
  await vi.waitFor(() =>
    expect(context.urlRevisions.get(url)).toBe(
      "0694dabf6871fb107ac842abba5f084b2a54d906ca6e854fcb80af0aa5702ea0",
    ),
  );
  context.trackUrls([]);
  expect(context.urlRevisions.size).toBe(0);
  context.dispose();
});

test("an attached work's page frames arrive with its projection, not with the session", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  const projection = {
    version: 1,
    interrupted: [],
    executions: [
      {
        id: "execution",
        approved_revision: "1",
        status: "completed",
        attempts: [],
        spec: { plan_revision: "1", limits, nodes: [] },
        artifacts: [],
        user_artifacts: [],
      },
    ],
    work: {
      schema_version: 2,
      profile,
      id: "objective",
      revision: "1",
      lifecycle: "active",
      objective: "Compare quiet keyboards",
      objective_revision: "1",
      context_revision: "1",
      objective_author: "user",
      questions: [],
      status: "plan_ready",
      plan: null,
    },
  };
  native.call.mockReset();
  native.call.mockResolvedValue({ version: 1, profile, reply: { kind: "projection", projection } });
  native.activity.mockReset();
  native.activity.mockResolvedValue({
    version: 1,
    profile,
    work: "objective",
    signals: [],
    pages: [
      {
        execution: "execution",
        attempt: "attempt",
        step: "read",
        url: "https://a.example/one",
        live: false,
        frame: { generation: 1, width: 640, height: 400 },
      },
    ],
    error: null,
  });
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: [
      { id: "element", area: null, reference: { kind: "objective", objective: "objective" } },
    ],
  });
  await context.start();
  await vi.waitFor(() => expect(context.pages.get("objective")).toHaveLength(1));
  expect(native.activity).toHaveBeenCalledExactlyOnceWith(profile, "objective");
  context.dispose();
  expect(context.pages.size).toBe(0);
});

test("an opening's reads are published together, and a lone read at once", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  const projection = (id: string) => ({
    version: 1,
    interrupted: [],
    executions: [],
    work: {
      schema_version: 2,
      profile,
      id,
      revision: "1",
      lifecycle: "active",
      objective: id,
      objective_revision: "1",
      context_revision: "1",
      objective_author: "user",
      questions: [],
      status: "plan_ready",
      plan: null,
    },
  });
  const resolvers: (() => void)[] = [];
  native.call.mockReset();
  native.call.mockImplementation(
    (_profile: string, call: { request: { query: { work: string } } }) =>
      new Promise((resolve) =>
        resolvers.push(() =>
          resolve({
            version: 1,
            profile,
            reply: { kind: "projection", projection: projection(call.request.query.work) },
          }),
        ),
      ),
  );
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: ["a", "b", "c"].map((id) => ({
      id: `element-${id}`,
      area: null,
      reference: { kind: "objective" as const, objective: id },
    })),
  });
  await context.start();
  await vi.waitFor(() => expect(resolvers).toHaveLength(2));
  resolvers[0]!();
  await vi.waitFor(() => expect(resolvers).toHaveLength(3));
  expect(context.objectives.size).toBe(0);
  resolvers[1]!();
  resolvers[2]!();
  await vi.waitFor(() => expect(context.objectives.size).toBe(3));
  context.update({
    ...snapshot,
    elements: [{ id: "element-d", area: null, reference: { kind: "objective", objective: "d" } }],
  });
  await vi.waitFor(() => expect(resolvers).toHaveLength(4));
  resolvers[3]!();
  await vi.waitFor(() => expect(context.objectives.has("d")).toBe(true));
  context.dispose();
});

test("a work read again at the revision it shows keeps its projection, and a new frame only its pages", async () => {
  const { WorkEnvironmentContext } = await import("../context.svelte");
  const projection = {
    version: 1,
    interrupted: [],
    executions: [
      {
        id: "execution",
        approved_revision: "1",
        status: "running",
        attempts: [],
        spec: { plan_revision: "1", limits, nodes: [] },
        artifacts: [],
        user_artifacts: [],
      },
    ],
    work: {
      schema_version: 2,
      profile,
      id: "objective",
      revision: "3",
      lifecycle: "active",
      objective: "Compare quiet keyboards",
      objective_revision: "1",
      context_revision: "1",
      objective_author: "user",
      questions: [],
      status: "plan_ready",
      plan: null,
    },
  };
  const page = (generation: number) => ({
    execution: "execution",
    attempt: "attempt",
    step: "read",
    url: "https://a.example/one",
    live: true,
    frame: { generation, width: 640, height: 400 },
  });
  let generation = 1;
  native.call.mockReset();
  native.call.mockImplementation(async () => ({
    version: 1,
    profile,
    reply: { kind: "projection", projection: structuredClone(projection) },
  }));
  native.activity.mockReset();
  native.activity.mockImplementation(async () => ({
    version: 1,
    profile,
    work: "objective",
    signals: [],
    pages: [page(generation)],
    error: null,
  }));
  const context = new WorkEnvironmentContext(profile);
  context.update({
    ...snapshot,
    elements: [
      { id: "element", area: null, reference: { kind: "objective", objective: "objective" } },
    ],
  });
  await context.start();
  await vi.waitFor(() => expect(context.pages.get("objective")).toHaveLength(1));
  const shown = context.objectives.get("objective");
  const pages = context.pages.get("objective");
  native.changed!({ payload: { profile, work: "objective" } });
  await vi.waitFor(() => expect(native.activity).toHaveBeenCalledTimes(2));
  await new Promise((done) => setTimeout(done, 300));
  expect(context.objectives.get("objective")).toBe(shown);
  expect(context.pages.get("objective")).toBe(pages);
  generation = 2;
  native.changed!({ payload: { profile, work: "objective" } });
  await vi.waitFor(() => expect(context.pages.get("objective")?.[0]?.frame?.generation).toBe(2));
  expect(context.objectives.get("objective")).toBe(shown);
  context.dispose();
});
