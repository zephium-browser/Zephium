import { afterEach, expect, test, vi } from "vitest";
import type { ZoomCall, ZoomResponse } from "$shared/ipc/bindings";
import { ZoomLevelsSession } from "$domain/zoom";

const native = vi.hoisted(() => ({
  call: vi.fn<(profile: string, call: ZoomCall) => Promise<ZoomResponse>>(),
}));
vi.mock("$shared/ipc/bindings", () => ({ commands: { zoomCall: native.call } }));

afterEach(() => vi.resetAllMocks());

test.each(["forget", "forget_all"] as const)(
  "a failed %s refreshes successful partial changes and keeps the failure visible",
  async (kind) => {
    const kept = { site: "kept.example", percent: 150 };
    native.call
      .mockResolvedValueOnce({
        kind: "levels",
        levels: [kept, { site: "gone.example", percent: 125 }],
      })
      .mockResolvedValueOnce({ kind: "error", error: "unavailable" })
      .mockResolvedValueOnce({ kind: "levels", levels: [kept] });
    const session = new ZoomLevelsSession("profile");
    await session.load();
    const success = await (kind === "forget"
      ? session.forget("gone.example")
      : session.forgetAll());
    expect(success).toBe(false);
    expect(session.unavailable).toBe(true);
    expect(session.busy).toBe(false);
    expect(session.levels).toEqual([kept]);
    expect(native.call).toHaveBeenLastCalledWith("profile", { kind: "levels" });
    session.dispose();
  },
);
