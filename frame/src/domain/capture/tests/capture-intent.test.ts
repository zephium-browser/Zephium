import { beforeEach, expect, test, vi } from "vitest";
import { stopCaptureFor } from "../capture-intent";

const native = vi.hoisted(() => ({ stop: vi.fn(), settle: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ captureStop: native.stop });
});
vi.mock("$domain/operations", () => ({ operations: {}, settle: native.settle }));

beforeEach(() => {
  vi.clearAllMocks();
  native.stop.mockResolvedValue({ accepted: true, operation_id: "stop" });
  native.settle.mockResolvedValue({ outcome: "deferred", disposition: null });
});

test("a retained stop control echoes its original item and document", async () => {
  let item = "original-tab";
  let navigation = "0000000000000012";
  const stop = stopCaptureFor(item, navigation);
  item = "replacement-tab";
  navigation = "0000000000000013";
  await expect(stop()).resolves.toBe(true);
  expect(native.stop).toHaveBeenCalledExactlyOnceWith("original-tab", "0000000000000012");
  const replacement = stopCaptureFor(item, navigation);
  await replacement();
  expect(native.stop).toHaveBeenLastCalledWith("replacement-tab", "0000000000000013");
});
