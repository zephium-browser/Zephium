import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { inspect, verifyBraces } from "./audit-dependencies.mjs";

function report(overrides = {}) {
  return {
    metadata: { vulnerabilities: { high: 1, critical: 0 } },
    advisories: {
      fixture: {
        severity: "high",
        github_advisory_id: "GHSA-vfj7-8cjw-p6xm",
        module_name: "braces",
        findings: [{ version: "3.0.3", dev: true, paths: ["frame>stylelint>micromatch>braces"] }],
        ...overrides,
      },
    },
  };
}

test("the exact installed patch survives hostile pattern and AST regressions", () => {
  assert.equal(inspect(report()), 1);
});

test("the exception cannot hide another advisory or runtime dependency", () => {
  assert.throws(() => inspect(report({ github_advisory_id: "GHSA-unrelated" })));
  assert.throws(() => inspect(report({ findings: [{ version: "3.0.3", dev: false, paths: ["frame>stylelint>micromatch>braces"] }] })));
  assert.throws(() => inspect(report({ findings: [{ version: "3.0.2", dev: true, paths: ["frame>stylelint>micromatch>braces"] }] })));
});

test("incomplete or failed registry results cannot produce a passing audit", () => {
  assert.throws(() => inspect({ error: { code: "ECONNRESET" } }));
  assert.throws(() => inspect({ ...report(), advisories: {} }));
  assert.throws(() => inspect(report({ findings: [] })));
});

test("altered patch bytes are rejected before package code executes", () => {
  const directory = mkdtempSync(join(tmpdir(), "zephium-braces-test-"));
  try {
    mkdirSync(join(directory, "lib"));
    writeFileSync(join(directory, "package.json"), JSON.stringify({ version: "3.0.3" }));
    writeFileSync(join(directory, "lib/parse.js"), "// unpatched or changed");
    assert.throws(() => verifyBraces(join(directory, "index.js")), /Unverified braces patch/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
