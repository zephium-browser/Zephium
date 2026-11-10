import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type {
  ResourceCall_Deserialize as ResourceCall,
  ResourceRecord_Serialize as ResourceRecord,
} from "$shared/ipc/bindings";
import { resourceTestServer } from "$shared/testing/resources/server";
const mocks = vi.hoisted(() => ({
  call: vi.fn(),
  listen: vi.fn(),
  stop: vi.fn(),
  close: null as null | (() => Promise<boolean>),
}));
vi.mock("../transport", () => ({ resourceCall: mocks.call }));
vi.mock("$shared/ipc/native-events", () => ({
  events: { resourceChanged: { listen: mocks.listen } },
}));
vi.mock("$shared/lib/close", () => ({
  registerCloseTask: (task: () => Promise<boolean>) => {
    mocks.close = task;
    return () => {
      mocks.close = null;
    };
  },
}));
import { OwnedObjects } from "../owned-objects";

const record = (revision = "1", done = false): ResourceRecord => ({
  id: "block",
  revision,
  created_at: "1",
  updated_at: revision,
  trashed: false,
  draft: {
    title: "Checklist",
    pinned: false,
    related: [],
    content: {
      kind: "object",
      object: {
        version: 1,
        evidence: [],
        data: { kind: "list", style: "todo", items: [{ title: "Passport", done }] },
      },
    },
  },
});
beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  mocks.close = null;
  mocks.listen.mockResolvedValue(mocks.stop);
});
afterEach(() => vi.useRealTimers());

test("a conflict preserves the draft and never overwrites a newer record", async () => {
  mocks.call.mockResolvedValueOnce({ kind: "record", record: record() });
  const owned = new OwnedObjects("profile");
  await owned.load(["block"]);
  await vi.waitFor(() => expect(owned.get("block")?.revision).toBe("1"));
  owned.update("block", {
    kind: "list",
    style: "todo",
    items: [{ title: "Passport", done: true }],
  });
  expect(owned.revisions().has("block")).toBe(false);
  mocks.call.mockResolvedValue({ kind: "error", error: "conflict" });
  expect(await owned.flushAll()).toBe(false);
  expect(mocks.call.mock.calls.filter(([, call]) => call.kind === "mutate")).toHaveLength(1);
  expect(owned.get("block")?.data).toMatchObject({ items: [{ done: true }] });
  expect(owned.get("block")?.revision).toBe("1");
  expect(owned.failed).toBe(true);
});

test("a conflict rereads the revision, keeps edits made during recovery and permits later edits", async () => {
  const server = resourceTestServer("profile");
  server.records.set("block", record());
  let resolve!: () => void;
  const reread = new Promise<void>((done) => (resolve = done));
  mocks.call.mockImplementation(async (_profile: string, call: ResourceCall) => {
    if (call.kind === "get" && server.records.get("block")?.revision === "2") await reread;
    return (await server.call("profile", call)).response;
  });
  const owned = new OwnedObjects("profile");
  await owned.load(["block"]);
  await vi.waitFor(() => expect(owned.get("block")?.revision).toBe("1"));
  const edit = { kind: "list", style: "todo", items: [{ title: "Passport", done: true }] } as const;
  owned.update("block", { ...edit, items: [...edit.items] });
  const stored = record("2");
  stored.draft.title = "Renamed elsewhere";
  stored.draft.pinned = true;
  server.records.set("block", stored);
  const saving = owned.flushAll();
  await vi.waitFor(() =>
    expect(mocks.call.mock.calls.filter(([, call]) => call.kind === "get")).toHaveLength(2),
  );
  owned.update("block", { ...edit, items: [{ title: "Passport and visa", done: true }] });
  resolve();
  expect(await saving).toBe(true);
  expect(owned.get("block")).toMatchObject({
    revision: "3",
    title: "Renamed elsewhere",
    data: { items: [{ title: "Passport and visa", done: true }] },
  });
  expect(server.records.get("block")?.draft.pinned).toBe(true);
  owned.update("block", { ...edit, items: [{ title: "Ready to travel", done: true }] });
  expect(await owned.flushAll()).toBe(true);
  expect(owned.get("block")?.revision).toBe("4");
  expect(owned.failed).toBe(false);
  expect(
    mocks.call.mock.calls
      .filter(([, call]) => call.kind === "mutate")
      .map(([, call]) => call.command.intent.expected_revision),
  ).toEqual(["1", "2", "3"]);
  owned.stop();
});

test.each(["changed", "trashed", "missing"])(
  "an unreapplicable conflict is surfaced without discarding local text (%s)",
  async (reason) => {
    mocks.call.mockResolvedValueOnce({ kind: "record", record: record() });
    const owned = new OwnedObjects("profile");
    await owned.load(["block"]);
    await vi.waitFor(() => expect(owned.get("block")?.revision).toBe("1"));
    owned.update("block", { kind: "list", style: "todo", items: [{ title: "My pending words" }] });
    const fresh = record("2", true);
    fresh.trashed = reason === "trashed";
    mocks.call.mockImplementation(async (_profile: string, call: ResourceCall) =>
      call.kind === "get" && reason !== "missing"
        ? { kind: "record", record: fresh }
        : { kind: "error", error: call.kind === "get" ? "not_found" : "conflict" },
    );
    expect(await owned.flushAll()).toBe(false);
    expect(owned.failed).toBe(true);
    expect(owned.get("block")?.data).toMatchObject({ items: [{ title: "My pending words" }] });
    expect(mocks.call.mock.calls.filter(([, call]) => call.kind === "mutate")).toHaveLength(1);
    expect(await mocks.close?.()).toBe(false);
  },
);

test("close flushes a pending debounced write before allowing the window to close", async () => {
  const server = resourceTestServer("profile");
  server.records.set("block", record());
  mocks.call.mockImplementation(
    async (_profile: string, call: ResourceCall) => (await server.call("profile", call)).response,
  );
  const owned = new OwnedObjects("profile");
  await owned.start();
  await owned.load(["block"]);
  await vi.waitFor(() => expect(owned.get("block")?.revision).toBe("1"));
  owned.update("block", {
    kind: "list",
    style: "todo",
    items: [{ title: "Last words before closing" }],
  });
  let resolve!: () => void;
  const applied = new Promise<void>((done) => (resolve = done));
  mocks.call.mockImplementation(async (_profile: string, call: ResourceCall) => {
    if (call.kind === "mutate") await applied;
    return (await server.call("profile", call)).response;
  });
  let closed = false;
  const closing = mocks.close?.().then((saved) => (closed = saved));
  await vi.advanceTimersByTimeAsync(0);
  expect(mocks.call.mock.calls.filter(([, call]) => call.kind === "mutate")).toHaveLength(1);
  expect(closed).toBe(false);
  resolve();
  expect(await closing).toBe(true);
  expect(owned.get("block")?.data).toMatchObject({
    items: [{ title: "Last words before closing" }],
  });
  await vi.advanceTimersByTimeAsync(450);
  expect(mocks.call.mock.calls.filter(([, call]) => call.kind === "mutate")).toHaveLength(1);
  owned.stop();
  await vi.advanceTimersByTimeAsync(0);
  expect(mocks.close).toBeNull();
});

test("authoritative block events refresh the record and stale events cannot regress it", async () => {
  mocks.call.mockResolvedValue({ kind: "record", record: record() });
  const owned = new OwnedObjects("profile");
  await owned.start();
  await owned.load(["block"]);
  await vi.waitFor(() => expect(owned.get("block")?.revision).toBe("1"));
  const notify = mocks.listen.mock.calls[0]![0];
  mocks.call.mockResolvedValue({ kind: "record", record: record("3", true) });
  notify({ payload: { profile: "profile", kind: "object", id: "block", revision: "3" } });
  await vi.waitFor(() => expect(owned.get("block")?.revision).toBe("3"));
  mocks.call.mockResolvedValue({ kind: "record", record: record("2", false) });
  notify({ payload: { profile: "profile", kind: "object", id: "block", revision: "2" } });
  await Promise.resolve();
  await Promise.resolve();
  expect(owned.get("block")?.revision).toBe("3");
  owned.stop();
  expect(mocks.stop).toHaveBeenCalledOnce();
});
