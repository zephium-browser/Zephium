import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSearchController, type SearchSnapshot } from "../lib/search-controller";
import type { SearchResult } from "$shared/ipc/bindings";

const owner = { window_id: "w", profile_id: "p", space_id: "s", session_id: "0000000000000001" };
const row = (title: string): SearchResult => ({
  kind: "search",
  title,
  detail: "DuckDuckGo",
  icon: null,
  action: { type: "OpenUrl", url: `https://duckduckgo.com/?q=${title}` },
});

describe("search request lifecycle", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("coalesces typing and makes no request during IME composition", async () => {
    const send = vi.fn(async () => true);
    const controller = createSearchController({
      owner,
      send,
      update: () => {},
      id: () => "request",
    });
    controller.compositionStart();
    controller.change("に");
    await vi.advanceTimersByTimeAsync(100);
    expect(send).not.toHaveBeenCalled();
    controller.compositionEnd("日本");
    controller.change("日本語");
    await vi.advanceTimersByTimeAsync(60);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith("日本語", "request");
    controller.dispose();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("answers the first character at once, then folds a burst into one request", async () => {
    const send = vi.fn(async () => true);
    let id = 0;
    const controller = createSearchController({
      owner,
      send,
      update: () => {},
      id: () => String(++id),
    });
    controller.change("r");
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    // A fast burst: each keystroke replaces the last, and only the final text
    // crosses IPC. Every request costs an actor pass, a history read, a note
    // read and a network observation, so this is the difference between one
    // and five of each.
    for (const value of ["ru", "rus", "rust", "rust "]) {
      controller.change(value);
      await vi.advanceTimersByTimeAsync(20);
    }
    await vi.advanceTimersByTimeAsync(60);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenLastCalledWith("rust ", "2");
    controller.dispose();
  });

  it("keeps the previous rows visible but unsettled while a replacement is in flight", async () => {
    const snapshots: SearchSnapshot[] = [];
    let id = 0;
    const controller = createSearchController({
      owner,
      send: async () => true,
      update: (state) => snapshots.push(state),
      id: () => String(++id),
    });
    controller.change("rust");
    await vi.advanceTimersByTimeAsync(0);
    controller.receive({
      context: { ...owner, request_id: "1" },
      query: "rust",
      completion: null,
      pending: false,
      results: [row("rust")],
    });
    expect(snapshots.at(-1)).toMatchObject({ settled: true, pending: false });
    expect(controller.context()).not.toBeNull();

    controller.change("rust b");
    // Rows stay on screen so the list does not blink, but they no longer
    // answer the question being asked, so the surface may not execute them.
    expect(snapshots.at(-1)?.results).toHaveLength(1);
    expect(snapshots.at(-1)?.settled).toBe(false);
    // Emphasis keeps measuring against the query the rows answer, so a row's
    // bold match cannot drop out for the moment before the next answer lands.
    expect(snapshots.at(-1)?.answered).toBe("rust");
    expect(controller.context()).toBeNull();
    controller.dispose();
  });

  it("starts from the rows the host is already showing, unsettled", async () => {
    const snapshots: SearchSnapshot[] = [];
    const controller = createSearchController({
      owner,
      send: async () => true,
      update: (state) => snapshots.push(state),
      shown: { results: [row("rust")], answered: "rust" },
      id: () => "request",
    });
    controller.start();
    expect(snapshots.at(0)).toMatchObject({
      results: [row("rust")],
      answered: "rust",
      pending: true,
      settled: false,
    });
    expect(controller.context()).toBeNull();
    controller.dispose();
  });

  it("retains rows through an empty intermediate response and settles only at the end", async () => {
    const snapshots: SearchSnapshot[] = [];
    const controller = createSearchController({
      owner,
      send: async () => true,
      update: (state) => snapshots.push(state),
      id: () => "request",
    });
    controller.change("rust");
    await vi.advanceTimersByTimeAsync(0);
    const context = { ...owner, request_id: "request" };
    controller.receive({ context, query: "rust", completion: null, pending: true, results: [] });
    expect(snapshots.at(-1)).toMatchObject({ pending: true, settled: false });
    controller.receive({
      context,
      query: "rust",
      completion: "rust-lang.org",
      pending: false,
      results: [row("rust")],
    });
    expect(snapshots.at(-1)).toMatchObject({
      pending: false,
      settled: true,
      completion: "rust-lang.org",
      answered: "rust",
    });
    controller.dispose();
  });

  it("never asks native about an empty field when the host shows nothing for one", async () => {
    const send = vi.fn(async () => true);
    const controller = createSearchController({
      owner,
      send,
      update: () => {},
      emptyQuery: "skip",
      id: () => "request",
    });
    controller.change("rust");
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    controller.change("");
    await vi.advanceTimersByTimeAsync(60);
    expect(send).toHaveBeenCalledTimes(1);
    controller.dispose();
  });

  it("cancels all owned timers when hidden and ignores a late response", async () => {
    const update = vi.fn();
    const send = vi.fn(async () => true);
    const controller = createSearchController({ owner, send, update, id: () => "request" });
    controller.start();
    await Promise.resolve();
    controller.dispose();
    update.mockClear();
    controller.receive({
      context: { ...owner, request_id: "request" },
      query: "",
      completion: null,
      pending: false,
      results: [],
    });
    await vi.runAllTimersAsync();
    expect(update).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not let an old request failure replace newer results", async () => {
    let reject!: (error: Error) => void;
    const snapshots: SearchSnapshot[] = [];
    let id = 0;
    const send = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, fail) => {
            reject = fail;
          }),
      )
      .mockResolvedValue(true);
    const controller = createSearchController({
      owner,
      send,
      update: (state) => snapshots.push(state),
      id: () => String(++id),
    });
    controller.start();
    controller.change("new");
    await vi.advanceTimersByTimeAsync(60);
    controller.receive({
      context: { ...owner, request_id: "2" },
      query: "new",
      completion: null,
      pending: false,
      results: [],
    });
    reject(new Error("old failure"));
    await Promise.resolve();
    expect(snapshots.at(-1)?.error).toBe("none");
    controller.dispose();
  });

  it("asks again once before reporting a refused search", async () => {
    const snapshots: SearchSnapshot[] = [];
    const send = vi.fn().mockResolvedValueOnce(false).mockResolvedValue(true);
    const controller = createSearchController({
      owner,
      send,
      update: (state) => snapshots.push(state),
      id: () => "request",
    });
    controller.change("g");
    await vi.advanceTimersByTimeAsync(200);
    expect(send).toHaveBeenCalledTimes(2);
    expect(snapshots.some((state) => state.error === "failed")).toBe(false);

    send.mockResolvedValue(false);
    controller.change("go");
    await vi.advanceTimersByTimeAsync(400);
    expect(send).toHaveBeenCalledTimes(4);
    expect(snapshots.at(-1)?.error).toBe("failed");
    controller.dispose();
  });

  it("bounds the UTF-8 request before crossing IPC", () => {
    const send = vi.fn(async () => true);
    const update = vi.fn();
    const controller = createSearchController({ owner, send, update });
    controller.change("界".repeat(1000));
    vi.advanceTimersByTime(60);
    expect(send).not.toHaveBeenCalled();
    expect(update).toHaveBeenLastCalledWith({
      results: [],
      completion: null,
      pending: false,
      settled: false,
      answered: "",
      error: "too_long",
    });
    controller.dispose();
  });
});
