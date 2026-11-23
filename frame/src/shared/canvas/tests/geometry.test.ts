import { describe, expect, test } from "vitest";
import {
  absolutePosition,
  bezier,
  bounds,
  cameNear,
  centreOn,
  digest,
  elbow,
  frameSpan,
  nextStop,
  overlaps,
  reveal,
  routedPoints,
  seenFar,
  snap,
  stack,
  toCanvas,
  toScreen,
  type FocusStop,
} from "..";

const room = { width: 1200, height: 800, top: 100, bottom: 80 };

describe("camera", () => {
  test("an item is centred in the band the chrome leaves, at the zoom asked", () => {
    expect(centreOn({ x: 800, y: 400, width: 480, height: 360 }, room)).toEqual({
      x: 360 - 800,
      y: 100 + 130 - 400,
      zoom: 1,
    });
    // Wider than the room: it starts 24 in, never off the left edge.
    expect(centreOn({ x: 0, y: 0, width: 2000, height: 100 }, room).x).toBe(24);
  });

  test("a span is framed whole down to half size, then by its right end", () => {
    expect(frameSpan({ x: 0, y: 0, width: 800 }, { width: 1200, top: 0 })).toEqual({
      x: 64,
      y: 40,
      zoom: 1,
    });
    const wide = frameSpan({ x: 0, y: 0, width: 4000 }, { width: 1200, top: 0 });
    expect(wide.zoom).toBe(0.5);
    expect(wide.x).toBe(64 - (4000 * 0.5 - 1072));
  });

  test("a stop in view moves nothing; one out of view comes in by the least pan", () => {
    const view = { x: 0, y: 0, zoom: 1 };
    expect(reveal({ x: 200, y: 300, width: 200, height: 40 }, view, room)).toBeNull();
    const below = reveal({ x: 200, y: 1400, width: 200, height: 40 }, view, room)!;
    expect(below.x).toBe(0);
    expect(1400 + 40 + below.y).toBe(800 - 80 - 48);
    const above = reveal({ x: 200, y: -300, width: 200, height: 40 }, view, room)!;
    expect(-300 + above.y).toBe(100 + 48);
  });

  test("screen and canvas points round-trip at any zoom", () => {
    const view = { x: -120, y: 40, zoom: 0.5 };
    const origin = { x: 10, y: 20 };
    const screen = toScreen({ x: 300, y: 500 }, origin, view);
    expect(screen).toEqual({ x: 10 + 150 - 120, y: 20 + 250 + 40 });
    expect(toCanvas(screen, origin, view)).toEqual({ x: 300, y: 500 });
  });

  test("an easing token becomes a curve from 0 to 1; a malformed one is linear", () => {
    const ease = bezier("cubic-bezier(0.2, 0, 0, 1)");
    expect(ease(0)).toBeCloseTo(0, 3);
    expect(ease(1)).toBeCloseTo(1, 3);
    expect(ease(0.5)).toBeGreaterThan(0.5);
    expect(bezier("ease")(0.3)).toBe(0.3);
  });
});

describe("culling", () => {
  const view = { x: 0, y: 0, zoom: 1 };
  test("what comes within a screen of the view is near, once", () => {
    const items = [
      { id: "in", rect: { x: 100, y: 100, width: 10, height: 10 } },
      { id: "beside", rect: { x: 1900, y: 100, width: 10, height: 10 } },
      { id: "far", rect: { x: 5000, y: 100, width: 10, height: 10 } },
    ];
    expect(cameNear(items, new Set(), view, 1000, 800)).toEqual(["in", "beside"]);
    expect(cameNear(items, new Set(["in"]), view, 1000, 800)).toEqual(["beside"]);
  });

  test("seen from far below half size, and near again only past 60%", () => {
    expect(seenFar(false, 0.49)).toBe(true);
    expect(seenFar(false, 0.55)).toBe(false);
    expect(seenFar(true, 0.55)).toBe(true);
    expect(seenFar(true, 0.61)).toBe(false);
  });
});

describe("lanes", () => {
  test("lanes stack on the grid, each a gap under the last", () => {
    expect(stack([100, 37, 0], 3, 24)).toEqual([8, 136, 200]);
    expect(snap(17)).toBe(24);
  });

  test("overlap counts the gap; bounds hold every rect", () => {
    const a = { x: 0, y: 0, width: 100, height: 100 };
    expect(overlaps(a, { x: 110, y: 0, width: 10, height: 10 })).toBe(false);
    expect(overlaps(a, { x: 110, y: 0, width: 10, height: 10 }, 16)).toBe(true);
    expect(bounds([a, { x: -20, y: 50, width: 10, height: 200 }])).toEqual({
      x: -20,
      y: 0,
      width: 120,
      height: 250,
    });
    expect(bounds([])).toBeNull();
  });

  test("a digest is short and stable", () => {
    expect(digest("part:a".repeat(40))).toBe(digest("part:a".repeat(40)));
    expect(digest("a")).not.toBe(digest("b"));
    expect(digest("x".repeat(500)).length).toBeLessThanOrEqual(7);
  });
});

describe("lines", () => {
  const route = {
    points: [
      { x: 10, y: 10 },
      { x: 50, y: 10 },
    ],
    from: { x: 10, y: 10 },
    to: { x: 0, y: 10 },
    laid: { source: { x: 0, y: 0 }, target: { x: 50, y: 0 } },
  };
  test("a routed line keeps its route, moves with both ends, and elbows when one moves alone", () => {
    expect(routedPoints(route, { x: 0, y: 0 }, { x: 50, y: 0 })).toBe(route.points);
    expect(routedPoints(route, { x: 5, y: 5 }, { x: 55, y: 5 })).toEqual([
      { x: 15, y: 15 },
      { x: 55, y: 15 },
    ]);
    expect(routedPoints(route, { x: 0, y: 0 }, { x: 200, y: 100 })).toEqual(
      elbow({ x: 10, y: 10 }, { x: 200, y: 110 }),
    );
    expect(routedPoints(route)).toBe(route.points);
  });
});

describe("items", () => {
  test("a child stands where its parent does, plus its own place", () => {
    const parent = { id: "p", position: { x: 100, y: 50 }, data: {} };
    const child = { id: "c", parentId: "p", position: { x: 10, y: 5 }, data: {} };
    expect(absolutePosition(child, [parent, child])).toEqual({ x: 110, y: 55 });
    expect(absolutePosition({ ...child, parentId: "gone" }, [child])).toEqual({ x: 10, y: 5 });
  });
});

describe("keyboard", () => {
  const stops: FocusStop[] = [
    { id: "card", rect: { x: 0, y: 0, width: 320, height: 40 } },
    { id: "row1", rect: { x: 0, y: 48, width: 320, height: 32 } },
    { id: "row2", rect: { x: 0, y: 80, width: 320, height: 32 } },
    { id: "inside", rect: { x: 400, y: 80, width: 280, height: 32 } },
    { id: "older", rect: { x: 0, y: 400, width: 320, height: 40 } },
  ];
  test("down and up go to the stop under or over, lined up first", () => {
    expect(nextStop(stops, "card", "down")).toBe("row1");
    expect(nextStop(stops, "row1", "down")).toBe("row2");
    expect(nextStop(stops, "row2", "down")).toBe("older");
    expect(nextStop(stops, "older", "up")).toBe("row2");
    expect(nextStop(stops, "card", "up")).toBeNull();
  });

  test("right goes across to what stands beside; left comes back", () => {
    expect(nextStop(stops, "row2", "right")).toBe("inside");
    expect(nextStop(stops, "inside", "left")).toBe("row2");
    expect(nextStop(stops, "inside", "right")).toBeNull();
  });

  test("from a stop that is gone, the first one; with none, nothing", () => {
    expect(nextStop(stops, "missing", "down")).toBe("card");
    expect(nextStop([], "missing", "down")).toBeNull();
  });
});
