import assert from "node:assert/strict";
import test from "node:test";
import { isSupportedPlatform } from "./require-supported-platform.mjs";

test("native platform allowlist includes Linux foundation", () => {
  for (const platform of ["darwin", "win32", "linux"]) {
    assert.equal(isSupportedPlatform(platform), true);
  }
  for (const platform of ["freebsd", "android", "aix", "", undefined]) {
    assert.equal(isSupportedPlatform(platform), false);
  }
});
