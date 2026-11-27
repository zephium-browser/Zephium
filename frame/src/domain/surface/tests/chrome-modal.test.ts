import { expect, it, vi } from "vitest";
const native = vi.hoisted(() => ({
  pending: [] as (() => void)[],
  modal: vi.fn((_open: boolean) => new Promise<void>((resolve) => native.pending.push(resolve))),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ chromeModal: native.modal });
});
const { holdChromeModal } = await import("../chrome-modal");

const settle = () => new Promise((resolve) => setTimeout(resolve));

it("reports one state at a time, the latest last", async () => {
  const release = holdChromeModal();
  await settle();
  expect(native.modal.mock.calls).toEqual([[true]]);
  release();
  await settle();
  // The close waits for the open to land.
  expect(native.modal.mock.calls).toEqual([[true]]);
  native.pending.shift()?.();
  await settle();
  expect(native.modal.mock.calls).toEqual([[true], [false]]);
  native.pending.shift()?.();
  await settle();
  expect(native.modal.mock.calls).toEqual([[true], [false]]);
});

it("sends nothing when a dialog closes before its report lands and nothing changed", async () => {
  native.modal.mockClear();
  const release = holdChromeModal();
  await settle();
  release();
  native.pending.shift()?.();
  await settle();
  native.pending.shift()?.();
  holdChromeModal()();
  await settle();
  expect(native.modal.mock.calls).toEqual([[true], [false]]);
});
