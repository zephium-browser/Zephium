import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OperationDisposition } from "$shared/ipc/bindings";

const native = vi.hoisted(() => {
  let listener: ((event: { payload: OperationDisposition }) => void) | null = null;
  const commands = {
    operationAcknowledge: vi.fn(),
    operationsReconcile: vi.fn(),
    operationStatus: vi.fn(),
  };
  const listen = vi.fn(
    async (next: (event: { payload: OperationDisposition }) => void): Promise<() => void> => {
      listener = next;
      return () => {
        if (listener === next) listener = null;
      };
    },
  );

  return {
    commands,
    listen,
    emit(disposition: OperationDisposition) {
      listener?.({ payload: disposition });
    },
    reset() {
      listener = null;
      commands.operationAcknowledge.mockReset().mockResolvedValue(true);
      commands.operationsReconcile.mockReset().mockResolvedValue([]);
      commands.operationStatus.mockReset().mockResolvedValue({ state: "pending" });
      listen.mockClear();
    },
  };
});

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings(native.commands);
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    operationProcessed: {
      listen: native.listen,
    },
  },
}));

function disposition(operationId: string): OperationDisposition {
  return {
    operation_id: operationId,
    outcome: "applied",
    reason: "mutation_applied",
  };
}

async function loadOperations() {
  return import("../operations");
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetModules();
  native.reset();
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("operation settlement", () => {
  it("rejects malformed ids without querying native state", async () => {
    const operations = await loadOperations();

    await expect(operations.waitForDisposition("not-an-operation", 10_000)).resolves.toEqual({
      state: "unknown",
      operation_id: "not-an-operation",
    });
    expect(native.commands.operationStatus).not.toHaveBeenCalled();
  });

  it("installs the event listener before reconciling the native ledger", async () => {
    const order: string[] = [];
    native.listen.mockImplementationOnce(async () => {
      order.push("listen");
      return () => undefined;
    });
    native.commands.operationsReconcile.mockImplementationOnce(async () => {
      order.push("reconcile");
      return [disposition("0000000000000001")];
    });
    const operations = await loadOperations();

    await operations.init();
    await flushPromises();

    expect(order).toEqual(["listen", "reconcile"]);
    await vi.advanceTimersByTimeAsync(0);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledWith("0000000000000001");
    operations.dispose();
  });

  it("settles from a scoped event and acknowledges the exact id", async () => {
    const operations = await loadOperations();
    await operations.init();
    await flushPromises();

    const waiting = operations.waitForDisposition("0000000000000002", 10_000);
    await flushPromises();
    native.emit(disposition("0000000000000002"));

    await expect(waiting).resolves.toEqual({
      state: "processed",
      disposition: disposition("0000000000000002"),
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledTimes(1);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledWith("0000000000000002");
    operations.dispose();
  });

  it("settles from the fallback status query without inventing a result", async () => {
    native.commands.operationStatus.mockResolvedValueOnce({
      state: "processed",
      disposition: disposition("0000000000000003"),
    });
    const operations = await loadOperations();

    await expect(operations.waitForDisposition("0000000000000003", 10_000)).resolves.toEqual({
      state: "processed",
      disposition: disposition("0000000000000003"),
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledWith("0000000000000003");
  });

  it("reports pending when bounded queries never establish settlement", async () => {
    native.commands.operationStatus.mockRejectedValue(new Error("temporarily unavailable"));
    const operations = await loadOperations();
    const waiting = operations.waitForDisposition("0000000000000004", 2_100);

    await vi.advanceTimersByTimeAsync(2_100);

    await expect(waiting).resolves.toEqual({
      state: "pending",
      operation_id: "0000000000000004",
    });
  });

  it("retains an acknowledgement after failure and retries at the bounded cadence", async () => {
    native.commands.operationsReconcile.mockResolvedValueOnce([disposition("0000000000000005")]);
    native.commands.operationAcknowledge
      .mockRejectedValueOnce(new Error("temporary failure"))
      .mockResolvedValueOnce(false);
    const operations = await loadOperations();

    await operations.init();
    await flushPromises();
    await vi.advanceTimersByTimeAsync(0);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1_000);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledTimes(2);
    operations.dispose();
  });

  it("deduplicates repeated dispositions before acknowledgement", async () => {
    const operations = await loadOperations();
    await operations.init();
    await flushPromises();
    const result = disposition("0000000000000006");

    native.emit(result);
    native.emit({ ...result, outcome: "rejected", reason: "invalid_input" });
    await vi.advanceTimersByTimeAsync(0);

    expect(operations.result(result.operation_id)).toEqual(result);
    expect(native.commands.operationAcknowledge).toHaveBeenCalledTimes(1);
    operations.dispose();
  });
});

it("does not admit a late ledger response after disposal", async () => {
  let finish!: (value: unknown) => void;
  native.commands.operationStatus.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const operations = await loadOperations();
  const pending = operations.waitForDisposition("0000000000000020", 1000);
  operations.dispose();
  finish({ state: "processed", disposition: disposition("0000000000000020") });
  await expect(pending).resolves.toEqual({ state: "pending", operation_id: "0000000000000020" });
  expect(operations.result("0000000000000020")).toBeUndefined();
  expect(native.commands.operationAcknowledge).not.toHaveBeenCalled();
});
it("releases an abortable settlement wait without accepting a different operation", async () => {
  const operations = await loadOperations();
  native.commands.operationStatus.mockResolvedValue({
    state: "processed",
    disposition: disposition("0000000000000022"),
  });
  const controller = new AbortController();
  const pending = operations.waitForDisposition("0000000000000021", 5000, controller.signal);
  await flushPromises();
  controller.abort();
  await expect(pending).resolves.toEqual({ state: "pending", operation_id: "0000000000000021" });
  expect(operations.result("0000000000000022")).toBeUndefined();
  expect(vi.getTimerCount()).toBe(0);
});

it("disposal releases a hung ledger observation without waiting for the IPC timeout", async () => {
  const operations = await loadOperations();
  native.commands.operationStatus.mockImplementation(() => new Promise(() => {}));
  const pending = operations.waitForDisposition("0000000000000030", 5000);
  operations.dispose();
  await expect(pending).resolves.toEqual({ state: "pending", operation_id: "0000000000000030" });
  expect(vi.getTimerCount()).toBe(0);
});

it("keeps an event-only observer after a timed-out sheet and releases it on completion", async () => {
  const operations = await import("../operations");
  await operations.init();
  const receive = vi.fn();
  const stop = operations.onDisposition("00000000000000a1", receive);
  expect(receive).not.toHaveBeenCalled();
  native.emit(disposition("00000000000000a2"));
  expect(receive).not.toHaveBeenCalled();
  native.emit(disposition("00000000000000a1"));
  expect(receive).toHaveBeenCalledExactlyOnceWith(disposition("00000000000000a1"));
  stop();
  native.emit(disposition("00000000000000a1"));
  expect(receive).toHaveBeenCalledTimes(1);
  const cached = vi.fn();
  operations.onDisposition("00000000000000a1", cached)();
  expect(cached).toHaveBeenCalledExactlyOnceWith(disposition("00000000000000a1"));
});

it("acknowledges completion even when a retained observer throws", async () => {
  const operations = await loadOperations();
  await operations.init();
  operations.onDisposition("00000000000000b1", () => {
    throw new Error("storage unavailable");
  });
  expect(() => native.emit(disposition("00000000000000b1"))).not.toThrow();
  await vi.advanceTimersByTimeAsync(0);
  expect(native.commands.operationAcknowledge).toHaveBeenCalledWith("00000000000000b1");
});
