import { expect, test, vi } from "vitest";

const release = vi.hoisted(() => vi.fn(async () => true));
vi.mock("$shared/ipc/bindings", () => ({ commands: { workReleaseMemory: release } }));

import { thumbnail } from "../frame-thumbs";

const PIXEL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAFklEQVR4nGP8z8Dwn4EIwESMolGFuBQBAHHqAg1SF5vzAAAAAElFTkSuQmCC";
const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));

test("pictures are let go of, and the webview asked to empty its cache, once the last canvas that drew one leaves", async () => {
  let settled: (value: boolean) => void = () => {};
  release.mockImplementationOnce(
    () =>
      new Promise<boolean>((resolve) => {
        settled = resolve;
      }),
  );
  const canvas = document.createElement("canvas");
  document.body.append(canvas);
  const action = thumbnail(canvas, { url: PIXEL, width: 4 });
  await vi.waitFor(() => expect(canvas.width).toBeGreaterThan(0));
  await wait(3500);
  expect(release).not.toHaveBeenCalled();
  action.destroy();
  canvas.remove();
  await wait(4300);
  expect(release).toHaveBeenCalledTimes(1);
  expect(release).toHaveBeenLastCalledWith(true);
  document.dispatchEvent(new Event("pointerdown"));
  expect(release).toHaveBeenLastCalledWith(false);
  settled(true);
  await vi.waitFor(() => expect(release).toHaveBeenCalledTimes(3));
  expect(release).toHaveBeenLastCalledWith(false);
}, 15_000);
