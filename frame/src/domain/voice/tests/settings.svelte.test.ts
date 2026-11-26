import { beforeEach, expect, test, vi } from "vitest";
import type { VoiceStatusV1 } from "$shared/ipc/bindings";
import { VoiceSettings } from "../settings.svelte";

const native = vi.hoisted(() => ({
  status: vi.fn<() => Promise<VoiceStatusV1>>(),
  listenModels: vi.fn<(listener: () => void) => Promise<() => void>>(),
  listenAccount: vi.fn<(listener: () => void) => Promise<() => void>>(),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    voiceStatus: native.status,
  });
});

vi.mock("$shared/ipc/native-events", () => ({
  events: {
    workModelsChanged: {
      listen: native.listenModels,
    },
    accountChanged: {
      listen: native.listenAccount,
    },
  },
}));

const status = (): VoiceStatusV1 => ({
  version: 1,
  engines: ["on_device"],
  chosen: null,
  effective: "on_device",
  language: null,
  languages: ["en-US"],
  on_device: "en_US",
  microphone: "granted",
});

beforeEach(() => {
  vi.resetAllMocks();
  native.status.mockResolvedValue(status());
});

test("a refused subscription releases the sibling listener and marks settings unavailable", async () => {
  const stopModels = vi.fn();
  native.listenModels.mockResolvedValueOnce(stopModels);
  native.listenAccount.mockRejectedValueOnce(new Error("subscription refused"));

  const settings = new VoiceSettings();
  await settings.start();

  expect(stopModels).toHaveBeenCalledOnce();
  expect(native.status).not.toHaveBeenCalled();
  expect(settings.unavailable).toBe(true);
  settings.dispose();
  expect(stopModels).toHaveBeenCalledOnce();
});

test("ignores an event during setup when the sibling subscription is refused", async () => {
  let modelsChanged!: () => void;
  let refuseAccount!: (error: Error) => void;
  native.listenModels.mockImplementationOnce((listener) => {
    modelsChanged = listener;
    return Promise.resolve(vi.fn());
  });
  native.listenAccount.mockImplementationOnce(
    () =>
      new Promise((_, reject) => {
        refuseAccount = reject;
      }),
  );

  const settings = new VoiceSettings();
  const starting = settings.start();
  await Promise.resolve();
  modelsChanged();
  refuseAccount(new Error("subscription refused"));
  await starting;

  expect(native.status).not.toHaveBeenCalled();
  expect(settings.unavailable).toBe(true);
  settings.dispose();
});

test("restarting while a previous start is still subscribing releases the stale listeners", async () => {
  const stopFirstModels = vi.fn();
  const stopFirstAccount = vi.fn();
  const stopSecondModels = vi.fn();
  const stopSecondAccount = vi.fn();
  let staleModelsListener: (() => void) | undefined;
  let finishFirstModels!: (stop: () => void) => void;
  let finishFirstAccount!: (stop: () => void) => void;

  native.listenModels
    .mockImplementationOnce((listener) => {
      staleModelsListener = listener;
      return new Promise((resolve) => {
        finishFirstModels = resolve;
      });
    })
    .mockResolvedValueOnce(stopSecondModels);
  native.listenAccount
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishFirstAccount = resolve;
        }),
    )
    .mockResolvedValueOnce(stopSecondAccount);

  const settings = new VoiceSettings();
  const first = settings.start();
  settings.dispose();
  const second = settings.start();
  await second;

  expect(native.status).toHaveBeenCalledOnce();

  finishFirstModels(stopFirstModels);
  finishFirstAccount(stopFirstAccount);
  await first;

  expect(stopFirstModels).toHaveBeenCalledOnce();
  expect(stopFirstAccount).toHaveBeenCalledOnce();
  expect(stopSecondModels).not.toHaveBeenCalled();
  expect(stopSecondAccount).not.toHaveBeenCalled();
  expect(native.status).toHaveBeenCalledOnce();

  staleModelsListener?.();
  await Promise.resolve();
  expect(native.status).toHaveBeenCalledOnce();

  settings.dispose();
  expect(stopSecondModels).toHaveBeenCalledOnce();
  expect(stopSecondAccount).toHaveBeenCalledOnce();
});
