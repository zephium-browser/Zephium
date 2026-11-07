import { beforeEach, expect, test, vi } from "vitest";
import type {
  WorkActivityResponseV1,
  WorkCallV1,
  WorkExecutionFact,
  WorkPageV1,
  WorkOperationV1,
  WorkOperationResponseV1,
  WorkResponseV1,
  WorkRuntimeProjection,
} from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({
  call: vi.fn(),
  activity: vi.fn(),
  begin: vi.fn(),
  status: vi.fn(),
  close: null as null | (() => Promise<boolean>),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    workCall: native.call,
    workActivity: native.activity,
    workOperation: native.begin,
    workOperationStatus: native.status,
  });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: { workChanged: { listen: async () => () => {} } },
}));
vi.mock("$shared/lib/close", () => ({
  registerCloseTask: (task: () => Promise<boolean>) => {
    native.close = task;
    return () => {
      native.close = null;
    };
  },
}));
const profile = "00000000000000000000000001";
const work = "00000000000000000000000002";
function projection(revision = "1", executions: WorkExecutionFact[] = []): WorkRuntimeProjection {
  return {
    version: 1,
    executions,
    interrupted: [],
    work: {
      schema_version: 2,
      id: work,
      profile,
      revision,
      lifecycle: "active",
      objective: "Investigate a dependency",
      objective_revision: "1",
      context_revision: "1",
      objective_author: "user",
      status: "draft",
      plan: null,
      questions: [],
    },
  };
}
function response(reply: WorkResponseV1["reply"]): WorkResponseV1 {
  return { version: 1, profile, reply };
}
function reads(
  call: WorkCallV1,
  revision = "1",
  executions: WorkExecutionFact[] = [],
): WorkResponseV1 {
  if (call.kind !== "query") throw new Error("Unexpected mutation");
  if (call.request.query.kind === "list") return response({ kind: "page", works: [], next: null });
  return response({ kind: "projection", projection: projection(revision, executions) });
}
beforeEach(() => {
  vi.resetModules();
  native.call.mockReset();
  native.activity.mockReset();
  native.begin.mockReset();
  native.status.mockReset();
  native.close = null;
});

test("an edit refused as a conflict stays a conflict across a reopen", async () => {
  const { WorkSession } = await import("../work.svelte");
  let revision = "1";
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call, revision));
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  revision = "9007199254740993";
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "author" || call.command.intent.kind !== "edit") return reads(call, revision);
    expect(call.command.intent.expected_revision).toBe("1");
    return response({ kind: "error", error: "conflict" });
  });
  expect(await session.edit({ kind: "set_objective", objective: "My investigation" })).toBe(false);
  expect(session.delivery).toBe("conflict");
  await session.open(work);
  expect(session.delivery).toBe("conflict");
  session.dispose();
});

test("retains exact operands after a wrong-profile acknowledgement and replays them once", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  const sent: WorkCallV1[] = [];
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "author") return reads(call, "2");
    sent.push(structuredClone(call));
    const reply = response({
      kind: "authoring_applied",
      receipt: {
        command: call.command.command,
        work,
        applied_revision: "2",
        deleted: false,
      },
    });
    return sent.length === 1 ? { ...reply, profile: "00000000000000000000000003" } : reply;
  });
  expect(await session.edit({ kind: "set_objective", objective: "Review the dependency" })).toBe(
    false,
  );
  expect(session.delivery).toBe("unknown");
  expect(session.pending).not.toBeNull();
  session.stopObserving();
  await session.start();
  expect(sent).toHaveLength(1);
  expect(await session.reconcile()).toBe(true);
  expect(sent[1]).toEqual(sent[0]);
  expect(session.pending).toBeNull();
  session.dispose();
});

test("rejects an acknowledgement for another Work without losing the pending command", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "author") return reads(call);
    return response({
      kind: "authoring_applied",
      receipt: {
        command: call.command.command,
        work: "00000000000000000000000003",
        applied_revision: "2",
        deleted: false,
      },
    });
  });
  expect(await session.edit({ kind: "archive" })).toBe(false);
  expect(session.delivery).toBe("unknown");
  expect(session.pending).not.toBeNull();
  session.dispose();
});

test("does not roll back a precision-safe projection when a stale read arrives", async () => {
  const { WorkSession } = await import("../work.svelte");
  let revision = "9007199254740993";
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call, revision));
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  revision = "9007199254740992";
  expect(await session.open(work)).toBe(false);
  expect(session.projection?.work.revision).toBe("9007199254740993");
  session.dispose();
});

test("a read of the revision already shown keeps the projection it has", async () => {
  const { WorkSession } = await import("../work.svelte");
  let revision = "4";
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call, revision));
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  const shown = session.projection;
  expect(await session.open(work)).toBe(true);
  expect(session.projection).toBe(shown);
  revision = "5";
  await session.open(work);
  expect(session.projection).not.toBe(shown);
  expect(session.projection?.work.revision).toBe("5");
  session.dispose();
});

test("a close waits only for a Work save that has not landed, never for an edit on its way", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  expect(await native.close?.()).toBe(true);
  session.pending = {
    kind: "author",
    command: {
      version: 1,
      command: "00000000000000000000000008",
      intent: { kind: "create", objective: "Unsaved objective" },
    },
  };
  native.call.mockImplementation(() => ({
    version: 1,
    profile,
    reply: { kind: "error", error: "outcome_unknown" },
  }));
  expect(await native.close?.()).toBe(false);
  session.dispose();
});

test("page frames survive a settled run, an overtaking read, and a return to Work", async () => {
  const { WorkSession } = await import("../work.svelte");
  const limits = {
    model_tokens: 100,
    cost_micro_usd: 0,
    operations: 1,
    timeout_seconds: 60,
    max_workers: 1,
  };
  const settled: WorkExecutionFact = {
    id: "execution",
    approved_revision: "1",
    status: "completed",
    attempts: [],
    spec: {
      plan_revision: "1",
      limits,
      nodes: [{ node: "node", parent: null, capability: { kind: "synthesize" }, limits }],
    },
    artifacts: [],
    user_artifacts: [],
  };
  const page: WorkPageV1 = {
    execution: "execution",
    attempt: "attempt",
    step: "read-1",
    url: "https://shop.example/p/1",
    live: false,
    frame: { generation: 2, width: 640, height: 400 },
  };
  const waiting: ((value: WorkActivityResponseV1) => void)[] = [];
  native.call.mockImplementation((_profile: string, call: WorkCallV1) =>
    reads(call, "1", [settled]),
  );
  native.activity.mockImplementation(
    () => new Promise<WorkActivityResponseV1>((resolve) => waiting.push(resolve)),
  );
  const session = new WorkSession(profile);
  await session.start();
  // A run that has already finished still owns pages, so every open reads them.
  await session.open(work);
  await session.open(work);
  expect(waiting).toHaveLength(2);
  // The second open overtook the first; the first answer is still about this
  // work, so its frames are kept rather than thrown away.
  waiting.shift()!({ version: 1, profile, work, signals: [], pages: [page], error: null });
  await expect.poll(() => session.pages.length).toBe(1);
  // Leaving Work unmounts the workspace; the frames outlive it.
  session.stopObserving();
  expect(session.pages).toEqual([page]);
  await session.start();
  expect(session.pages).toEqual([page]);
  // Another objective starts with a clean canvas.
  await session.open("00000000000000000000000009");
  expect(session.pages).toEqual([]);
  session.dispose();
});

test("a live run's activity is read each second, its projection only every tenth", async () => {
  vi.useFakeTimers();
  const { WorkSession } = await import("../work.svelte");
  const live = {
    id: "execution",
    approved_revision: "1",
    status: "running",
    attempts: [],
    artifacts: [],
    user_artifacts: [],
  } as unknown as WorkExecutionFact;
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call, "1", [live]));
  native.activity.mockResolvedValue({
    version: 1,
    profile,
    work,
    signals: [],
    pages: [],
    error: null,
  } satisfies WorkActivityResponseV1);
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  const projections = () =>
    native.call.mock.calls.filter((args) => {
      const call = args[1] as WorkCallV1;
      return call.kind === "query" && call.request.query.kind === "projection";
    }).length;
  const opened = projections();
  const activities = native.activity.mock.calls.length;
  await vi.advanceTimersByTimeAsync(9_500);
  expect(projections()).toBe(opened);
  expect(native.activity.mock.calls.length).toBe(activities + 9);
  await vi.advanceTimersByTimeAsync(1_000);
  expect(projections()).toBeGreaterThan(opened);
  session.dispose();
  vi.useRealTimers();
});

test("the next request first acknowledges a run an earlier launch left running", async () => {
  const { WorkSession } = await import("../work.svelte");
  const session = new WorkSession(profile);
  const left = { id: "00000000000000000000000009", status: "running" } as WorkExecutionFact;
  session.selected = work;
  session.projection = { ...projection("4", [left]), interrupted: [left.id] };
  const order: string[] = [];
  const execute = vi.spyOn(session, "execute").mockImplementation(async (intent) => {
    order.push(intent.kind);
    session.projection = { ...projection("5", [{ ...left, status: "interrupted" }]) };
    return true;
  });
  const begin = vi.spyOn(session.operations, "begin").mockImplementation(async (input) => {
    order.push(input.kind);
    return { id: "operation", input, state: { kind: "pending", work } };
  });
  await session.run();
  expect(execute).toHaveBeenCalledExactlyOnceWith({
    kind: "acknowledge_interruption",
    execution: left.id,
  });
  expect(order).toEqual(["acknowledge_interruption", "run"]);
  expect(begin.mock.calls[0]![0]).toMatchObject({ command: { expected_revision: "5" } });
  session.dispose();
});

const otherWork = "00000000000000000000000007";
function select(session: import("../work.svelte").WorkSession, id = work) {
  session.selected = id;
  session.projection = { ...projection(), work: { ...projection().work, id } };
}
function operationResponse(
  id: string,
  state: WorkOperationResponseV1["state"],
): WorkOperationResponseV1 {
  return { version: 1, profile, operation: id, state };
}

test("queued messages stay with their Work when another canvas is selected", async () => {
  const { WorkSession } = await import("../work.svelte");
  const session = new WorkSession(profile);
  select(session);
  const edit = vi.spyOn(session, "edit").mockResolvedValue(true);
  native.begin.mockImplementation((_profile: string, id: string, input: WorkOperationV1) => {
    expect(input.kind).toBe("run");
    return operationResponse(id, {
      kind: "pending",
      work: input.kind === "run" ? input.command.work : "wrong",
    });
  });
  expect(session.enqueue("Retry the first Work")).toBe(true);
  select(session, otherWork);
  expect(session.queue).toEqual([]);
  expect(session.enqueue("Continue the other Work")).toBe(true);
  expect(await session.sendQueued()).toBe(true);
  expect(edit).toHaveBeenCalledExactlyOnceWith({
    kind: "set_objective",
    objective: "Continue the other Work",
  });
  expect(native.begin.mock.calls[0]![2].command.work).toBe(otherWork);
  expect(session.queue).toEqual([]);
  select(session);
  expect(session.queue).toEqual(["Retry the first Work"]);
  session.back();
  expect(session.canRelease).toBe(false);
  session.dispose();
});

test("an objective conflict retains the queued head and prevents automatic resend", async () => {
  const { WorkSession } = await import("../work.svelte");
  const session = new WorkSession(profile);
  select(session);
  const edit = vi.spyOn(session, "edit").mockImplementation(async () => {
    session.failure = "conflict";
    session.delivery = "conflict";
    return false;
  });
  session.enqueue("Retry the closed sign-in page");
  expect(await session.sendQueued()).toBe(false);
  expect(session.queue).toEqual(["Retry the closed sign-in page"]);
  expect(await session.sendQueued()).toBe(false);
  expect(edit).toHaveBeenCalledTimes(1);
  expect(native.begin).not.toHaveBeenCalled();
  expect(session.failure).toBe("conflict");
  session.dispose();
});

test("a refused run is retained and the same retry click recovers without duplicating it", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  native.status.mockImplementation((_profile: string, _work: string, id: string) =>
    operationResponse(id, { kind: "refused", error: "unavailable" }),
  );
  let allow = false;
  native.begin.mockImplementation((_profile: string, id: string) =>
    operationResponse(
      id,
      allow ? { kind: "pending", work } : { kind: "refused", error: "unavailable" },
    ),
  );
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  vi.spyOn(session, "edit").mockResolvedValue(true);
  session.enqueue("Retry the closed page");
  expect(await session.sendQueued()).toBe(false);
  expect(session.delivery).toBe("rejected");
  expect(session.failure).toBe("unavailable");
  expect(session.queue).toEqual(["Retry the closed page"]);
  expect(await session.sendQueued()).toBe(false);
  expect(native.begin).toHaveBeenCalledTimes(1);
  allow = true;
  expect(session.enqueue("Retry the closed page")).toBe(true);
  await expect.poll(() => session.delivery).toBe("ready");
  expect(session.queue).toEqual(["Retry the closed page"]);
  expect(await session.sendQueued()).toBe(true);
  expect(native.begin).toHaveBeenCalledTimes(2);
  expect(session.queue).toEqual([]);
  session.dispose();
});

test("an unknown queued admission reconciles the original operation without dispatching again", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  native.begin.mockRejectedValue(new Error("lost admission response"));
  let known = false;
  native.status.mockImplementation((_profile: string, _work: string, id: string) =>
    operationResponse(id, known ? { kind: "pending", work } : { kind: "unknown" }),
  );
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  vi.spyOn(session, "edit").mockResolvedValue(true);
  session.enqueue("Retry the closed check");
  expect(await session.sendQueued()).toBe(false);
  expect(session.delivery).toBe("unknown");
  const original = session.operations.latest(work, "run")!.id;
  expect(await session.sendQueued()).toBe(false);
  expect(session.queue).toEqual(["Retry the closed check"]);
  known = true;
  expect(session.enqueue("Retry the closed check")).toBe(true);
  await expect.poll(() => session.queue).toEqual([]);
  expect(session.operations.latest(work, "run")!.id).toBe(original);
  expect(native.begin).toHaveBeenCalledTimes(1);
  expect(native.status.mock.calls.every((call) => call[2] === original)).toBe(true);
  session.dispose();
});

test("switching Work while the objective edit settles never starts the newly selected Work", async () => {
  const { WorkSession } = await import("../work.svelte");
  const session = new WorkSession(profile);
  select(session);
  let edited!: (value: boolean) => void;
  vi.spyOn(session, "edit").mockImplementation(
    () =>
      new Promise((resolve) => {
        edited = resolve;
      }),
  );
  session.enqueue("Retry A");
  const pending = session.sendQueued();
  select(session, otherWork);
  edited(true);
  expect(await pending).toBe(false);
  expect(native.begin).not.toHaveBeenCalled();
  expect(session.queue).toEqual([]);
  select(session);
  expect(session.queue).toEqual(["Retry A"]);
  session.dispose();
});

test("leaving and returning to the same Work still invalidates an in-flight continuation", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  const session = new WorkSession(profile);
  select(session);
  let edited!: (value: boolean) => void;
  vi.spyOn(session, "edit").mockImplementation(
    () =>
      new Promise((resolve) => {
        edited = resolve;
      }),
  );
  const pending = session.continueWith("Retry A");
  await session.open(otherWork);
  await session.open(work);
  select(session);
  edited(true);
  expect(await pending).toBe(false);
  expect(native.begin).not.toHaveBeenCalled();
  session.dispose();
});

test("queue capacity refusal is observable and keeps all already admitted messages", async () => {
  const { WorkSession } = await import("../work.svelte");
  const session = new WorkSession(profile);
  select(session);
  for (let index = 0; index < 16; index++) expect(session.enqueue(`Request ${index}`)).toBe(true);
  expect(session.enqueue("Overflow")).toBe(false);
  expect(session.failure).toBe("capacity");
  expect(session.delivery).toBe("rejected");
  expect(session.queue).toHaveLength(16);
  select(session, otherWork);
  expect(session.enqueue("Other Work overflow")).toBe(false);
  expect(session.queue).toEqual([]);
  session.dispose();
});

test("queue recovery cannot leave a newly selected Work resynchronizing", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind === "query" && call.request.query.kind === "projection") {
      const id = call.request.query.work;
      return response({
        kind: "projection",
        projection: { ...projection(), work: { ...projection().work, id } },
      });
    }
    return reads(call);
  });
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  session.enqueue("Retry A");
  let reconciled!: () => void;
  vi.spyOn(session.operations, "reconcile").mockImplementation(
    () =>
      new Promise((resolve) => {
        reconciled = resolve;
      }),
  );
  const recovering = session.reconcile();
  await session.open(otherWork);
  reconciled();
  expect(await recovering).toBe(false);
  expect(session.selected).toBe(otherWork);
  expect(session.delivery).toBe("ready");
  expect(session.queue).toEqual([]);
  await session.open(work);
  expect(session.queue).toEqual(["Retry A"]);
  session.dispose();
});

test("an outcome-unknown settled response never becomes permission to redispatch the queued request", async () => {
  const { WorkSession } = await import("../work.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
  native.status.mockImplementation((_profile: string, _work: string, id: string) =>
    operationResponse(id, {
      kind: "settled",
      response: response({ kind: "error", error: "outcome_unknown" }),
    }),
  );
  native.begin.mockImplementation((_profile: string, id: string) =>
    operationResponse(id, {
      kind: "settled",
      response: response({ kind: "error", error: "outcome_unknown" }),
    }),
  );
  const session = new WorkSession(profile);
  await session.start();
  await session.open(work);
  vi.spyOn(session, "edit").mockResolvedValue(true);
  session.enqueue("Retry the closed page");
  expect(await session.sendQueued()).toBe(false);
  expect(session.enqueue("Retry the closed page")).toBe(true);
  await expect.poll(() => session.delivery).toBe("unknown");
  expect(await session.sendQueued()).toBe(false);
  expect(session.queue).toEqual(["Retry the closed page"]);
  expect(native.begin).toHaveBeenCalledTimes(1);
  session.dispose();
});

test.each(["refused", "unknown"] as const)(
  "evicting a %s queued job preserves its actual retry authority",
  async (initial) => {
    const { WorkSession } = await import("../work.svelte");
    native.call.mockImplementation((_profile: string, call: WorkCallV1) => reads(call));
    const first =
      initial === "unknown"
        ? { kind: "unknown" as const }
        : { kind: "refused" as const, error: "unavailable" as const };
    let allow = false;
    native.begin.mockImplementation((_profile: string, id: string) =>
      operationResponse(id, allow ? { kind: "pending", work } : first),
    );
    native.status.mockImplementation((_profile: string, _work: string, id: string) =>
      operationResponse(id, allow ? { kind: "pending", work } : first),
    );
    const session = new WorkSession(profile);
    await session.start();
    await session.open(work);
    vi.spyOn(session, "edit").mockResolvedValue(true);
    session.enqueue("Retry the retained request");
    expect(await session.sendQueued()).toBe(false);
    const original = session.operations.latest(work, "run")!.id;
    session.operations.jobs.delete(original);
    allow = true;
    const reconcile = vi.spyOn(session, "reconcile");
    expect(session.enqueue("Retry the retained request")).toBe(true);
    await reconcile.mock.results[0]!.value;
    expect(session.queue).toEqual(["Retry the retained request"]);
    expect(session.delivery).toBe(initial === "unknown" ? "unknown" : "ready");
    expect(await session.sendQueued()).toBe(initial === "refused");
    expect(native.begin).toHaveBeenCalledTimes(initial === "unknown" ? 1 : 2);
    expect(session.queue).toEqual(initial === "unknown" ? ["Retry the retained request"] : []);
    session.dispose();
  },
);
