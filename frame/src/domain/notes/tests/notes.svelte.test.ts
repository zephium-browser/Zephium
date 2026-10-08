import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ChangedNote, NoteCall } from "$shared/ipc/bindings";
import { notesTestServer } from "$shared/testing/notes/server";

const host = vi.hoisted(() => ({
  call: null as null | ((profile: string, call: NoteCall) => Promise<unknown>),
  listener: null as null | ((event: { payload: unknown }) => void),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ noteCall: (profile, call) => host.call!(profile, call) as never });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    notesChanged: {
      listen: (listener: typeof host.listener) => {
        host.listener = listener;
        return Promise.resolve(() => {
          host.listener = null;
        });
      },
    },
  },
}));
vi.mock("$shared/lib/close", () => ({ registerCloseTask: () => () => {} }));

const { NoteSession } = await import("../notes.svelte");

const profile = "01J9ZQ3V6Q4M8Y2K7T5R1N0B3P";
let server: ReturnType<typeof notesTestServer>;

beforeEach(() => {
  vi.useFakeTimers();
  server = notesTestServer(profile, (notes: ChangedNote[], reset: boolean, links: string[]) =>
    queueMicrotask(() => host.listener?.({ payload: { profile, notes, reset, links } })),
  );
  host.call = server.call;
});
afterEach(() => {
  vi.useRealTimers();
});

async function settle(ms = 0) {
  await vi.advanceTimersByTimeAsync(ms);
}

async function started() {
  const session = new NoteSession(profile, "test");
  await session.start();
  return session;
}

test("a new note has no file until it has something in it, then exactly one", async () => {
  const session = await started();
  await session.create();
  expect(session.note?.id).toBeNull();
  await session.close();
  expect(server.writes()).toEqual([]);

  await session.create();
  session.edit("# Groceries");
  session.edit("# Groceries\n\nMilk");
  await settle(1300);
  expect(server.writes().map((call) => call.kind)).toEqual(["create"]);
  expect(session.note?.id).toBeTruthy();
  expect(session.items[0]?.title).toBe("Groceries");
  expect(session.saveState).toBe("saved");
});

test("typing saves after a pause, and at least every few seconds while it goes on", async () => {
  const session = await started();
  const note = server.seed("# Plans\n");
  await session.open(note.id);
  for (let tick = 1; tick <= 20; tick++) {
    session.edit(`# Plans\n\n${"a".repeat(tick)}`);
    await settle(300);
  }
  const during = server.writes().length;
  expect(during).toBeGreaterThanOrEqual(1);
  expect(during).toBeLessThanOrEqual(3);
  await settle(1300);
  expect(server.notes.get(note.id)?.markdown).toBe(`# Plans\n\n${"a".repeat(20)}`);
  expect(session.saveState).toBe("saved");
});

test("the list follows the title as it is typed", async () => {
  const note = server.seed("# Old title\n");
  const session = await started();
  await session.open(note.id);
  session.edit("# New title\n\nBody");
  expect(session.items.find((item) => item.id === note.id)?.title).toBe("New title");
  expect(session.note?.summary?.preview).toBe("Body");
});

test("a change made elsewhere reloads a note with nothing unsaved", async () => {
  const session = await started();
  const note = server.seed("# Shared\n");
  await session.open(note.id);
  const version = session.note!.version;
  server.editOnDisk(note.id, "# Shared\n\nFrom another app");
  await settle(10);
  expect(session.note!.version).toBeGreaterThan(version);
  expect(session.note!.source).toBe("# Shared\n\nFrom another app");
});

test("the browser's own saves do not reload the editor", async () => {
  const session = await started();
  const note = server.seed("# Mine\n");
  await session.open(note.id);
  const version = session.note!.version;
  session.edit("# Mine\n\nTyped here");
  await settle(1300);
  await settle(200);
  expect(session.note!.version).toBe(version);
});

test("a save that meets another app's change keeps both until one is chosen", async () => {
  const session = await started();
  const note = server.seed("# Draft\n");
  await session.open(note.id);
  session.edit("# Draft\n\nmine");
  server.editOnDisk(note.id, "# Draft\n\ntheirs");
  await settle(1300);
  expect(session.saveState).toBe("conflict");
  expect(session.conflict?.markdown).toBe("# Draft\n\ntheirs");
  expect(server.notes.get(note.id)?.markdown).toBe("# Draft\n\ntheirs");

  await session.resolve("mine");
  expect(server.notes.get(note.id)?.markdown).toBe("# Draft\n\nmine");
  expect(session.saveState).toBe("saved");
});

test("choosing the other version replaces the editor's text", async () => {
  const session = await started();
  const note = server.seed("# Draft\n");
  await session.open(note.id);
  session.edit("# Draft\n\nmine");
  server.editOnDisk(note.id, "# Draft\n\ntheirs");
  await settle(1300);
  await session.resolve("theirs");
  expect(session.note?.source).toBe("# Draft\n\ntheirs");
  expect(session.saveState).toBe("saved");
});

test("text whose file disappeared is saved as a new note", async () => {
  const session = await started();
  const note = server.seed("# Doomed\n");
  await session.open(note.id);
  session.edit("# Doomed\n\nstill typing");
  server.removeOnDisk(note.id);
  await settle(1300);
  await settle(1300);
  const saved = [...server.notes.values()].find((stored) =>
    stored.markdown.includes("still typing"),
  );
  expect(saved).toBeTruthy();
  expect(saved!.summary.id).not.toBe(note.id);
  expect(session.note?.id).toBe(saved!.summary.id);
});

test("an unknown outcome is retried with the same request, never duplicated", async () => {
  const session = await started();
  await session.create();
  let dropped = false;
  host.call = async (expected: string, call: NoteCall) => {
    const reply = await server.call(expected, call);
    if (!dropped) {
      dropped = true;
      return { profile, response: { kind: "error", error: "outcome_unknown" } };
    }
    return reply;
  };
  session.edit("# Once\n");
  await settle(1300);
  expect(session.saveState).toBe("retrying");
  await settle(2000);
  const creates = server.calls.filter((call) => call.kind === "create");
  expect(creates).toHaveLength(2);
  expect(creates[0]).toMatchObject({
    request_id: (creates[1] as { request_id: string }).request_id,
  });
  expect(server.notes.size).toBe(1);
  expect(session.saveState).toBe("saved");
});

test("a note emptied and left goes to the trash, and trash can be undone", async () => {
  const session = await started();
  const note = server.seed("# Temporary\n\ntext");
  await session.open(note.id);
  session.edit("");
  await session.close();
  expect(server.notes.get(note.id)?.summary.trashed).toBe(true);

  const kept = server.seed("# Kept\n");
  await session.reload();
  await session.moveToTrash(kept.id);
  expect(session.items.some((item) => item.id === kept.id)).toBe(false);
  expect(session.notice?.id).toBe(kept.id);
  await session.undo();
  expect(server.notes.get(kept.id)?.summary.trashed).toBe(false);
  expect(session.items.some((item) => item.id === kept.id)).toBe(true);
});

test("hiding the host saves pending text before letting go", async () => {
  const session = await started();
  const note = server.seed("# Hidden\n");
  await session.open(note.id);
  session.edit("# Hidden\n\nlast words");
  session.stop();
  await settle(10);
  expect(server.notes.get(note.id)?.markdown).toBe("# Hidden\n\nlast words");
});

test("its own saves cost no listing and no link lookups", async () => {
  const session = await started();
  const note = server.seed("# Plans\n\nSee [[Roadmap]]");
  await session.open(note.id);
  await session.resolveTargets(["Roadmap"]);
  await settle(200);
  const before = server.calls.filter((call) => call.kind !== "write").length;
  for (let tick = 1; tick <= 5; tick++) {
    session.edit(`# Plans\n\nSee [[Roadmap]] ${tick}`);
    await settle(1300);
  }
  expect(server.writes().length).toBe(5);
  expect(server.calls.filter((call) => call.kind !== "write")).toHaveLength(before);
  expect(session.items.find((item) => item.id === note.id)?.revision).toBe(
    server.notes.get(note.id)?.summary.revision,
  );

  // A new title changes where links to the old and the new title lead, and
  // nothing else: the listing already shows it and other links stay cached.
  await session.resolveTargets(["Plans", "Roadmap"]);
  const retitled = server.calls.length;
  session.edit("# Roadmap notes\n\nSee [[Roadmap]]");
  await settle(1300);
  const since = server.calls.length;
  await session.resolveTargets(["Roadmap"]);
  expect(server.calls.length).toBe(since);
  await session.resolveTargets(["Plans"]);
  expect(server.calls.slice(since).map((call) => call.kind)).toEqual(["resolve"]);
  expect(server.calls.slice(retitled).some((call) => call.kind === "list")).toBe(false);
});

test("a retitled file takes its new name once the title settles", async () => {
  const session = await started();
  const note = server.seed("# Draft\n");
  await session.open(note.id);
  session.edit("# Fin\n");
  await settle(1300);
  session.edit("# Final\n");
  await settle(1300);
  expect(server.writes().every((call) => call.kind === "write" && !call.settle)).toBe(true);
  expect(server.notes.get(note.id)?.summary.path).toBe("Draft.md");
  await settle(5000);
  expect(server.notes.get(note.id)?.summary.path).toBe("Final.md");
  expect(session.items.find((item) => item.id === note.id)?.path).toBe("Final.md");
});

test("leaving a retitled note gives its file the new name at once", async () => {
  const session = await started();
  const note = server.seed("# Draft\n");
  await session.open(note.id);
  session.edit("# Final\n\nDone");
  expect(await session.close()).toBe(true);
  expect(server.notes.get(note.id)?.summary.path).toBe("Final.md");
  expect(server.notes.get(note.id)?.markdown).toBe("# Final\n\nDone");
});

test("a note's first save is not mistaken for someone else's new note", async () => {
  const session = await started();
  await session.create();
  const before = server.calls.length;
  session.edit("# Groceries\n");
  await settle(1300);
  await settle(200);
  expect(server.calls.slice(before).map((call) => call.kind)).toEqual(["create", "backlinks"]);
  expect(session.items[0]?.title).toBe("Groceries");
});

test("a pause shorter than a breath is not yet a save", async () => {
  const session = await started();
  const note = server.seed("# Pace\n");
  await session.open(note.id);
  session.edit("# Pace\n\nOne");
  await settle(800);
  expect(server.writes()).toEqual([]);
  session.edit("# Pace\n\nOne two");
  await settle(800);
  expect(server.writes()).toEqual([]);
  await settle(500);
  expect(server.writes()).toHaveLength(1);
});

test("typing reads the whole note out only to save it", async () => {
  const note = server.seed("# Long\n");
  const session = await started();
  await session.open(note.id);
  const reads: (number | undefined)[] = [];
  for (let tick = 1; tick <= 5; tick++) {
    const text = `# Long read\n\n${"word ".repeat(tick)}`;
    session.edit((blocks) => {
      reads.push(blocks);
      return text;
    });
  }
  expect(reads.every((blocks) => blocks !== undefined)).toBe(true);
  expect(session.items.find((item) => item.id === note.id)?.title).toBe("Long read");
  await settle(1300);
  expect(reads.filter((blocks) => blocks === undefined)).toHaveLength(1);
  expect(server.notes.get(note.id)?.markdown).toBe(`# Long read\n\n${"word ".repeat(5)}`);
});

test("leaving a note saves typing that was never read out", async () => {
  const session = await started();
  const note = server.seed("# Quick\n");
  await session.open(note.id);
  session.edit(() => "# Quick\n\nlast words");
  expect(await session.close()).toBe(true);
  expect(server.notes.get(note.id)?.markdown).toBe("# Quick\n\nlast words");
});

test("a save keeps the list's order unless the note has moved in it", async () => {
  server.seed("# Older\n");
  server.seed("# Newer\n");
  const session = await started();
  expect(session.items.map((item) => item.title)).toEqual(["Newer", "Older"]);
  const newer = session.items[0]!;
  await session.open(newer.id);
  session.edit("# Newer\n\nmore");
  await settle(1300);
  expect(session.items.map((item) => item.title)).toEqual(["Newer", "Older"]);
  expect(session.items[0]!.revision).not.toBe(newer.revision);
  await session.open(session.items[1]!.id);
  session.edit("# Older\n\nrevived");
  await settle(1300);
  expect(session.items.map((item) => item.title)).toEqual(["Older", "Newer"]);
});
