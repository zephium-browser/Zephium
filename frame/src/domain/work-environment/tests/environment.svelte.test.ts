import { beforeEach, expect, test, vi } from "vitest";
import type { WorkCallV1, WorkEnvironmentSnapshot, WorkResponseV1 } from "$shared/ipc/bindings";
const native = vi.hoisted(() => ({
  call: vi.fn(),
  close: null as null | (() => Promise<boolean>),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ workCall: native.call });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: { workEnvironmentChanged: { listen: async () => () => {} } },
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
const id = "00000000000000000000000002";
const space = "00000000000000000000000003";
const element = "00000000000000000000000004";
function snapshot(revision = "1", viewRevision = "1"): WorkEnvironmentSnapshot {
  return {
    version: 1,
    id,
    profile,
    space,
    title: "A manual Work",
    lifecycle: "active",
    revision,
    elements: [],
    areas: [],
    view: { revision: viewRevision, x: 0, y: 0, zoom_milli: 1000, placements: [] },
  };
}
function response(reply: WorkResponseV1["reply"]): WorkResponseV1 {
  return { version: 1, profile, reply };
}
function page(selected: string | null = id) {
  return response({
    kind: "environment",
    reply: { kind: "page", works: [], next: null, selected },
  });
}
beforeEach(() => {
  vi.resetModules();
  vi.useRealTimers();
  native.call.mockReset();
  native.close = null;
  vi.stubGlobal("localStorage", memory());
});

function memory(): Storage {
  const held = new Map<string, string>();
  return {
    get length() {
      return held.size;
    },
    key: (index) => [...held.keys()][index] ?? null,
    getItem: (key) => held.get(key) ?? null,
    setItem: (key, value) => void held.set(key, value),
    removeItem: (key) => void held.delete(key),
    clear: () => held.clear(),
  };
}

test("reopens persisted selection without creating an objective or a second Work", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("No objective calls expected");
    if (call.request.kind === "list") return page();
    expect(call.request).toEqual({ kind: "open", id });
    return response({ kind: "environment", reply: { kind: "snapshot", snapshot: snapshot() } });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled Work");
  expect(session.snapshot?.id).toBe(id);
  expect(native.call).toHaveBeenCalledTimes(2);
  session.dispose();
});

test("creates an empty persistent environment only after a confirmed empty list", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("No objective calls expected");
    if (call.request.kind === "list") return page(null);
    if (call.request.kind !== "command") throw new Error("Expected creation");
    expect(call.request.intent).toEqual({ kind: "create", space, title: "My Work" });
    return response({
      kind: "environment",
      reply: {
        kind: "applied",
        command: call.request.command,
        applied_revision: "1",
        applied_view_revision: "1",
        replayed: false,
        snapshot: snapshot(),
      },
    });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("My Work");
  expect(session.snapshot?.elements).toEqual([]);
  expect(session.pending).toBeNull();
  session.dispose();
});

test("a launch onto a used work reuses the one empty draft instead of making another", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  const draft = "00000000000000000000000009";
  const summary = (work: string, empty: boolean) => ({
    id: work,
    space,
    title: empty ? "New work" : "Trip",
    lifecycle: "active" as const,
    revision: "1",
    name: null,
    requests: empty ? [] : ["Plan my trip"],
    touched_ms: empty ? "1" : "2",
    empty,
  });
  const used = {
    ...snapshot(),
    elements: [
      {
        id: element,
        area: null,
        reference: { kind: "objective" as const, objective: "00000000000000000000000005" },
      },
    ],
  };
  const opened: string[] = [];
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("No objective calls expected");
    if (call.request.kind === "list")
      return response({
        kind: "environment",
        reply: {
          kind: "page",
          works: [summary(id, false), summary(draft, true)],
          next: null,
          selected: id,
        },
      });
    if (call.request.kind !== "open") throw new Error("Nothing should be created");
    opened.push(call.request.id);
    const shown = call.request.id === id ? used : { ...snapshot(), id: draft, title: "New work" };
    return response({ kind: "environment", reply: { kind: "snapshot", snapshot: shown } });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("New work");
  expect(opened).toEqual([id, draft]);
  expect(session.snapshot?.id).toBe(draft);
  session.dispose();
});

test("keeps unknown mutation operands across hide/show and retries their exact command", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  let saved = snapshot("9007199254740993", "7");
  let lost = true;
  const mutations: WorkCallV1[] = [];
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Unexpected call");
    if (call.request.kind === "list") return page();
    if (call.request.kind !== "command")
      return response({ kind: "environment", reply: { kind: "snapshot", snapshot: saved } });
    mutations.push(call);
    if (lost) {
      lost = false;
      return Promise.reject(new Error("lost reply"));
    }
    saved = {
      ...saved,
      revision: "9007199254740994",
      elements: [{ id: element, area: null, reference: { kind: "browser", tab: "retained-tab" } }],
    };
    return response({
      kind: "environment",
      reply: {
        kind: "applied",
        command: call.request.command,
        applied_revision: saved.revision,
        applied_view_revision: "7",
        replayed: true,
        snapshot: saved,
      },
    });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled");
  expect(
    await session.edit({
      kind: "add",
      reference: { kind: "browser", tab: "retained-tab" },
      area: null,
    }),
  ).toBe(false);
  expect(session.delivery).toBe("unknown");
  const pending = session.pending;
  session.stopObserving();
  await session.start("Untitled");
  expect(mutations[0]).toEqual(mutations[1]);
  expect(pending?.kind === "command" ? pending.intent : null).toMatchObject({
    expected: "9007199254740993",
  });
  expect(session.snapshot?.elements[0]?.reference).toEqual({
    kind: "browser",
    tab: "retained-tab",
  });
  session.dispose();
});

test("checkpoints use view revision and retain local arrangement on conflict", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Unexpected call");
    if (call.request.kind === "list") return page();
    if (call.request.kind !== "checkpoint")
      return response({
        kind: "environment",
        reply: { kind: "snapshot", snapshot: snapshot("91", "7") },
      });
    expect(call.request).toMatchObject({
      kind: "checkpoint",
      id,
      expected: "7",
      view: { revision: "7" },
    });
    expect(call.request).not.toHaveProperty("command");
    return response({ kind: "error", error: "conflict" });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled");
  session.checkpoint({ ...snapshot().view, x: 42 });
  expect(await session.flushView()).toBe(false);
  expect(session.viewDraft?.view.x).toBe(42);
  expect(session.delivery).toBe("conflict");
  session.dispose();
});

test("rejects a cross-Space or wrong-command reply without claiming mutation completion", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Unexpected call");
    if (call.request.kind === "list") return page();
    if (call.request.kind !== "command")
      return response({ kind: "environment", reply: { kind: "snapshot", snapshot: snapshot() } });
    return response({
      kind: "environment",
      reply: {
        kind: "applied",
        command: "wrong",
        applied_revision: "2",
        applied_view_revision: "1",
        replayed: false,
        snapshot: { ...snapshot("2"), space: "another-space" },
      },
    });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled");
  expect(await session.edit({ kind: "rename", title: "Changed" })).toBe(false);
  expect(session.pending).not.toBeNull();
  expect(session.snapshot?.title).toBe("A manual Work");
  session.dispose();
});

test.each([false, true])(
  "unknown checkpoint retains exact identity and later gestures (remote advanced: %s)",
  async (advanced) => {
    const { WorkEnvironmentSession } = await import("../environment.svelte");
    const session = new WorkEnvironmentSession(profile, space);
    session.snapshot = snapshot("91", "7");
    const calls: WorkCallV1[] = [];
    native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
      calls.push(call);
      if (call.kind !== "environment" || call.request.kind !== "checkpoint")
        throw new Error("Only checkpoints expected");
      const request = call.request;
      if (calls.length === 1) throw new Error("lost receipt");
      const applied = String(BigInt(request.expected) + 1n);
      return response({
        kind: "environment",
        reply: {
          kind: "checkpointed",
          expected: request.expected,
          applied_view_revision: applied,
          replayed: calls.length === 2,
          snapshot: snapshot("91", advanced ? "10" : applied),
        },
      });
    });
    session.checkpoint({ ...snapshot().view, x: 42 });
    expect(await session.flushView()).toBe(false);
    const pending = session.pending;
    session.checkpoint({ ...snapshot().view, x: 84 });
    expect(await session.edit({ kind: "rename", title: "Blocked" })).toBe(false);
    expect(calls).toHaveLength(1);
    expect(session.pending).toBe(pending);
    expect(await session.retry()).toBe(true);
    expect(calls[0]).toEqual(calls[1]);
    expect(session.viewDraft?.view).toMatchObject({ revision: "8", x: 84 });
    if (advanced) {
      expect(session.delivery).toBe("conflict");
      expect(await session.flushView()).toBe(false);
      expect(await session.create("Blocked")).toBe(false);
      expect(calls).toHaveLength(2);
    } else {
      expect(await session.flushView()).toBe(true);
      expect(calls[2]).toMatchObject({
        request: { kind: "checkpoint", expected: "8", view: { revision: "8", x: 84 } },
      });
      expect(session.snapshot?.view.revision).toBe("9");
      expect(session.viewDraft).toBeNull();
    }
    session.dispose();
  },
);

test("mismatched checkpoint receipt retains unknown operands and draft", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  const session = new WorkEnvironmentSession(profile, space);
  session.snapshot = snapshot("91", "7");
  native.call.mockResolvedValue(
    response({
      kind: "environment",
      reply: {
        kind: "checkpointed",
        expected: "6",
        applied_view_revision: "8",
        replayed: true,
        snapshot: snapshot("91", "8"),
      },
    }),
  );
  session.checkpoint({ ...snapshot().view, x: 42 });
  expect(await session.flushView()).toBe(false);
  expect(session.delivery).toBe("unknown");
  expect(session.pending).toMatchObject({ kind: "checkpoint", expected: "7" });
  expect(session.viewDraft?.view.x).toBe(42);
  expect(session.snapshot.view.revision).toBe("7");
  session.dispose();
});

test("a checkpoint reply with a newer remote view cannot release a queued semantic mutation", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  const session = new WorkEnvironmentSession(profile, space);
  session.snapshot = snapshot("91", "7");
  let resolve!: (value: WorkResponseV1) => void;
  native.call.mockImplementation(
    () =>
      new Promise<WorkResponseV1>((done) => {
        resolve = done;
      }),
  );
  session.checkpoint({ ...snapshot().view, x: 42 });
  const editing = session.edit({ kind: "rename", title: "Must wait" });
  await vi.waitFor(() => expect(native.call).toHaveBeenCalledTimes(1));
  session.checkpoint({ ...snapshot().view, x: 84 });
  resolve(
    response({
      kind: "environment",
      reply: {
        kind: "checkpointed",
        expected: "7",
        applied_view_revision: "8",
        replayed: true,
        snapshot: snapshot("91", "10"),
      },
    }),
  );
  expect(await editing).toBe(false);
  expect(session.delivery).toBe("conflict");
  expect(session.viewDraft?.view.x).toBe(84);
  expect(native.call).toHaveBeenCalledTimes(1);
  session.dispose();
});

test("a refused arrangement save releases the draft and lets the saved arrangement stand", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Unexpected call");
    if (call.request.kind === "list") return page();
    if (call.request.kind === "checkpoint") return response({ kind: "error", error: "invalid" });
    return response({
      kind: "environment",
      reply: { kind: "snapshot", snapshot: snapshot("91", "7") },
    });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled");
  session.checkpoint({ ...snapshot().view, x: 42 });
  expect(await session.flushView()).toBe(true);
  expect(session.viewDraft).toBeNull();
  expect(session.pending).toBeNull();
  await vi.waitFor(() => expect(session.delivery).toBe("ready"));
  expect(native.call.mock.calls.at(-1)?.[1]).toMatchObject({ request: { kind: "read", id } });
  expect(await session.open(id)).toBe(true);
  session.dispose();
});

test("a busy refusal retries the arrangement save before releasing it", async () => {
  vi.useFakeTimers();
  try {
    const { WorkEnvironmentSession } = await import("../environment.svelte");
    let checkpoints = 0;
    native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
      if (call.kind !== "environment") throw new Error("Unexpected call");
      if (call.request.kind === "list") return page();
      if (call.request.kind !== "checkpoint")
        return response({
          kind: "environment",
          reply: { kind: "snapshot", snapshot: snapshot("91", "7") },
        });
      checkpoints += 1;
      if (checkpoints < 2) return response({ kind: "error", error: "capacity" });
      return response({
        kind: "environment",
        reply: {
          kind: "checkpointed",
          expected: "7",
          applied_view_revision: "8",
          replayed: false,
          snapshot: snapshot("91", "8"),
        },
      });
    });
    const session = new WorkEnvironmentSession(profile, space);
    await session.start("Untitled");
    session.checkpoint({ ...snapshot().view, x: 42 });
    expect(await session.flushView()).toBe(false);
    expect(session.viewDraft?.view.x).toBe(42);
    expect(session.failure).toBeNull();
    await vi.advanceTimersByTimeAsync(1500);
    expect(checkpoints).toBe(2);
    expect(session.viewDraft).toBeNull();
    expect(session.snapshot?.view.revision).toBe("8");
    session.dispose();
  } finally {
    vi.useRealTimers();
  }
});

test("words in the ask bar never hold up a close and are back after a relaunch", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Unexpected call");
    if (call.request.kind === "list") return page();
    return response({ kind: "environment", reply: { kind: "snapshot", snapshot: snapshot() } });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled");
  session.composer = "Compare the two plans";
  session.objectiveToAttach = "00000000000000000000000007";

  // A close does not wait for the pause in typing, and is not refused.
  expect(await native.close?.()).toBe(true);
  expect(localStorage.length).toBe(1);

  const relaunched = new WorkEnvironmentSession(profile, space);
  expect(relaunched.composer).toBe("Compare the two plans");
  expect(relaunched.objectiveToAttach).toBeNull();
  const elsewhere = new WorkEnvironmentSession(profile, "00000000000000000000000009");
  expect(elsewhere.composer).toBe("");
  session.dispose();
  relaunched.dispose();
  elsewhere.dispose();
});

test("typing is kept once it pauses, and sending or clearing lets the draft go at once", async () => {
  vi.useFakeTimers();
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  const session = new WorkEnvironmentSession(profile, space);
  session.composer = "Plan";
  session.composer = "Plan a trip";
  expect(localStorage.length).toBe(0);
  vi.advanceTimersByTime(600);
  expect(localStorage.length).toBe(1);
  session.composer = "";
  expect(localStorage.length).toBe(0);
  session.dispose();
});

test("a close still waits for a Work save that has not landed", async () => {
  const { WorkEnvironmentSession } = await import("../environment.svelte");
  native.call.mockImplementation((_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Unexpected call");
    if (call.request.kind === "list") return page();
    if (call.request.kind !== "checkpoint")
      return response({ kind: "environment", reply: { kind: "snapshot", snapshot: snapshot() } });
    return response({ kind: "error", error: "conflict" });
  });
  const session = new WorkEnvironmentSession(profile, space);
  await session.start("Untitled");
  session.composer = "Words that are kept";
  session.checkpoint({ ...snapshot().view, x: 42 });

  expect(await native.close?.()).toBe(false);
  session.dispose();
});

test("a deleted space takes the words in its ask bar with it", async () => {
  const { environmentSession } = await import("../environment.svelte");
  const { forgetDrafts, readDraft } = await import("$shared/lib/drafts");
  const live = environmentSession(profile, space)!;
  live.composer = "Words in a space about to go";

  forgetDrafts(profile, space);

  expect(live.composer).toBe("");
  expect(readDraft("work-composer", [profile, space])).toBeNull();
});
