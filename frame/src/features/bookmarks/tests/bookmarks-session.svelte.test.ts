import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BookmarkResponse, BookmarkView } from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({ call: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ bookmarkCall: native.call });
});

const view = (id: string, title: string, url: string | null = null): BookmarkView => ({
  id,
  title,
  url,
  icon: null,
  children: 0,
  parent: null,
});
const listing = (folder: string | null, items: BookmarkView[]): BookmarkResponse => ({
  kind: "listing",
  folder,
  path: folder ? [{ id: folder, title: "Folder" }] : [],
  items,
});

beforeEach(() => {
  vi.resetAllMocks();
  vi.useRealTimers();
});

describe("bookmarks session", () => {
  it("ignores a prior search reply as soon as the query changes", async () => {
    vi.useFakeTimers();
    const { BookmarksSession } = await import("../lib/bookmarks-session.svelte");
    const session = new BookmarksSession("profile");
    let resolve!: (value: BookmarkResponse) => void;
    native.call.mockReturnValueOnce(new Promise((done) => (resolve = done)));
    session.search("old");
    await vi.advanceTimersByTimeAsync(200);
    session.search("new");
    resolve({ kind: "results", items: [view("1", "Old result")] });
    await vi.advanceTimersByTimeAsync(0);
    expect(session.items).toEqual([]);
    native.call.mockResolvedValueOnce({ kind: "results", items: [view("2", "New result")] });
    await vi.advanceTimersByTimeAsync(200);
    expect(session.items.map((item) => item.id)).toEqual(["2"]);
  });

  it("opens folders and keeps only the newest read", async () => {
    const { BookmarksSession } = await import("../lib/bookmarks-session.svelte");
    const session = new BookmarksSession("profile");
    let slow!: (value: BookmarkResponse) => void;
    native.call
      .mockReturnValueOnce(new Promise((resolve) => (slow = resolve)))
      .mockResolvedValueOnce(listing("7", [view("8", "Inside", "https://in.example/")]));
    const older = session.open(null);
    await session.open("7");
    slow(listing(null, [view("1", "Stale")]));
    await older;
    expect(session.folder).toBe("7");
    expect(session.items.map((item) => item.title)).toEqual(["Inside"]);
    expect(native.call).toHaveBeenLastCalledWith("profile", { kind: "list", folder: "7" });
  });

  it("falls back to the top level when the open folder disappears", async () => {
    const { BookmarksSession } = await import("../lib/bookmarks-session.svelte");
    const session = new BookmarksSession("profile");
    native.call
      .mockResolvedValueOnce({ kind: "error", error: "missing" })
      .mockResolvedValueOnce(listing(null, [view("1", "Top")]));
    await session.open("gone");
    await vi.waitFor(() => expect(session.items.map((item) => item.title)).toEqual(["Top"]));
    expect(session.folder).toBeNull();
    expect(session.error).toBeNull();
  });

  it("reads the view again after a write, and reports a refused one", async () => {
    const { BookmarksSession } = await import("../lib/bookmarks-session.svelte");
    const session = new BookmarksSession("profile");
    native.call.mockResolvedValueOnce(listing(null, []));
    await session.open(null);
    native.call
      .mockResolvedValueOnce({ kind: "saved", id: "4" })
      .mockResolvedValueOnce(listing(null, [view("4", "New folder")]));
    expect(await session.addFolder("New folder")).toBe("4");
    expect(session.highlighted).toBe("4");
    expect(session.items.map((item) => item.id)).toEqual(["4"]);
    native.call.mockResolvedValueOnce({ kind: "error", error: "cycle" });
    expect(await session.move("4", "4", 0)).toBe(false);
    expect(session.error).toBe("cycle");
  });

  it("searches after a pause and returns to the folder when cleared", async () => {
    vi.useFakeTimers();
    const { BookmarksSession } = await import("../lib/bookmarks-session.svelte");
    const session = new BookmarksSession("profile");
    native.call.mockResolvedValue({ kind: "results", items: [view("2", "Docs", "https://d/")] });
    session.search("do");
    session.search("doc");
    await vi.advanceTimersByTimeAsync(200);
    expect(native.call).toHaveBeenCalledOnce();
    expect(native.call).toHaveBeenLastCalledWith("profile", { kind: "search", query: "doc" });
    native.call.mockResolvedValue(listing(null, []));
    session.search("");
    await vi.advanceTimersByTimeAsync(0);
    expect(native.call).toHaveBeenLastCalledWith("profile", { kind: "list", folder: null });
  });
});
