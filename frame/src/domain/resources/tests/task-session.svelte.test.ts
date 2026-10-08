import { expect, test, vi } from "vitest";
import type { ResourceSummary } from "$shared/ipc/bindings";

const host = vi.hoisted(() => ({
  call: vi.fn(),
  changed: null as null | ((event: { payload: unknown }) => void),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ resourceCall: host.call });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    resourceChanged: {
      listen: async (listener: typeof host.changed) => {
        host.changed = listener;
        return () => {};
      },
    },
  },
}));

const profile = "00000000000000000000000001";

function row(id: string): ResourceSummary {
  return {
    id,
    revision: "1",
    title: "Kept",
    pinned: false,
    updated_at: "1",
    completed: false,
    due_date: null,
    due_time: null,
    status: "open",
    assignee: "user",
    origin: "user",
    context: null,
    sort_key: null,
    work: null,
  };
}

function serve() {
  host.call.mockImplementation(async () => ({
    profile,
    response: {
      kind: "task_page",
      metadata: [],
      lists: [],
      items: [row("00000000000000000000000009")],
      next: null,
      counts: { inbox: 0, today: 0, overdue: 0, upcoming: 0, all: 1, completed: 0, trash: 0 },
    },
  }));
}

const settled = () => new Promise((resolve) => setTimeout(resolve, 20));

/** A component cleanup cannot await, so stopping must not leave work that
 *  lands after the next start and tears down what it just built. */
test("a session restarted straight after being stopped still loads", async () => {
  const { taskSession } = await import("../tasks.svelte");
  serve();
  const session = taskSession(profile, "restart");
  await session.start();
  expect(session.items.length).toBe(1);

  // Exactly what a remount does: fire-and-forget cleanup, immediate restart.
  session.stop();
  await session.start();
  await settled();

  expect(session.items.length).toBe(1);
  expect(session.error).toBeNull();
});

test("a stopped session keeps its rows to draw while they are read again", async () => {
  const { taskSession } = await import("../tasks.svelte");
  serve();
  const session = taskSession(profile, "released");
  await session.start();
  expect(session.items.length).toBe(1);

  session.stop();
  await settled();
  expect(session.items.length).toBe(1);
  expect(session.error).toBeNull();
  let answer!: () => void;
  const answered = new Promise<void>((resolve) => (answer = resolve));
  const forward = host.call.getMockImplementation()!;
  host.call.mockImplementation(async (owner, call) => {
    await answered;
    return forward(owner, call);
  });
  const restarted = session.start();
  await settled();
  // Shown again, the host has rows while the listing is read, not "Loading…".
  expect(session.loading).toBe(true);
  expect(session.rows.length).toBe(1);
  answer();
  await restarted;
  session.stop();
});

test("the least recently used clean session is let go, never one with unsaved work", async () => {
  const { taskSession } = await import("../tasks.svelte");
  serve();
  const keep = taskSession(profile, "lru-unsaved");
  keep.captureDraft = "half a thought";
  const first = taskSession(profile, "lru-0");
  const others = Array.from({ length: 8 }, (_, index) => taskSession(profile, `lru-${index + 1}`));
  expect(taskSession(profile, "lru-unsaved")).toBe(keep);
  expect(taskSession(profile, "lru-0")).not.toBe(first);
  expect(taskSession(profile, `lru-8`)).toBe(others.at(-1));
  keep.captureDraft = "";
});

async function editingSession(name: string) {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const { TaskSession } = await import("../tasks.svelte");
  const server = resourceTestServer(profile);
  host.call.mockImplementation(server.call);
  const session = new TaskSession(profile);
  await session.start();
  const id = (await session.create({ title: name }))!;
  return { server, session, id };
}

test("undoing a date change consumes history instead of recording its inverse", async () => {
  const { session, id } = await editingSession("Schedule");
  await session.schedule(id, "2026-09-25");
  await session.undo();
  expect(session.rows.find((row) => row.id === id)?.dueDate).toBeNull();
  // The next undo reverses creation, not the undo of scheduling.
  await session.undo();
  expect(session.rows).toHaveLength(0);
  expect(session.undoable).toBe(false);
  session.stop();
});

test("a lost create reply is replayed as the same request without being asked", async () => {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const { TaskSession } = await import("../tasks.svelte");
  const server = resourceTestServer(profile);
  const requests: string[] = [];
  host.call.mockImplementation(async (owner, call) => {
    const result = await server.call(owner, call);
    if (call.kind !== "mutate") return result;
    requests.push(call.command.request_id);
    return requests.length === 1
      ? { profile, response: { kind: "error", error: "outcome_unknown" } }
      : result;
  });
  const session = new TaskSession(profile);
  await session.start();
  vi.useFakeTimers();
  const created = session.create({ title: "Keep this thought" });
  await vi.advanceTimersByTimeAsync(1000);
  vi.useRealTimers();
  expect(await created).toBeTruthy();
  expect(server.records.size).toBe(1);
  expect(new Set(requests).size).toBe(1);
  expect(session.failure).toBeNull();
  session.stop();
});

test("a write native never answers is kept, and retried as one request", async () => {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const { TaskSession } = await import("../tasks.svelte");
  const server = resourceTestServer(profile);
  let answering = false;
  host.call.mockImplementation(async (owner, call) => {
    const result = await server.call(owner, call);
    return call.kind === "mutate" && !answering
      ? { profile, response: { kind: "error", error: "outcome_unknown" } }
      : result;
  });
  const session = new TaskSession(profile);
  await session.start();
  session.captureDraft = "Keep this thought";
  vi.useFakeTimers();
  const created = session.create({ title: session.captureDraft });
  await vi.advanceTimersByTimeAsync(10_000);
  vi.useRealTimers();
  expect(await created).toBeNull();
  expect(session.captureDraft).toBe("Keep this thought");
  expect(session.failure).toBe("outcome_unknown");
  expect(await session.flush()).toBe(false);
  answering = true;
  await session.retry();
  expect(server.records.size).toBe(1);
  expect(session.captureDraft).toBe("");
  expect(session.failure).toBeNull();
  session.stop();
});

test("a busy native takes the same edit a moment later", async () => {
  const { server, session, id } = await editingSession("Busy");
  let busy = true;
  host.call.mockImplementation(async (owner, call) => {
    if (call.kind === "mutate" && busy) {
      busy = false;
      return { profile, response: { kind: "error", error: "capacity" } };
    }
    return server.call(owner, call);
  });
  session.rename(id, "Busy day");
  vi.useFakeTimers();
  const saved = session.flush();
  await vi.advanceTimersByTimeAsync(1000);
  vi.useRealTimers();
  expect(await saved).toBe(true);
  expect(server.records.get(id)?.draft.title).toBe("Busy day");
  expect(session.failure).toBeNull();
  session.stop();
});

test("typing into a task asks for no new totals", async () => {
  const { server, session, id } = await editingSession("Totals");
  // Creating it moved the totals; that refresh is not this test's.
  await new Promise((resolve) => setTimeout(resolve, 150));
  const calls: string[] = [];
  host.call.mockImplementation(async (owner, call) => {
    calls.push(call.kind);
    return server.call(owner, call);
  });
  session.rename(id, "Totals stay");
  await session.flush();
  await session.setPinned(id, true);
  await new Promise((resolve) => setTimeout(resolve, 150));
  expect(calls).toContain("mutate");
  expect(calls).not.toContain("task_overview");
  session.stop();
});

test("an earlier title save cannot replace newer typing", async () => {
  const { server, session, id } = await editingSession("Original");
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  host.call.mockImplementation(async (owner, call) => {
    if (call.kind === "mutate" && call.command.intent.kind === "update_task") await gate;
    return server.call(owner, call);
  });
  session.rename(id, "First edit");
  const first = session.flush();
  await settled();
  session.rename(id, "Second edit");
  release();
  await first;
  expect(session.rows.find((row) => row.id === id)?.title).toBe("Second edit");
  await session.flush();
  expect(server.records.get(id)?.draft.title).toBe("Second edit");
  session.stop();
});

test("failed edits remain visible across reload and host teardown", async () => {
  const { server, session, id } = await editingSession("Original");
  host.call.mockImplementation(async (owner, call) =>
    call.kind === "mutate"
      ? { profile, response: { kind: "error", error: "unavailable" } }
      : server.call(owner, call),
  );
  session.rename(id, "Retained edit");
  expect(await session.flush()).toBe(false);
  await session.reload();
  expect(session.rows.find((row) => row.id === id)?.title).toBe("Retained edit");
  expect(session.failure).toBe("unavailable");
  session.stop();
  await settled();
  await session.start();
  expect(session.rows.find((row) => row.id === id)?.title).toBe("Retained edit");
  host.call.mockImplementation(server.call);
  await session.retry();
  expect(server.records.get(id)?.draft.title).toBe("Retained edit");
  expect(session.failure).toBeNull();
  session.stop();
});

test("external description revisions refresh a selected task", async () => {
  const { server, session, id } = await editingSession("Shared task");
  await session.load(id);
  const record = structuredClone(server.records.get(id)!);
  record.revision = "2";
  if (record.draft.content.kind === "task") record.draft.content.description = "Edited elsewhere";
  server.records.set(id, record);
  await session.reload();
  await settled();
  expect(session.rows.find((row) => row.id === id)?.description).toBe("Edited elsewhere");
  session.stop();
});

test("same-field conflict preserves local text until explicit retry", async () => {
  const { server, session, id } = await editingSession("Original");
  session.rename(id, "My edit");
  const record = structuredClone(server.records.get(id)!);
  record.revision = "2";
  record.draft.title = "Their edit";
  server.records.set(id, record);
  expect(await session.flush()).toBe(false);
  expect(server.records.get(id)?.draft.title).toBe("Their edit");
  expect(session.rows.find((row) => row.id === id)?.title).toBe("My edit");
  expect(session.failure).toBe("conflict");
  await session.retry();
  expect(server.records.get(id)?.draft.title).toBe("My edit");
  session.stop();
});

test("an edit to one field leaves another actor's change to a different field alone", async () => {
  const { server, session, id } = await editingSession("Original");
  const record = structuredClone(server.records.get(id)!);
  record.revision = "2";
  if (record.draft.content.kind === "task") record.draft.content.details.priority = "high";
  server.records.set(id, record);
  session.rename(id, "Renamed here");
  expect(await session.flush()).toBe(true);
  const saved = server.records.get(id)!;
  expect(saved.draft.title).toBe("Renamed here");
  expect(saved.draft.content.kind === "task" && saved.draft.content.details.priority).toBe("high");
  expect(session.failure).toBeNull();
  session.stop();
});

test("a settled write refreshes totals without reloading the list", async () => {
  const { server, session, id } = await editingSession("Count me");
  await settled();
  const calls: string[] = [];
  host.call.mockImplementation(async (owner, call) => {
    calls.push(call.kind === "mutate" ? call.command.intent.kind : call.kind);
    return server.call(owner, call);
  });
  await session.setStatus(id, "done");
  // Totals follow on the refresh debounce.
  await new Promise((resolve) => setTimeout(resolve, 150));
  expect(calls).not.toContain("list_tasks");
  expect(calls).not.toContain("get");
  expect(calls).toContain("update_task");
  expect(calls).toContain("task_overview");
  expect(session.counts.completed).toBe(1);
  session.stop();
});

test("a deadline and an estimate are written and undone like any other field", async () => {
  const { server, session, id } = await editingSession("Estimate");
  await session.setDeadline(id, "2026-10-02");
  await session.setDuration(id, 90);
  const details = () => {
    const content = server.records.get(id)!.draft.content;
    return content.kind === "task" ? [content.details.deadline, content.details.duration] : [];
  };
  expect(details()).toEqual(["2026-10-02", 90]);
  expect(session.rows.find((row) => row.id === id)).toMatchObject({
    deadline: "2026-10-02",
    duration: 90,
  });
  await session.undo();
  await session.undo();
  expect(details()).toEqual([null, null]);
  session.stop();
});

function recordMutations() {
  const sent: string[] = [];
  const forward = host.call.getMockImplementation()!;
  host.call.mockImplementation(async (owner, call) => {
    if (call.kind === "mutate") sent.push(call.command.intent.kind);
    return forward(owner, call);
  });
  return sent;
}

test("a title cleared to retype it neither fails nor conflicts", async () => {
  const { server, session, id } = await editingSession("Call mom");
  const sent = recordMutations();
  vi.useFakeTimers();
  session.rename(id, "");
  await vi.advanceTimersByTimeAsync(2000);
  expect(sent).toEqual([]);
  expect(session.failure).toBeNull();
  session.rename(id, "Call dad");
  await vi.advanceTimersByTimeAsync(2000);
  vi.useRealTimers();
  expect(await session.flush()).toBe(true);
  expect(server.records.get(id)?.draft.title).toBe("Call dad");
  expect(session.failure).toBeNull();
  session.stop();
});

test("leaving a title blank gives back the one it had", async () => {
  const { server, session, id } = await editingSession("Call mom");
  const sent = recordMutations();
  session.rename(id, "  ");
  session.commitText(id);
  expect(session.rows.find((row) => row.id === id)?.title).toBe("Call mom");
  session.rename(id, "");
  expect(await session.flush()).toBe(true);
  expect(sent).toEqual([]);
  expect(server.records.get(id)?.draft.title).toBe("Call mom");
  expect(session.retained).toBe(false);
  session.stop();
});

test("a blank step title waits, and leaving it keeps the old title", async () => {
  const { server, session, id } = await editingSession("Pack");
  await session.load(id);
  await session.updateSteps(id, [
    { id: "step-0000000000000001", title: "Socks", completed: false },
  ]);
  const sent = recordMutations();
  vi.useFakeTimers();
  session.renameStep(id, "step-0000000000000001", "");
  await vi.advanceTimersByTimeAsync(2000);
  vi.useRealTimers();
  expect(sent).toEqual([]);
  expect(session.failure).toBeNull();
  session.renameStep(id, "step-0000000000000001", "Shoes");
  expect(await session.flush()).toBe(true);
  const content = server.records.get(id)!.draft.content;
  expect(content.kind === "task" && content.details.steps?.map((step) => step.title)).toEqual([
    "Shoes",
  ]);
  session.renameStep(id, "step-0000000000000001", "");
  expect(await session.flush()).toBe(true);
  expect(session.rows.find((row) => row.id === id)?.steps?.[0]?.title).toBe("Shoes");
  session.stop();
});

test("unchanged text after typing writes nothing", async () => {
  const { session, id } = await editingSession("Same");
  const sent = recordMutations();
  session.rename(id, "Samey");
  session.rename(id, "Same");
  expect(await session.flush()).toBe(true);
  expect(sent).toEqual([]);
  session.stop();
});

test("a pause after a space saves the words as typed, and leaving tidies them", async () => {
  const { server, session, id } = await editingSession("Call");
  vi.useFakeTimers();
  session.rename(id, "Call ");
  await vi.advanceTimersByTimeAsync(2000);
  vi.useRealTimers();
  await session.flush();
  expect(server.records.get(id)?.draft.title).toBe("Call ");
  // The field still reads what was typed, so the next word follows the space.
  expect(session.rows.find((row) => row.id === id)?.title).toBe("Call ");
  session.rename(id, "Call mom ");
  session.commitText(id);
  await session.flush();
  expect(server.records.get(id)?.draft.title).toBe("Call mom");
  session.stop();
});

test("a task reads as saving only while a write to it is on its way", async () => {
  const { server, session, id } = await editingSession("Slow");
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  host.call.mockImplementation(async (owner, call) => {
    if (call.kind === "mutate") await gate;
    return server.call(owner, call);
  });
  session.rename(id, "Slower");
  // Typing that has not been sent yet is not a save.
  expect(session.saving(id)).toBe(false);
  const done = session.flush();
  expect(session.saving(id)).toBe(true);
  release();
  await done;
  expect(session.saving(id)).toBe(false);
  session.stop();
});

test("a task announced while its own creation is in flight is not read again", async () => {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const { TaskSession } = await import("../tasks.svelte");
  const server = resourceTestServer(profile);
  const reads: string[] = [];
  host.call.mockImplementation(async (owner, call) => {
    if (call.kind === "get") reads.push(call.id);
    const reply = await server.call(owner, call);
    if (call.kind === "mutate" && reply.response.kind === "applied") {
      const { id, revision } = reply.response.record;
      // Native announces the write before the reply reaches the caller.
      host.changed?.({ payload: { profile, kind: "task", id, revision } });
      await settled();
    }
    return reply;
  });
  const session = new TaskSession(profile);
  await session.start();
  await session.create({ title: "Announced" });
  await new Promise((resolve) => setTimeout(resolve, 150));
  expect(reads).toEqual([]);
  expect(session.items.map((item) => item.title)).toEqual(["Announced"]);
  session.stop();
});

test("a capture that fails keeps the words it was sent with for its retry", async () => {
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const { TaskSession } = await import("../tasks.svelte");
  const server = resourceTestServer(profile);
  let fail = true;
  host.call.mockImplementation(async (owner, call) =>
    call.kind === "mutate" && fail
      ? { profile, response: { kind: "error", error: "unavailable" } }
      : server.call(owner, call),
  );
  const session = new TaskSession(profile);
  await session.start();
  // The composer empties its field as it sends, and gives the words back on failure.
  const saving = session.create({ title: "Buy milk", draft: "Buy milk" });
  session.captureDraft = "";
  expect(await saving).toBeNull();
  session.captureDraft = "Buy milk";
  fail = false;
  await session.retry();
  expect(server.records.size).toBe(1);
  expect(session.captureDraft).toBe("");
  session.stop();
});

test("a short pause mid-sentence is not yet a save", async () => {
  const { session, id } = await editingSession("Write");
  const sent = recordMutations();
  vi.useFakeTimers();
  session.rename(id, "Write the");
  await vi.advanceTimersByTimeAsync(600);
  expect(sent).toEqual([]);
  session.rename(id, "Write the report");
  await vi.advanceTimersByTimeAsync(600);
  expect(sent).toEqual([]);
  await vi.advanceTimersByTimeAsync(500);
  vi.useRealTimers();
  expect(sent).toEqual(["update_task"]);
  session.stop();
});
