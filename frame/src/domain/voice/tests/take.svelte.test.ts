import { beforeEach, expect, test, vi } from "vitest";
import type { VoiceChanged, VoiceLevels, VoiceStartV1 } from "$shared/ipc/bindings";
import { VoiceTake } from "../take.svelte";

const native = vi.hoisted(() => ({
  start: vi.fn<(...args: unknown[]) => Promise<VoiceStartV1>>(),
  cancel: vi.fn<(...args: unknown[]) => Promise<boolean>>(),
  stop: vi.fn<(take: number) => Promise<boolean>>(),
  changed: (_event: { payload: VoiceChanged }) => {},
  levels: (_event: { payload: VoiceLevels }) => {},
  listenChanged:
    vi.fn<(listener: (event: { payload: VoiceChanged }) => void) => Promise<() => void>>(),
  listenLevels:
    vi.fn<(listener: (event: { payload: VoiceLevels }) => void) => Promise<() => void>>(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    voiceStart: native.start,
    voiceCancel: native.cancel,
    voiceStop: native.stop,
  });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    voiceChanged: {
      listen: native.listenChanged,
    },
    voiceLevels: {
      listen: native.listenLevels,
    },
  },
}));
beforeEach(() => {
  vi.resetAllMocks();
  native.listenChanged.mockImplementation(async (listener) => {
    native.changed = listener;
    return () => {};
  });
  native.listenLevels.mockImplementation(async (listener) => {
    native.levels = listener;
    return () => {};
  });
});
const result = (take: number): VoiceStartV1 => ({ take, engine: "groq", fault: null });

test("a stale successful start cannot cancel a replacement take", async () => {
  let finish!: (value: VoiceStartV1) => void;
  let current: { request: unknown; take: number } | null = null;
  native.start
    .mockImplementationOnce((request) => {
      current = { request, take: 1 };
      return new Promise((resolve) => {
        finish = resolve;
      });
    })
    .mockImplementationOnce(async (request) => {
      current = { request, take: 2 };
      return result(2);
    });
  native.cancel.mockImplementation(async (request, take) => {
    if (
      current &&
      (request === undefined ||
        (current.request === request && (take == null || current.take === take)))
    ) {
      current = null;
      return true;
    }
    return false;
  });
  const voice = new VoiceTake(vi.fn());
  const first = voice.start();
  await vi.waitFor(() => expect(native.start).toHaveBeenCalledOnce());
  voice.cancel();
  await voice.start();
  finish(result(1));
  await first;
  expect(current).toMatchObject({ take: 2 });
  expect(voice.phase).toBe("listening");
  voice.dispose();
});

test("early changes from an old request cannot attach to a new start", async () => {
  let finishFirst!: (value: VoiceStartV1) => void;
  let finishSecond!: (value: VoiceStartV1) => void;
  native.start
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishFirst = resolve;
        }),
    )
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishSecond = resolve;
        }),
    );
  native.cancel.mockResolvedValue(true);
  const voice = new VoiceTake(vi.fn());
  const first = voice.start();
  await vi.waitFor(() => expect(native.start).toHaveBeenCalledOnce());
  const oldRequest = String(native.start.mock.calls[0]?.[0] ?? "old");
  voice.cancel();
  const second = voice.start();
  await vi.waitFor(() => expect(native.start).toHaveBeenCalledTimes(2));
  native.changed({
    payload: {
      request: oldRequest,
      take: 1,
      phase: "listening",
      engine: "groq",
      partial: "old words",
      text: null,
      fault: null,
    },
  });
  expect(voice.phase).toBe("starting");
  expect(voice.partial).toBe("");
  const request = String(native.start.mock.calls[1]?.[0]);
  native.changed({
    payload: {
      request,
      take: 2,
      phase: "listening",
      engine: "groq",
      partial: "new words",
      text: null,
      fault: null,
    },
  });
  expect(voice.phase).toBe("listening");
  expect(voice.partial).toBe("new words");
  const levels = vi.fn();
  voice.onLevels(levels);
  native.levels({ payload: { request: oldRequest, take: 2, levels: [1] } });
  expect(levels).not.toHaveBeenCalled();
  native.levels({ payload: { request, take: 2, levels: [2] } });
  expect(levels).toHaveBeenCalledWith([2]);
  finishSecond(result(2));
  await second;
  finishFirst(result(1));
  await first;
  voice.dispose();
});

test("an early terminal event settles once before the start response arrives", async () => {
  let finish!: (value: VoiceStartV1) => void;
  native.start.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  native.cancel.mockResolvedValue(true);
  const commit = vi.fn();
  const voice = new VoiceTake(commit);
  const starting = voice.start();
  await vi.waitFor(() => expect(native.start).toHaveBeenCalledOnce());
  const request = String(native.start.mock.calls[0]?.[0]);
  native.changed({
    payload: {
      request,
      take: 9,
      phase: "done",
      engine: "groq",
      partial: null,
      text: " Words ",
      fault: null,
    },
  });
  expect(commit).toHaveBeenCalledExactlyOnceWith("Words");
  finish(result(9));
  await starting;
  expect(voice.phase).toBe("idle");
  expect(native.cancel).toHaveBeenCalledWith(request, 9);
  voice.dispose();
});

test("a previous stop failure cannot settle a replacement request", async () => {
  let reject!: (error: Error) => void;
  native.stop.mockImplementationOnce(
    () =>
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
  );
  native.cancel.mockResolvedValue(true);
  native.start.mockResolvedValueOnce(result(1)).mockResolvedValueOnce(result(2));
  const voice = new VoiceTake(vi.fn());
  await voice.start();
  voice.stop();
  voice.cancel();
  await voice.start();
  reject(new Error("old transport failed"));
  await Promise.resolve();
  await Promise.resolve();
  expect(voice.phase).toBe("listening");
  voice.dispose();
});

test("listener refusal releases partial subscriptions and leaves a retryable failure", async () => {
  const stop = vi.fn();
  let registered!: (stop: () => void) => void;
  native.listenChanged.mockReturnValueOnce(
    new Promise((resolve) => {
      registered = resolve;
    }),
  );
  native.listenLevels.mockRejectedValueOnce(new Error("registration failed"));
  native.start.mockResolvedValue(result(3));
  native.cancel.mockResolvedValue(true);
  const voice = new VoiceTake(vi.fn());
  const starting = voice.start();
  const settled = expect(starting).resolves.toBeUndefined();
  registered(stop);
  await settled;
  expect(stop).toHaveBeenCalledOnce();
  expect(native.start).not.toHaveBeenCalled();
  expect(voice.phase).toBe("failed");
  expect(voice.problem).toBe("unavailable");
  await voice.start();
  expect(native.listenChanged).toHaveBeenCalledTimes(2);
  expect(native.listenLevels).toHaveBeenCalledTimes(2);
  expect(voice.phase).toBe("listening");
  voice.dispose();
});
