import { describe, expect, test } from "vitest";
import { followMove, settleMove, glideMs } from "../capabilities/camera-follow";

const room = { width: 1200, top: 100, bottom: 800 };
const view = { x: 0, y: 0, zoom: 1 };
const base = { room, view, following: true, reduced: false, base: 320 };
const far = { x: 0, y: -1500, zoom: 1 };
const tall = { left: 200, top: 100, right: 900, bottom: 2000 };

describe("followMove", () => {
  test("a column that appears to the right pans just far enough, keeping the zoom", () => {
    const move = followMove({
      ...base,
      newest: { left: 1100, top: 300, right: 1500, bottom: 500 },
    });
    expect(move!.view.zoom).toBe(1);
    expect(move!.view.y).toBe(0);
    expect(move!.view.x).toBeLessThan(0);
    expect(1500 + move!.view.x).toBeLessThanOrEqual(room.width - 48);
    expect(move!.view.x).toBeGreaterThan(-600);
  });
  test("nothing new out of view means no motion", () => {
    expect(
      followMove({ ...base, newest: { left: 300, top: 300, right: 700, bottom: 500 } }),
    ).toBeNull();
  });
  test("the person's own panning stops it", () => {
    expect(
      followMove({
        ...base,
        following: false,
        newest: { left: 2000, top: 300, right: 2400, bottom: 500 },
      }),
    ).toBeNull();
  });
  test("a run past the request never pans back to it", () => {
    const moved = { x: -2000, y: 0, zoom: 1 };
    expect(
      followMove({
        ...base,
        view: moved,
        newest: { left: 2100, top: 300, right: 2500, bottom: 500 },
      }),
    ).toBeNull();
  });
  test("a span wider than the room is anchored by its left end", () => {
    const move = followMove({
      ...base,
      newest: { left: 1300, top: 300, right: 3000, bottom: 500 },
    });
    expect(1300 + move!.view.x).toBe(48);
  });
  test("reduced motion is instant, and motion is otherwise short", () => {
    const box = { left: 1100, top: 300, right: 1500, bottom: 500 };
    expect(followMove({ ...base, reduced: true, newest: box })!.ms).toBe(0);
    expect(followMove({ ...base, newest: box })!.ms).toBe(320);
    expect(glideMs(false, 800)).toBe(400);
    expect(glideMs(false, 100)).toBe(250);
  });
});

describe("settleMove", () => {
  test("a result already showing its top stays where it is", () => {
    expect(settleMove({ ...base, result: { ...tall, top: 300 } })).toBeNull();
  });
  test("a result whose top is out of view comes in with one pan", () => {
    const move = settleMove({ ...base, view: far, result: tall });
    expect(move!.view.zoom).toBe(1);
    expect(100 + move!.view.y).toBe(room.top + 48);
  });
  test("the person who moved is not moved again", () => {
    expect(settleMove({ ...base, following: false, view: far, result: tall })).toBeNull();
  });
  test("reduced motion settles instantly", () => {
    expect(settleMove({ ...base, reduced: true, view: far, result: tall })!.ms).toBe(0);
  });
});
