import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import CaptureControl from "../CaptureControl.svelte";

const native = { stop: vi.fn() };

beforeEach(() => {
  vi.clearAllMocks();
  native.stop.mockResolvedValue(true);
});

test("native stop preserves the capture indicator until the host changes state", async () => {
  const screen = await render(CaptureControl, {
    onStop: native.stop,
    site: "https://call.example/",
    capture: { camera: "active", microphone: "none" },
  });
  const stop = screen.getByRole("button", {
    name: "Stop camera and microphone for https://call.example/",
  });
  await stop.click();
  await expect.poll(() => native.stop.mock.calls.length).toBe(1);
  await expect.element(stop).toBeEnabled();
  await expect.element(stop).toHaveAttribute("title", "Camera in use");
  expect(screen.container.querySelector("[data-zephium-capture-control]")).not.toBeNull();
});

test("muted native capture is labelled truthfully and failed stop offers recovery", async () => {
  native.stop.mockResolvedValue(false);
  const screen = await render(CaptureControl, {
    onStop: native.stop,
    site: "https://call.example/",
    capture: { camera: "none", microphone: "muted" },
  });
  const stop = screen.getByRole("button", {
    name: "Stop camera and microphone for https://call.example/",
  });
  await expect.element(stop).toHaveAttribute("title", "Microphone muted");
  await stop.click();
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("Could not request capture stop");
  await expect.element(stop).toBeEnabled();
});
