import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { BlockerSiteContext } from "$shared/ipc/bindings";

const context: BlockerSiteContext = {
  profile: "profile",
  tab: "tab",
  site: "example.com",
  revision: "0000000000000001",
};
const native = vi.hoisted(() => ({
  site: true,
  reload: vi.fn(async (_tab: string) => ({ accepted: true, operation_id: null })),
  change: vi.fn(),
}));

vi.mock("../blocker.svelte", () => ({
  status: () => ({ site: native.site ? { context } : null }),
  boundedIpc: async <T>(request: Promise<T>) => request,
  settleMutation: async (dispatch: () => Promise<unknown>) => {
    await dispatch();
    return native.change();
  },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    blockerSiteChange: async () => ({ accepted: true, operation_id: "op" }),
    tabsReload: native.reload,
  });
});

const { setProtected } = await import("../site-actions");

beforeEach(() => {
  native.site = true;
  native.change.mockResolvedValue({
    state: "processed",
    disposition: { outcome: "applied" },
  });
});
afterEach(() => vi.clearAllMocks());

it("reloads the page once a pause or resume has been applied", async () => {
  expect(await setProtected(false)).toBe(true);
  expect(native.reload).toHaveBeenCalledExactlyOnceWith("tab");
});

it("counts a change that was already in place as done", async () => {
  native.change.mockResolvedValue({ state: "processed", disposition: { outcome: "no_op" } });
  expect(await setProtected(true)).toBe(true);
  expect(native.reload).toHaveBeenCalledOnce();
});

it("leaves the page alone when the change did not go through", async () => {
  native.change.mockResolvedValue({ state: "processed", disposition: { outcome: "rejected" } });
  expect(await setProtected(false)).toBe(false);
  native.change.mockResolvedValue({ state: "unavailable" });
  expect(await setProtected(false)).toBe(false);
  expect(native.reload).not.toHaveBeenCalled();
});

it("has nothing to change without a site in front", async () => {
  native.site = false;
  expect(await setProtected(false)).toBe(false);
  expect(native.change).not.toHaveBeenCalled();
});
