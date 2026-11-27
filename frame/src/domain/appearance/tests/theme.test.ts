import type { UiInfo } from "$shared/ipc/bindings";
import { beforeEach, describe, expect, it, vi } from "vitest";

const harness = vi.hoisted(() => {
  let commandListener: ((event: { payload: string }) => void) | undefined;

  return {
    order: [] as string[],
    attributes: new Map<string, string>(),
    media: {
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    },
    stop: vi.fn(),
    listen: vi.fn((listener: (event: { payload: string }) => void) => {
      harness.order.push("listen");
      commandListener = listener;
      return Promise.resolve(harness.stop);
    }),
    emit(command: string) {
      commandListener?.({ payload: command });
    },
    uiInfo: vi.fn(async (): Promise<UiInfo> => {
      harness.order.push("uiInfo");
      return { material: "vibrancy" as const };
    }),
    settingGet: vi.fn(async () => {
      harness.order.push("settingGet");
      return "light";
    }),
  };
});

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    uiInfo: harness.uiInfo,
    settingGet: harness.settingGet,
  });
});

vi.mock("$shared/ipc/native-events", () => ({
  events: {
    uiCommand: {
      listen: harness.listen,
    },
  },
}));

describe("theme startup lifecycle", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    harness.order.length = 0;
    harness.attributes.clear();
    harness.media.matches = false;
    harness.uiInfo.mockImplementation(async () => {
      harness.order.push("uiInfo");
      return { material: "vibrancy" as const };
    });
    harness.settingGet.mockImplementation(async () => {
      harness.order.push("settingGet");
      return "light";
    });

    vi.stubGlobal("navigator", { userAgent: "Mac" });
    vi.stubGlobal("window", {
      matchMedia: vi.fn(() => harness.media),
    });
    vi.stubGlobal("document", {
      documentElement: {
        setAttribute(name: string, value: string) {
          harness.attributes.set(name, value);
          harness.order.push(`${name}:${value}`);
        },
      },
    });
  });

  it("applies a deterministic theme and subscribes before native queries", async () => {
    const theme = await import("../theme");
    const first = theme.init();
    const second = theme.init();

    expect(first).toBe(second);
    expect(harness.order.slice(0, 3)).toEqual(["data-theme:dark", "listen", "uiInfo"]);

    await first;

    expect(harness.attributes.get("data-material")).toBe("vibrancy");
    expect(harness.attributes.get("data-theme")).toBe("light");
    expect(harness.media.addEventListener).toHaveBeenCalledOnce();
    expect(harness.listen).toHaveBeenCalledOnce();

    theme.dispose();
    theme.dispose();
    expect(harness.stop).toHaveBeenCalledOnce();
    expect(harness.media.removeEventListener).toHaveBeenCalledOnce();
  });

  it("does not let a stale stored preference overwrite a live theme command", async () => {
    let resolveStored: ((value: string) => void) | undefined;
    harness.settingGet.mockImplementation(
      () =>
        new Promise((resolve) => {
          harness.order.push("settingGet");
          resolveStored = resolve;
        }),
    );

    const theme = await import("../theme");
    const ready = theme.init();
    await vi.waitFor(() => expect(resolveStored).toBeTypeOf("function"));

    harness.emit("theme.dark");
    resolveStored?.("light");
    await ready;

    expect(harness.attributes.get("data-theme")).toBe("dark");
    theme.dispose();
  });

  it("keeps a newer native material update when the startup query settles late", async () => {
    let resolveInfo: ((value: UiInfo) => void) | undefined;
    harness.uiInfo.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveInfo = resolve;
        }),
    );
    const theme = await import("../theme");
    const ready = theme.init();
    harness.emit("material.none");
    resolveInfo?.({ material: "liquid_glass" });
    await ready;
    expect(harness.attributes.get("data-material")).toBe("none");
    harness.emit("material.liquid_glass");
    expect(harness.attributes.get("data-material")).toBe("liquid_glass");
    harness.emit("material.mica_alt");
    expect(harness.attributes.get("data-material")).toBe("mica_alt");
    harness.emit("material.untrusted-value");
    expect(harness.attributes.get("data-material")).toBe("mica_alt");
    theme.dispose();
    harness.emit("material.none");
    expect(harness.attributes.get("data-material")).toBe("mica_alt");
  });

  it("removes a listener that resolves after disposal", async () => {
    let resolveInfo: ((value: UiInfo) => void) | undefined;
    harness.uiInfo.mockImplementation(
      () =>
        new Promise((resolve) => {
          harness.order.push("uiInfo");
          resolveInfo = resolve;
        }),
    );

    const theme = await import("../theme");
    const ready = theme.init();
    theme.dispose();
    resolveInfo?.({ material: "vibrancy" as const });
    await ready;

    expect(harness.stop).toHaveBeenCalledOnce();
    expect(harness.media.addEventListener).not.toHaveBeenCalled();
  });
});
