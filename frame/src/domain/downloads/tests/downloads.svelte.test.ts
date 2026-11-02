import { beforeEach, expect, test, vi } from "vitest";
import type { DownloadResponse, DownloadView } from "$shared/ipc/bindings";

const host = vi.hoisted(() => ({
  call: vi.fn(),
  listener: null as null | ((event: { payload: { profile: string } }) => void),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ downloadCall: host.call });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    downloadsChanged: {
      listen: (listener: typeof host.listener) => {
        host.listener = listener;
        return Promise.resolve(() => {
          host.listener = null;
        });
      },
    },
  },
}));
const profile = "00000000000000000000000001";
const entry: DownloadView = {
  id: "00000000000000000000000002",
  revision: "00000001",
  created_at: "1",
  filename: "fixture.txt",
  source: "https://example.com",
  source_is_context: false,
  state: "receiving",
  received: "10",
  total: "100",
  error: null,
};
function deferred() {
  let resolve!: (response: DownloadResponse) => void;
  return {
    promise: new Promise<DownloadResponse>((done) => {
      resolve = done;
    }),
    resolve: (response: DownloadResponse) => resolve(response),
  };
}
beforeEach(() => {
  vi.resetModules();
  host.call.mockReset();
  host.listener = null;
});

test.each([false, true])(
  "loading more reconciles a previously observed download (newer live update: %s)",
  async (liveUpdate) => {
    const first = { ...entry, id: "00000000000000000000000050" };
    const older = { ...entry, id: "00000000000000000000000001" };
    const completed: DownloadView = {
      ...entry,
      revision: "00000002",
      state: "completed",
      received: "100",
    };
    host.call.mockResolvedValueOnce({
      kind: "page",
      entries: [first],
      next: first.id,
      supported: true,
      cleanup: { running: false, error: null },
    });
    const { DownloadSession } = await import("../downloads.svelte");
    const session = new DownloadSession(profile);
    await session.reload();
    host.call.mockResolvedValueOnce({
      kind: "updates",
      entries: [entry],
      removed: [],
      cleanup: { running: false, error: null },
    });
    await session.refresh();

    const page = deferred();
    host.call.mockReturnValueOnce(page.promise);
    const loading = session.reload(true);
    expect(host.call).toHaveBeenLastCalledWith(profile, {
      kind: "list",
      before: first.id,
      limit: 50,
    });
    if (liveUpdate) {
      host.call.mockResolvedValueOnce({
        kind: "updates",
        entries: [completed],
        removed: [],
        cleanup: { running: false, error: null },
      });
      await session.refresh();
    }
    page.resolve({
      kind: "page",
      entries: [liveUpdate ? entry : completed, older],
      next: null,
      supported: true,
      cleanup: { running: false, error: null },
    });
    await loading;
    expect(session.entries).toEqual([first, completed, older]);
    expect(session.next).toBeNull();
    session.stop();
  },
);

test("the status indicator reads a bounded snapshot on subscribe and ignores another profile", async () => {
  host.call.mockResolvedValue({
    kind: "updates",
    cleanup: { running: false, error: null },
    entries: [entry],
    removed: [],
  });
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  await session.start(false);
  expect(host.call).toHaveBeenCalledTimes(1);
  host.listener?.({ payload: { profile: "other" } });
  expect(host.call).toHaveBeenCalledTimes(1);
  host.listener?.({ payload: { profile } });
  await vi.waitFor(() => expect(session.entries).toHaveLength(1));
  expect(host.call).toHaveBeenCalledWith(profile, { kind: "updates" });
  session.stop();
});

test("a late history page cannot roll back newer native progress", async () => {
  const page = deferred();
  host.call.mockImplementation((_profile, call) =>
    call.kind === "list"
      ? page.promise
      : Promise.resolve({
          kind: "updates",
          cleanup: { running: false, error: null },
          entries: [{ ...entry, revision: "00000002", received: "80" }],
          removed: [],
        }),
  );
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  const loading = session.reload();
  await session.refresh();
  page.resolve({
    kind: "page",
    cleanup: { running: false, error: null },
    entries: [entry],
    next: null,
    supported: true,
  });
  await loading;
  expect(session.entries[0]?.received).toBe("80");
});

test("a removal cannot be resurrected by an older in-flight history page", async () => {
  const page = deferred();
  host.call.mockImplementation((_profile, call) =>
    call.kind === "list"
      ? page.promise
      : Promise.resolve({
          kind: "updates",
          cleanup: { running: false, error: null },
          entries: [],
          removed: [entry.id],
        }),
  );
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  const loading = session.reload();
  await session.refresh();
  page.resolve({
    kind: "page",
    cleanup: { running: false, error: null },
    entries: [entry],
    next: null,
    supported: true,
  });
  await loading;
  expect(session.entries).toHaveLength(0);
});

test("a stopped profile ignores an outstanding native response", async () => {
  const page = deferred();
  host.call.mockReturnValue(page.promise);
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  const loading = session.reload();
  session.stop();
  page.resolve({
    kind: "page",
    cleanup: { running: false, error: null },
    entries: [entry],
    next: null,
    supported: true,
  });
  await loading;
  expect(session.entries).toHaveLength(0);
});

test("a download started during a history read survives an older empty page", async () => {
  const page = deferred();
  host.call.mockImplementation((_profile, call) =>
    call.kind === "list"
      ? page.promise
      : Promise.resolve({
          kind: "updates",
          cleanup: { running: false, error: null },
          entries: [entry],
          removed: [],
        }),
  );
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  const loading = session.reload();
  await session.refresh();
  page.resolve({
    kind: "page",
    cleanup: { running: false, error: null },
    entries: [],
    next: null,
    supported: true,
  });
  await loading;
  expect(session.entries).toEqual([entry]);
});

test("cleanup failures remain visible without hiding history and retries request native cleanup", async () => {
  host.call.mockImplementation((_profile, call) =>
    Promise.resolve(
      call.kind === "retry_cleanup"
        ? { kind: "accepted" }
        : {
            kind: "updates",
            entries: [entry],
            removed: [],
            cleanup: { running: false, error: "changed_file" },
          },
    ),
  );
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  await session.start(false);
  expect(session.entries).toEqual([entry]);
  expect(session.cleanup.error).toBe("changed_file");
  await session.perform({ kind: "retry_cleanup" });
  expect(host.call).toHaveBeenCalledWith(profile, { kind: "retry_cleanup" });
  session.stop();
});

test("late history cannot erase a newer cleanup failure", async () => {
  const page = deferred();
  host.call.mockImplementation((_profile, call) =>
    call.kind === "list"
      ? page.promise
      : Promise.resolve({
          kind: "updates",
          entries: [],
          removed: [],
          cleanup: { running: false, error: "destination" },
        }),
  );
  const { DownloadSession } = await import("../downloads.svelte");
  const session = new DownloadSession(profile);
  const loading = session.reload();
  await session.refresh();
  page.resolve({
    kind: "page",
    entries: [],
    next: null,
    supported: true,
    cleanup: { running: false, error: null },
  });
  await loading;
  expect(session.cleanup.error).toBe("destination");
});
