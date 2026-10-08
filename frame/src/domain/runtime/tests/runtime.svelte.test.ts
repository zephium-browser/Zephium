import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RuntimeStatus } from "$shared/ipc/bindings";

const native = vi.hoisted(() => {
  let listener: ((event: { payload: RuntimeStatus }) => void) | undefined;
  const stop = vi.fn();
  const listen = vi.fn((next: (event: { payload: RuntimeStatus }) => void) => {
    listener = next;
    return Promise.resolve(stop);
  });

  return {
    listen,
    stop,
    emit(status: RuntimeStatus) {
      listener?.({ payload: status });
    },
    reset() {
      listener = undefined;
      listen.mockClear();
      stop.mockClear();
    },
  };
});

vi.mock("$shared/ipc/native-events", () => ({
  events: {
    runtimeStatusChanged: {
      listen: native.listen,
    },
  },
}));

describe("runtime status projection state", () => {
  beforeEach(() => {
    vi.resetModules();
    native.reset();
  });

  it("installs one listener, accepts the actor projection, and clears on disposal", async () => {
    const runtime = await import("../runtime.svelte");
    const first = runtime.init();
    const second = runtime.init();

    expect(first).toBe(second);
    await first;
    expect(native.listen).toHaveBeenCalledOnce();

    native.emit({
      restart_required: false,
      session_set_aside: false,
      user_content_degraded_scope_count: 0,
      security_advisories: [
        {
          kind: "update_recommended",
          update_target: "operating_system",
        },
      ],
    });
    expect(runtime.status().security_advisories[0]?.update_target).toBe("operating_system");

    runtime.dispose();
    runtime.dispose();
    expect(native.stop).toHaveBeenCalledOnce();
    expect(runtime.status()).toEqual({
      restart_required: false,
      session_set_aside: false,
      user_content_degraded_scope_count: 0,
      security_advisories: [],
    });
  });
});
