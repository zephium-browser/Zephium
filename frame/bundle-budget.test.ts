import { expect, it } from "vitest";
import { checkBundleBudget, GROWTH_ALLOWANCE, UNRECORDED_LAZY_LIMIT } from "./bundle-budget";

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
