import { expect, test, vi } from "vitest";
import type { ProfilesView } from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({
  get: vi.fn(),
  notify: null as ((event: { payload: ProfilesView }) => void) | null,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ profilesGet: native.get });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    profilesChanged: {
      listen: (listener: typeof native.notify) => {
        native.notify = listener;
        return Promise.resolve(() => {
          native.notify = null;
        });
      },
    },
  },
}));

import { all, current, inPrivate, release, retain } from "../profiles.svelte";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("a delayed initial read cannot restore a profile removed by a live update", async () => {
  const personal = {
    id: "personal",
    name: "Personal",
    kind: "default" as const,
    avatar: { style: "initials" as const, color: 1, symbol: 0, revision: 0 },
  };
  const work = { ...personal, id: "work", name: "Work", kind: "named" as const };
  const stale: ProfilesView = { current: work.id, private: true, profiles: [personal, work] };
  const latest: ProfilesView = { current: personal.id, private: false, profiles: [personal] };
  const reading = deferred<void>();
  const reply = deferred<{ status: "ok"; data: ProfilesView }>();
  native.get.mockImplementationOnce(() => {
    reading.resolve();
    return reply.promise;
  });

  const retained = retain();
  try {
    await reading.promise;
    native.notify?.({ payload: latest });
    expect(current()?.id).toBe(personal.id);
    reply.resolve({ status: "ok", data: stale });
    await retained;
    expect(all().map((profile) => profile.id)).toEqual([personal.id]);
    expect(current()?.id).toBe(personal.id);
    expect(inPrivate()).toBe(false);
  } finally {
    release();
  }
});
