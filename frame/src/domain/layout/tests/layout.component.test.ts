import { afterEach, describe, expect, it, vi } from "vitest";
import type { GlanceLayout, GlanceRect } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { layout } from "..";

const glanceAct = vi.fn(async (_action: string) => ({ accepted: true, operation_id: null }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ glanceAct: (action) => glanceAct(action) });
});

const rect = (x: number | null, y: number | null, width: number | null, height: number | null) =>
  ({ x, y, width, height }) satisfies GlanceRect;

const glance = (overrides: Partial<GlanceLayout> = {}): GlanceLayout => ({
  tab: "child",
  parent: "opener",
  region: rect(200, 0, 1000, 800),
  page: rect(300, 0, 800, 800),
  backdrop: null,
  ...overrides,
});

const deliver = (value: GlanceLayout | null) =>
  emitNativeEvent("layoutChanged", { dividers: [], work_pane: null, glance: value });

afterEach(() => {
  layout.dispose();
  glanceAct.mockClear();
});

describe("glance layout", () => {
  it("projects a finite page and backdrop from native layout", async () => {
    await layout.init();
    deliver(glance({ backdrop: "http://zephium-preview.localhost/1/opener?v=2" }));

    expect(layout.glance()).toEqual({
      tab: "child",
      parent: "opener",
      region: { x: 200, y: 0, width: 1000, height: 800 },
      page: { x: 300, y: 0, width: 800, height: 800 },
      backdrop: "http://zephium-preview.localhost/1/opener?v=2",
    });
  });

  it("shows nothing for an absent glance or an unusable rectangle", async () => {
    await layout.init();
    deliver(glance());
    expect(layout.glance()).not.toBeNull();

    deliver(null);
    expect(layout.glance()).toBeNull();

    for (const bad of [
      glance({ region: rect(null, 0, 1000, 800) }),
      glance({ page: rect(300, 0, null, 800) }),
      glance({ page: rect(300, 0, Number.POSITIVE_INFINITY, 800) }),
      glance({ region: rect(Number.NaN, 0, 1000, 800) }),
    ]) {
      deliver(glance());
      deliver(bad);
      expect(layout.glance()).toBeNull();
    }
  });

  it("sends each action and swallows a refused one", async () => {
    for (const action of ["close", "expand", "split"] as const) layout.glanceAct(action);
    expect(glanceAct.mock.calls.map(([action]) => action)).toEqual(["close", "expand", "split"]);

    const unhandled = vi.fn();
    window.addEventListener("unhandledrejection", unhandled);
    glanceAct.mockRejectedValueOnce(new Error("refused"));
    layout.glanceAct("close");
    await new Promise((resolve) => setTimeout(resolve, 0));
    window.removeEventListener("unhandledrejection", unhandled);
    expect(unhandled).not.toHaveBeenCalled();
  });
});
