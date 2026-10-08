import { describe, expect, it } from "vitest";
import {
  checkBundleBudget,
  GROWTH_ALLOWANCE,
  loadedBeforeOpening,
  UNRECORDED_LAZY_LIMIT,
} from "./bundle-budget";

it("allows ordinary growth over a recorded size and fails a real jump", () => {
  const recorded = { js: 100_000, css: 20_000 };
  expect(
    checkBundleBudget("panel", { js: 100_000 + GROWTH_ALLOWANCE.js, css: 20_000 }, recorded),
  ).toBeNull();
  expect(
    checkBundleBudget("panel", { js: 100_001 + GROWTH_ALLOWANCE.js, css: 20_000 }, recorded),
  ).toContain("JS grew");
  expect(
    checkBundleBudget("panel", { js: 100_000, css: 20_001 + GROWTH_ALLOWANCE.css }, recorded),
  ).toContain("CSS grew");
});

it("requires a recorded size for a page but lets a small new lazy destination through", () => {
  expect(checkBundleBudget("browser", { js: 1, css: 0 }, undefined)).toContain("no recorded size");
  expect(
    checkBundleBudget("lazy:features/new/New.svelte", { ...UNRECORDED_LAZY_LIMIT }, undefined),
  ).toBeNull();
  expect(
    checkBundleBudget(
      "lazy:features/new/New.svelte",
      { js: UNRECORDED_LAZY_LIMIT.js + 1, css: 0 },
      undefined,
    ),
  ).toContain("no recorded size");
});

it("rejects invalid sizes rather than disabling the check", () => {
  expect(checkBundleBudget("panel", { js: 1, css: 0 }, { js: NaN, css: 0 })).toContain("invalid");
});

describe("loadedBeforeOpening", () => {
  const root = (name: string, file: string, visited: string[], surface = false) => ({
    name,
    file,
    surface,
    visited: new Set(visited),
    files: new Set(visited),
  });

  it("gives a destination its page, and what the destination that opens it brought", () => {
    const chunks = {
      "page.js": { imports: ["shared.js"], dynamicImports: ["settings.js"] },
      "shared.js": { imports: [], dynamicImports: [] },
      "settings.js": { imports: ["settings-ui.js"], dynamicImports: ["about.js"] },
      "settings-ui.js": { imports: [], dynamicImports: [] },
      "about.js": { imports: ["settings-ui.js"], dynamicImports: [] },
    };
    const before = loadedBeforeOpening(chunks, [
      root("page", "page.js", ["page.js", "shared.js"], true),
      root("settings", "settings.js", ["settings.js", "settings-ui.js"]),
      root("about", "about.js", ["about.js", "settings-ui.js"]),
    ]);
    expect([...before.get("settings")!].sort()).toEqual(["page.js", "shared.js"]);
    expect([...before.get("about")!].sort()).toEqual([
      "page.js",
      "settings-ui.js",
      "settings.js",
      "shared.js",
    ]);
  });

  it("keeps only what every opener has loaded", () => {
    const chunks = {
      "page.js": { imports: [], dynamicImports: ["a.js", "b.js"] },
      "a.js": { imports: ["common.js"], dynamicImports: ["view.js"] },
      "b.js": { imports: ["common.js"], dynamicImports: ["view.js"] },
      "common.js": { imports: [], dynamicImports: [] },
      "view.js": { imports: ["common.js"], dynamicImports: [] },
    };
    const before = loadedBeforeOpening(chunks, [
      root("page", "page.js", ["page.js"], true),
      root("a", "a.js", ["a.js", "common.js"]),
      root("b", "b.js", ["b.js", "common.js"]),
      root("view", "view.js", ["view.js", "common.js"]),
    ]);
    expect([...before.get("view")!].sort()).toEqual(["common.js", "page.js"]);
  });

  it("gives a page and an unreachable destination nothing", () => {
    const chunks = {
      "page.js": { imports: [], dynamicImports: [] },
      "orphan.js": { imports: [], dynamicImports: [] },
    };
    const before = loadedBeforeOpening(chunks, [
      root("page", "page.js", ["page.js"], true),
      root("orphan", "orphan.js", ["orphan.js"]),
    ]);
    expect(before.get("page")!.size).toBe(0);
    expect(before.get("orphan")!.size).toBe(0);
  });
});
