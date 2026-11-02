import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { browserMark } from "../browsers";

test("every browser native import supports has its own bundled mark", () => {
  const native = readFileSync("../crates/zephium-import/src/lib.rs", "utf8");
  const ids = [...native.matchAll(/Browser::\w+ => "([a-z-]+)"/gu)].map((match) => match[1]!);
  expect(ids.length).toBeGreaterThan(0);
  const marks = ids.map((id) => {
    const mark = browserMark(id);
    expect(mark, id).not.toBeNull();
    return mark!.light;
  });
  expect(new Set(marks).size).toBe(ids.length);
  expect(browserMark("unknown-browser")).toBeNull();
});
