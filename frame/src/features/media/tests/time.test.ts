import { expect, test } from "vitest";
import { clock, fraction } from "../lib/time";

test("times read as a player shows them", () => {
  expect(clock(0)).toBe("0:00");
  expect(clock(61.9)).toBe("1:01");
  expect(clock(3600 + 62)).toBe("1:01:02");
  expect(clock(-5)).toBe("0:00");
  expect(clock(Number.NaN)).toBe("0:00");
});

test("progress stays between the ends and is empty without a length", () => {
  expect(fraction(30, 120)).toBe(0.25);
  expect(fraction(500, 120)).toBe(1);
  expect(fraction(-1, 120)).toBe(0);
  expect(fraction(30, null)).toBe(0);
  expect(fraction(30, 0)).toBe(0);
});
