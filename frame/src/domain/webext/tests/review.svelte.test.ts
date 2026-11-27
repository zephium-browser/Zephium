import { beforeEach, expect, test, vi } from "vitest";
import type { WebExtensionAccessRequestView, WebExtensionReview } from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({
  prepare: vi.fn(),
  confirm: vi.fn(),
  cancel: vi.fn(),
  list: vi.fn(),
  answer: vi.fn(),
  accessListen: vi.fn(),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    webExtensionPrepare: native.prepare,
    webExtensionConfirm: native.confirm,
    webExtensionCancel: native.cancel,
    webExtensionList: native.list,
    webExtensionAnswerAccess: native.answer,
  });
});

vi.mock("$shared/ipc/native-events", () => ({
  events: { webExtensionAccessRequested: { listen: native.accessListen } },
}));

function review(reviewId: string): WebExtensionReview {
  return {
    review_id: reviewId,
    id: "same-extension",
    name: "Fixture",
    version: "1.0",
    description: "",
    warnings: [],
    limitations: [],
    icon: null,
    update: false,
    from_file: false,
  };
}

beforeEach(() => {
  vi.resetModules();
  vi.resetAllMocks();
  native.confirm.mockResolvedValue({ status: "ok", data: null });
  native.cancel.mockResolvedValue(undefined);
  native.list.mockResolvedValue({ status: "ok", data: [] });
});

test("confirmation identifies the exact displayed review", async () => {
  const webext = await import("../webext.svelte");
  native.prepare.mockResolvedValue({ status: "ok", data: review("review-a") });
  await webext.prepare("tab");
  await webext.confirm("review-a");
  expect(native.confirm).toHaveBeenCalledExactlyOnceWith("same-extension", "review-a");
});

test("a stale displayed review cannot confirm or cancel a newer review of the same extension", async () => {
  const webext = await import("../webext.svelte");
  native.prepare.mockResolvedValue({ status: "ok", data: review("review-b") });
  await webext.prepare("tab");
  await webext.confirm("review-a");
  webext.cancel("review-a");
  expect(native.confirm).not.toHaveBeenCalled();
  expect(native.cancel).not.toHaveBeenCalled();
  expect(webext.review()?.review_id).toBe("review-b");
});

test("an earlier confirmation cannot discard a newer review when it settles", async () => {
  const webext = await import("../webext.svelte");
  native.prepare.mockResolvedValueOnce({ status: "ok", data: review("review-a") });
  await webext.prepare("tab");
  let complete!: (value: { status: "ok"; data: null }) => void;
  native.confirm.mockReturnValueOnce(new Promise((resolve) => (complete = resolve)));
  const confirming = webext.confirm("review-a");
  native.prepare.mockResolvedValueOnce({ status: "ok", data: review("review-b") });
  await webext.prepare("tab");
  complete({ status: "ok", data: null });
  await confirming;
  expect(webext.review()?.review_id).toBe("review-b");
});

test("cancellation identifies the exact current review", async () => {
  const webext = await import("../webext.svelte");
  native.prepare.mockResolvedValueOnce({ status: "ok", data: review("review-a") });
  await webext.prepare("tab");
  webext.cancel("review-a");
  expect(native.cancel).toHaveBeenCalledExactlyOnceWith("review-a");
  expect(webext.review()).toBeNull();
});

test("a newly prepared review clears errors from the replaced review", async () => {
  const webext = await import("../webext.svelte");
  native.prepare.mockResolvedValueOnce({ status: "ok", data: review("review-a") });
  await webext.prepare("tab");
  let prepared!: (value: unknown) => void;
  native.prepare.mockReturnValueOnce(new Promise((resolve) => (prepared = resolve)));
  const preparing = webext.prepare("another-tab");
  native.confirm.mockResolvedValueOnce({ status: "error", error: "The old review changed." });
  await webext.confirm("review-a");
  expect(webext.error()).toBe("The old review changed.");
  prepared({ status: "ok", data: review("review-b") });
  await preparing;
  expect(webext.review()?.review_id).toBe("review-b");
  expect(webext.error()).toBeNull();
});

test.each(["result", "rejection"])(
  "a failed access answer (%s) keeps the prompt for retry or denial",
  async (failure) => {
    const webext = await import("../webext.svelte");
    let receive!: (event: { payload: WebExtensionAccessRequestView }) => void;
    native.accessListen.mockImplementation((listener) => {
      receive = listener;
      return Promise.resolve(() => {});
    });
    await webext.listenForAccess();
    const first: WebExtensionAccessRequestView = {
      profile_id: "profile",
      request: "1",
      extension_id: "extension",
      warnings: [],
      permissions: ["bookmarks"],
      patterns: [],
    };
    const second = { ...first, request: "2" };
    receive({ payload: first });
    receive({ payload: second });
    if (failure === "result")
      native.answer.mockResolvedValueOnce({ status: "error", error: "Could not save access." });
    else native.answer.mockRejectedValueOnce("Could not save access.");
    await webext.answerAccess(true);
    expect(webext.accessRequest()).toEqual(first);
    expect(webext.accessError()).toBe("Could not save access.");
    expect(webext.isAnswering()).toBe(false);
    native.answer.mockResolvedValueOnce({ status: "ok", data: null });
    await webext.answerAccess(false);
    expect(native.answer).toHaveBeenLastCalledWith(first, false);
    expect(webext.accessRequest()).toEqual(second);
    expect(webext.accessError()).toBeNull();
  },
);

test("access approval can be retried after a failed save", async () => {
  const webext = await import("../webext.svelte");
  const request: WebExtensionAccessRequestView = {
    profile_id: "profile",
    request: "1",
    extension_id: "extension",
    warnings: [],
    permissions: [],
    patterns: [],
  };
  native.accessListen.mockImplementation((listener) => {
    listener({ payload: request });
    return Promise.resolve(() => {});
  });
  await webext.listenForAccess();
  native.answer.mockResolvedValueOnce({ status: "error", error: "Could not save access." });
  await webext.answerAccess(true);
  native.answer.mockResolvedValueOnce({ status: "ok", data: null });
  await webext.answerAccess(true);
  expect(native.answer).toHaveBeenCalledTimes(2);
  expect(native.answer).toHaveBeenLastCalledWith(request, true);
  expect(webext.accessRequest()).toBeNull();
  expect(webext.accessError()).toBeNull();
});
