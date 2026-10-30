import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

const source = readFileSync(
  new URL("./require-supported-platform.mjs", import.meta.url),
  "utf8",
);

for (const platform of ["linux", "darwin", "win32", "freebsd"]) {
  test(`platform guard: ${platform}`, () => {
    const exits = [];
    const errors = [];
    runInNewContext(source, {
      process: { platform, exit: (code) => exits.push(code) },
      console: { error: (message) => errors.push(message) },
    });
    assert.deepEqual(exits, platform === "freebsd" ? [1] : []);
    assert.equal(errors.length, platform === "freebsd" ? 1 : 0);
    if (platform === "freebsd") {
      assert.match(errors[0], /does not support freebsd/);
    }
  });
}
