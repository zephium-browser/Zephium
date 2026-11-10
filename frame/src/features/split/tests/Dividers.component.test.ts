import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import { layout } from "$domain/layout";
import { emitNativeEvent } from "$shared/testing/native-events";
import Dividers from "../components/Dividers.svelte";

const native = vi.hoisted(() => ({
  grab: vi.fn(async (_x: number | null, _y: number | null) => {}),
  drag: vi.fn(async (_x: number | null, _y: number | null) => {}),
  release: vi.fn(async (_x: number | null, _y: number | null) => ({
    accepted: true,
    operation_id: null,
  })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    dividerGrab: native.grab,
    dividerDrag: native.drag,
    dividerRelease: native.release,
  });
});

afterEach(async () => {
  await cleanup();
  layout.dispose();
  document.body.style.cursor = "";
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

async function divider() {
  await layout.init();
  emitNativeEvent("layoutChanged", {
    dividers: [{ x: 500, y: 8, width: 8, height: 600, vertical: true }],
    work_pane: null,
  });
  const screen = await render(Dividers);
  const handle = screen.getByRole("slider", { name: "Split position" }).element();
  vi.spyOn(handle, "setPointerCapture").mockImplementation(() => {});
  vi.spyOn(handle, "hasPointerCapture").mockReturnValue(true);
  vi.spyOn(handle, "releasePointerCapture").mockImplementation((pointerId) => {
    handle.dispatchEvent(new PointerEvent("lostpointercapture", { bubbles: true, pointerId }));
  });
  document.body.style.cursor = "crosshair";
  const pointer = (type: string, pointerId = 1, clientX = 504) =>
    handle.dispatchEvent(
      new PointerEvent(type, { bubbles: true, pointerId, button: 0, clientX, clientY: 100 }),
    );
  return { screen, pointer };
}

test.each(["lostpointercapture", "blur"])(
  "%s settles the split and permits another drag",
  async (interruption) => {
    const { screen, pointer } = await divider();
    vi.spyOn(window, "requestAnimationFrame").mockReturnValue(71);
    const cancel = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => {});
    pointer("pointerdown");
    pointer("pointermove", 1, 650);
    expect(document.body.style.cursor).toBe("col-resize");

    if (interruption === "blur") window.dispatchEvent(new Event("blur"));
    else pointer("lostpointercapture");
    expect(native.release).toHaveBeenCalledExactlyOnceWith(null, null);
    expect(cancel).toHaveBeenCalledExactlyOnceWith(71);
    expect(native.drag).not.toHaveBeenCalled();
    expect(document.body.style.cursor).toBe("crosshair");
    // A late event from the interrupted pointer cannot end the new gesture.
    pointer("pointerdown", 2);
    pointer("pointerup", 1, 650);
    expect(native.release).toHaveBeenCalledTimes(1);
    pointer("pointerup", 2, 700);
    expect(native.grab).toHaveBeenCalledTimes(2);
    expect(native.release.mock.calls).toEqual([
      [null, null],
      [700, 100],
    ]);
    screen.unmount();
    expect(native.release).toHaveBeenCalledTimes(2);
  },
);

test("unmount settles a captured split once and restores the previous cursor", async () => {
  const { screen, pointer } = await divider();
  pointer("pointerdown");
  screen.unmount();
  window.dispatchEvent(new Event("blur"));
  expect(native.release).toHaveBeenCalledExactlyOnceWith(null, null);
  expect(document.body.style.cursor).toBe("crosshair");
});

test("pointerup sends the final coordinates once when releasing capture emits another event", async () => {
  const { pointer } = await divider();
  pointer("pointerdown");
  pointer("pointerup", 1, 720);
  expect(native.release).toHaveBeenCalledExactlyOnceWith(720, 100);
  expect(document.body.style.cursor).toBe("crosshair");
});
