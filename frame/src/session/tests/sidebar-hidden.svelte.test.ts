import { afterEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  hidden: vi.fn(async (_hidden: boolean) => undefined),
  overlay: vi.fn(async (_raised: boolean, _width: number | null): Promise<boolean | null> => true),
  width: vi.fn(async (_width: number | null, _animate: boolean, _revision: number | null) => {}),
  save: vi.fn(async (_key: string, _value: string) => ({ operation_id: null, accepted: true })),
}));

vi.mock("$shared/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarSetHidden: native.hidden,
    sidebarOverlay: native.overlay,
    sidebarSetWidth: native.width,
    settingSet: native.save,
    settingGet: vi.fn(async () => null),
  });
});

const {
  adoptResizeWidth,
  expanded,
  hideFromDrag,
  holdHidden,
  isHidden,
  isPeeking,
  peek,
  sidebarResizeRevision,
  toggleHidden,
  unpeek,
} = await import("../sidebar-mode.svelte");

afterEach(() => {
  if (isHidden()) toggleHidden();
  holdHidden(false);
  vi.useRealTimers();
  vi.clearAllMocks();
});

it("hides through native and saves the choice", () => {
  toggleHidden();
  expect(isHidden()).toBe(true);
  expect(native.hidden).toHaveBeenLastCalledWith(true);
  expect(native.save).toHaveBeenLastCalledWith("sidebar.hidden", "true");
});

it("brings the column back while a panel needs it beside the page", () => {
  toggleHidden();
  holdHidden(true);
  expect(isHidden()).toBe(false);
  expect(native.hidden).toHaveBeenLastCalledWith(false);
  holdHidden(false);
  expect(native.hidden).toHaveBeenLastCalledWith(true);
});

it("floats over the page only once native has raised it", async () => {
  await peek();
  expect(isPeeking()).toBe(false);
  toggleHidden();
  native.overlay.mockResolvedValueOnce(null);
  await peek();
  expect(isPeeking()).toBe(false);
  await peek();
  expect(isPeeking()).toBe(true);
  // Native measures the floating column from its width, not the chrome's.
  expect(native.overlay).toHaveBeenLastCalledWith(true, expanded());
});

it("returns beneath the page after it has slid away", async () => {
  vi.useFakeTimers();
  toggleHidden();
  await peek();
  unpeek(200);
  expect(isPeeking()).toBe(false);
  expect(native.overlay.mock.calls.some(([raised]) => !raised)).toBe(false);
  vi.advanceTimersByTime(200);
  expect(native.overlay.mock.calls.at(-1)?.[0]).toBe(false);
});

it("a peek that comes back before the column has gone stays raised", async () => {
  vi.useFakeTimers();
  toggleHidden();
  await peek();
  unpeek(200);
  await peek();
  vi.advanceTimersByTime(200);
  expect(isPeeking()).toBe(true);
  expect(native.overlay.mock.calls.some(([raised]) => !raised)).toBe(false);
});

it("showing the column again ends a peek", async () => {
  toggleHidden();
  await peek();
  toggleHidden();
  expect(isPeeking()).toBe(false);
  expect(native.hidden).toHaveBeenLastCalledWith(false);
});

it("a drag released past the narrowest hides the column at the width it had", () => {
  adoptResizeWidth(300);
  // The live drag left the column at its narrowest before release.
  adoptResizeWidth(180);
  hideFromDrag(300, sidebarResizeRevision());
  expect(isHidden()).toBe(true);
  expect(native.hidden).toHaveBeenLastCalledWith(true);
  expect(native.save).toHaveBeenLastCalledWith("sidebar.hidden", "true");
  expect(expanded()).toBe(300);
  expect(native.width.mock.calls.at(-1)?.slice(0, 2)).toEqual([300, false]);
});

it("ignores a hide from a drag a newer width has overtaken", () => {
  hideFromDrag(300, sidebarResizeRevision() - 1);
  expect(isHidden()).toBe(false);
  expect(native.hidden).not.toHaveBeenCalled();
});

it("slides in only once raised over the page", async () => {
  toggleHidden();
  let raise: (onSide: boolean | null) => void = () => {};
  native.overlay.mockReturnValueOnce(new Promise((resolve) => (raise = resolve)));
  const peeking = peek();
  expect(isPeeking()).toBe(false);
  raise(true);
  await peeking;
  expect(isPeeking()).toBe(true);
});

it("a floating column dragged past its narrowest stays up", async () => {
  toggleHidden();
  await peek();
  native.width.mockClear();
  hideFromDrag(300, sidebarResizeRevision());
  expect(isHidden()).toBe(true);
  expect(isPeeking()).toBe(true);
  expect(native.width).not.toHaveBeenCalled();
});

it("tells whether the pointer is still on the column once it is up", async () => {
  toggleHidden();
  // The pointer left while the column was rising: nothing else will say so.
  native.overlay.mockResolvedValueOnce(false);
  expect(await peek()).toBe(false);
  expect(isPeeking()).toBe(true);
});
