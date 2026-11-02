import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import type { ProfilesView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import PrivateBar from "../components/PrivateBar.svelte";
import PrivateReturn from "../components/PrivateReturn.svelte";

const native = vi.hoisted(() => ({
  run: vi.fn(async () => ({ accepted: true, operation_id: null })),
  profiles: vi.fn(),
}));

const avatar = { style: "initials" as const, color: 1, symbol: 0, revision: 0 };
const roster = (over: Partial<ProfilesView>): ProfilesView => ({
  current: "p1",
  private: false,
  private_tabs: null,
  profiles: [{ id: "p1", name: "Personal", kind: "default", avatar }],
  ...over,
});

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ runCommand: native.run, profilesGet: native.profiles });
});

beforeEach(() => {
  native.run.mockClear();
  native.profiles.mockReset();
});

const reads = (data: ProfilesView) => native.profiles.mockResolvedValue({ status: "ok", data });

test("the private bar goes back without closing, and closing says it erases", async () => {
  reads(roster({ private: true, private_tabs: 2 }));
  const screen = await render(PrivateBar);

  const back = screen.getByRole("button", { name: "Back to Personal" });
  const close = screen.getByRole("button", { name: "Close private window" });
  await expect.element(back).toBeVisible();
  await expect.element(close).toBeVisible();
  await expect
    .element(back)
    .toHaveAttribute("title", "Return to Personal. Your private tabs stay open.");
  await expect.element(close).toHaveAttribute("title", expect.stringContaining("erases its tabs"));
  await expect.element(screen.getByText(/Going back keeps these tabs/u)).toBeVisible();

  await back.click();
  expect(native.run).toHaveBeenLastCalledWith("window.showRegular");
  await close.click();
  expect(native.run).toHaveBeenLastCalledWith("window.closePrivate");
});

test("at rail width the same two ways out stand as glyphs", async () => {
  reads(roster({ private: true, private_tabs: 1 }));
  const screen = await render(PrivateBar, { compact: true });

  await screen.getByRole("button", { name: "Back to Personal" }).click();
  expect(native.run).toHaveBeenLastCalledWith("window.showRegular");
  await screen.getByRole("button", { name: "Close private window" }).click();
  expect(native.run).toHaveBeenLastCalledWith("window.closePrivate");
});

test("a private window set aside is offered in the regular column with its tab count", async () => {
  reads(roster({ private_tabs: 3 }));
  const screen = await render(PrivateReturn);

  const entry = screen.getByRole("button", {
    name: "Back to private window. Open tabs: 3",
  });
  await expect.element(entry).toBeVisible();
  await expect.element(entry).toHaveTextContent("Back to private window");
  await expect.element(entry).toHaveTextContent("3");
  await entry.click();
  expect(native.run).toHaveBeenLastCalledWith("window.showPrivate");

  // The count follows the roster, and the entry leaves when the window does.
  emitNativeEvent("profilesChanged", roster({ private_tabs: 4 }));
  await expect
    .element(screen.getByRole("button", { name: "Back to private window. Open tabs: 4" }))
    .toBeVisible();
  emitNativeEvent("profilesChanged", roster({ private_tabs: null }));
  await expect.poll(() => screen.container.querySelector("button")).toBeNull();
});

test("nothing is offered while there is no private window or it is in front", async () => {
  reads(roster({ private: false, private_tabs: null }));
  const screen = await render(PrivateReturn);
  await expect.poll(() => native.profiles.mock.calls.length).toBeGreaterThan(0);
  expect(screen.container.querySelector("button")).toBeNull();

  emitNativeEvent("profilesChanged", roster({ private: true, private_tabs: 2 }));
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(screen.container.querySelector("button")).toBeNull();
});

test("at rail width the way back is a glyph named with its count", async () => {
  reads(roster({ private_tabs: 2 }));
  const screen = await render(PrivateReturn, { compact: true });

  await screen.getByRole("button", { name: "Back to private window. Open tabs: 2" }).click();
  expect(native.run).toHaveBeenLastCalledWith("window.showPrivate");
});
