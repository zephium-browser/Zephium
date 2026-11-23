import { expect, test } from "vitest";
import { branch, merge, roundedPath } from "$shared/canvas";
import { registrableSite, siteKey, siteName } from "../lib/run/site";

test("parallel parts fork from one branch point, up and down alike, never over each other", () => {
  const start = { x: 326, y: 200 };
  const routes = branch(start, 368, [
    { x: 410, y: 32 },
    { x: 410, y: 200 },
    { x: 410, y: 368 },
    { x: 410, y: 520 },
  ]);
  expect(routes[1]).toEqual([start, { x: 410, y: 200 }]);
  // The level line holds the stub; the sides leave it an elbow before the trunk.
  expect(routes[0]).toEqual([
    { x: 356, y: 200 },
    { x: 368, y: 200 },
    { x: 368, y: 32 },
    { x: 410, y: 32 },
  ]);
  expect(routes[2]![0]).toEqual({ x: 356, y: 200 });
  // A later line on a side takes the trunk where the one before turned away.
  expect(routes[3]![0]).toEqual({ x: 368, y: 356 });
});

test("lines into one end run level to the collector and join a trunk into it", () => {
  const routes = merge(
    [
      { x: 900, y: 32 },
      { x: 700, y: 200 },
    ],
    960,
    { x: 1018, y: 116 },
  );
  expect(routes[0]!.at(0)).toEqual({ x: 900, y: 32 });
  expect(routes[0]!.at(-1)).toEqual({ x: 1018, y: 116 });
  expect(routes[1]!.at(0)).toEqual({ x: 700, y: 200 });
  expect(routes[1]!.at(1)).toEqual({ x: 960, y: 200 });
});

test("elbows turn on a 12 px radius", () => {
  expect(
    roundedPath([
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 100, y: 100 },
    ]),
  ).toBe("M 0,0 L 88,0 Q 100,0 100,12 L 100,100");
});

test("a site's parts across its countries are one, named as people call it", () => {
  expect(registrableSite("www.airbnb.co.uk")).toBe("airbnb.co.uk");
  expect(registrableSite("jobs.paessler.com")).toBe("paessler.com");
  expect(siteKey("airbnb.co.uk")).toBe(siteKey("www.airbnb.com"));
  expect(siteName("ycombinator.com", ["Apply to YC | Y Combinator"])).toBe("Y Combinator");
  expect(siteName("careers.epam.com", ["Senior Rust Engineer | Remote Work With EPAM"])).toBe(
    "EPAM",
  );
  expect(siteName("cmu.edu")).toBe("Carnegie Mellon");
  // Without a real name, the address as written: never a name made from a fragment.
  expect(siteName("dthain.github.io")).toBe("dthain.github.io");
  expect(siteName("www.cl.cam.ac.uk")).toBe("cam.ac.uk");
  expect(siteName("craftinginterpreters.com", ["Scanning · Crafting Interpreters"])).toBe(
    "Crafting Interpreters",
  );
  expect(siteName("rublon.com", ["Careers – Rublon"])).toBe("Rublon");
  expect(siteName("www.justonecookbook.com", ["Rice Bowl (Video) • Just One Cookbook"])).toBe(
    "Just One Cookbook",
  );
});
