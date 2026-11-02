import { afterEach, describe, expect, it, vi } from "vitest";
import { browserImport, type ImportAdapter, type ImportJob } from "$domain/browser-import";

function adapter(overrides: Partial<ImportAdapter> = {}) {
  let listener: ((job: ImportJob) => void) | null = null;
  const port: ImportAdapter = {
    sources: async () => [
      {
        id: "chrome:default",
        browser: "chrome",
        name: "Google Chrome",
        profiles: [{ id: "Default", name: "Person 1" }],
        kinds: ["bookmarks", "history"],
        needsPermission: false,
        running: false,
      },
    ],
    start: vi.fn(async () => true),
    cancel: vi.fn(async () => {}),
    onProgress: (next) => {
      listener = next;
      return () => (listener = null);
    },
    openPermissionSettings: vi.fn(async () => {}),
    ...overrides,
  };
  return { port, emit: (job: ImportJob) => listener?.(job) };
}

const running = (finished: boolean): ImportJob => ({
  source: "chrome:default",
  profile: "Default",
  kinds: [{ kind: "bookmarks", state: finished ? "done" : "running", done: 12, total: 40 }],
  finished,
  cancelled: false,
});

afterEach(() => browserImport.provide(null));

describe("browser import", () => {
  it("an old start reply cannot release a replacement adapter's pending start", async () => {
    let oldDone!: (accepted: boolean) => void;
    let nextDone!: (accepted: boolean) => void;
    const old = adapter({ start: () => new Promise((resolve) => (oldDone = resolve)) });
    const next = adapter({
      start: vi.fn(() => new Promise<boolean>((resolve) => (nextDone = resolve))),
    });
    browserImport.provide(old.port);
    const oldStart = browserImport.start("chrome", "Default", ["history"]);
    browserImport.provide(next.port);
    const nextStart = browserImport.start("chrome", "Default", ["history"]);
    oldDone(true);
    expect(await oldStart).toBe(false);
    expect(browserImport.busy()).toBe(true);
    expect(await browserImport.start("chrome", "Default", ["history"])).toBe(false);
    expect(next.port.start).toHaveBeenCalledOnce();
    nextDone(true);
    expect(await nextStart).toBe(true);
    expect(browserImport.busy()).toBe(false);
  });

  it("claims nothing until native provides a way to import", async () => {
    browserImport.provide(null);
    expect(browserImport.available()).toBe(false);
    await browserImport.detect();
    expect(browserImport.found()).toBeNull();
    expect(await browserImport.start("chrome:default", "Default", ["history"])).toBe(false);
  });

  it("lists sources once native provides them", async () => {
    browserImport.provide(adapter().port);
    expect(browserImport.available()).toBe(true);
    await browserImport.detect();
    expect(browserImport.found()?.map((source) => source.name)).toEqual(["Google Chrome"]);
  });

  it("follows the job native reports and is busy until it finishes", async () => {
    const { port, emit } = adapter();
    browserImport.provide(port);
    await browserImport.detect();
    expect(await browserImport.start("chrome:default", "Default", ["bookmarks"])).toBe(true);
    emit(running(false));
    expect(browserImport.busy()).toBe(true);
    expect(await browserImport.start("chrome:default", "Default", ["history"])).toBe(false);
    emit(running(true));
    expect(browserImport.busy()).toBe(false);
    expect(browserImport.current()?.kinds[0]?.state).toBe("done");
  });

  it("keeps following progress after looking for sources again", async () => {
    const { port, emit } = adapter();
    browserImport.provide(port);
    await browserImport.detect();
    await browserImport.detect();
    emit(running(false));
    expect(browserImport.current()?.finished).toBe(false);
  });

  it("refuses to start with nothing chosen", async () => {
    const { port } = adapter();
    browserImport.provide(port);
    expect(await browserImport.start("chrome:default", "Default", [])).toBe(false);
    expect(port.start).not.toHaveBeenCalled();
  });

  it("keeps an empty list when native cannot list sources", async () => {
    browserImport.provide(
      adapter({
        sources: async () => {
          throw new Error("unavailable");
        },
      }).port,
    );
    await browserImport.detect();
    expect(browserImport.found()).toEqual([]);
  });
});
