import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import CaptureControl from "../CaptureControl.svelte";

test("a live camera reads as recording and opens its menu under itself", async () => {
  const onOpen = vi.fn();
  const screen = await render(CaptureControl, {
    onOpen,
    site: "https://call.example/",
    capture: { camera: "active", microphone: "active" },
  });
  const control = screen.getByRole("button", {
    name: "Camera and microphone for https://call.example/",
  });
  await expect.element(control).toHaveAttribute("title", "Camera in use, Microphone in use");
  await expect.element(control).toHaveAttribute("aria-haspopup", "menu");
  await expect.element(control).toHaveAttribute("data-live");
  await control.click();
  expect(onOpen).toHaveBeenCalledOnce();
  const [x, y] = onOpen.mock.calls[0] as [number, number];
  const rect = control.element().getBoundingClientRect();
  expect(x).toBe(rect.left);
  expect(y).toBe(rect.bottom);
});

test("muted capture is labelled truthfully and is not drawn as recording", async () => {
  const screen = await render(CaptureControl, {
    onOpen: vi.fn(),
    site: "https://call.example/",
    capture: { camera: "none", microphone: "muted" },
  });
  const control = screen.getByRole("button", {
    name: "Camera and microphone for https://call.example/",
  });
  await expect.element(control).toHaveAttribute("title", "Microphone muted");
  await expect.element(control).not.toHaveAttribute("data-live");
});
