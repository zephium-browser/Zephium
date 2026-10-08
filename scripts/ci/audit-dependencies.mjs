// GHSA-vfj7-8cjw-p6xm is excepted only for the exact locally patched
// braces 3.0.3 files. All other high/critical findings and registry errors fail.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, existsSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const advisory = "GHSA-vfj7-8cjw-p6xm";
const hashes = {
  "lib/parse.js": "e56573ef2598060f5b7f708c9f3056138b65835475f4033b3372b9a70c33bbc8",
  "lib/compile.js": "5bfecc8ae4bea1a9ecb058a3f8704728183bd6cbf3494aaa8c0e399b7e3f112d",
  "lib/expand.js": "b9021e848acc686dfc77542c4c8d256909e44542b602621a36a88bd58df54cb2",
  "lib/stringify.js": "cff4c78da974bb31f15e6f9e2cc01edde8f93b1958f246215d6cc91a4adb4eb5"
};

export function verifyBraces(entry) {
  const directory = dirname(entry);
  assert.equal(JSON.parse(readFileSync(resolve(directory, "package.json"))).version, "3.0.3");
  for (const [file, digest] of Object.entries(hashes)) {
    assert.equal(createHash("sha256").update(readFileSync(resolve(directory, file))).digest("hex"), digest,
      `Unverified braces patch: ${file}`);
  }
  const braces = createRequire(import.meta.url)(entry);
  const rejected = (error) => error instanceof SyntaxError && error.message === "Input nesting exceeds max depth (128)";
  for (const input of ["{".repeat(4000) + "x" + "}".repeat(4000), "(".repeat(4000) + "x" + ")".repeat(4000)]) {
    for (const operation of [braces, braces.parse, braces.compile, braces.expand, braces.stringify]) {
      assert.throws(() => operation(input), rejected);
    }
  }
  // Direct AST callers cannot bypass the pattern parser's bound.
  const nested = () => {
    let node = {type: "text", value: "x"};
    for (let i = 0; i < 300; i++) node = {type: "brace", open: true, close: true, nodes: [node]};
    return {type: "root", nodes: [node]};
  };
  for (const operation of [braces.compile, braces.expand, braces.stringify]) assert.throws(() => operation(nested()), rejected);
  assert.deepEqual(braces.expand("src/{a,b}.{js,ts}"), ["src/a.js", "src/a.ts", "src/b.js", "src/b.ts"]);
  assert.equal(braces.compile("a/{b,c}/d"), "a/(b|c)/d");
  assert.equal(braces.stringify("\\{".repeat(1000)), "{".repeat(1000));
}

function installedEntry(path) {
  const [workspace, ...dependencies] = path.split(">");
  assert.ok(["frame", "desktop", "zephium"].includes(workspace), "Unknown audit workspace");
  let entry = resolve(root, workspace === "zephium" ? "package.json" : `${workspace}/package.json`);
  assert.ok(dependencies.length > 0 && dependencies.length <= 64);
  for (const name of dependencies) {
    assert.match(name, /^(?:@[\w.-]+\/)?[\w.-]+$/u);
    const candidate = createRequire(entry).resolve.paths(name)
      ?.map((directory) => resolve(directory, name, "package.json"))
      .find((path) => existsSync(path));
    assert.ok(candidate, `Dependency is not installed: ${name}`);
    entry = realpathSync(candidate);
    assert.equal(JSON.parse(readFileSync(entry)).name, name);
  }
  assert.equal(dependencies.at(-1), "braces");
  return resolve(dirname(entry), "index.js");
}

export function inspect(report, verify = verifyBraces) {
  assert.ok(report && !report.error && report.advisories && report.metadata, "Registry audit did not return a complete report");
  const serious = Object.values(report.advisories).filter((item) => ["high", "critical"].includes(item.severity));
  assert.equal(serious.length, report.metadata.vulnerabilities.high + report.metadata.vulnerabilities.critical,
    "Audit summary and advisory details disagree");
  const checked = new Set();
  const failures = [];
  for (const finding of Object.values(report.advisories)) {
    if (!["high", "critical"].includes(finding.severity)) continue;
    if (finding.github_advisory_id !== advisory || finding.module_name !== "braces") {
      failures.push(`${finding.module_name}: ${finding.github_advisory_id ?? finding.title}`);
      continue;
    }
    assert.ok(finding.findings?.length, "Missing affected dependency paths");
    for (const affected of finding.findings) {
      assert.equal(affected.version, "3.0.3", "Exception does not cover this version");
      assert.equal(affected.dev, true, "Exception covers build tooling only");
      assert.ok(affected.paths?.length, "Missing affected dependency paths");
      for (const path of affected.paths) {
        const entry = installedEntry(path);
        if (!checked.has(entry)) { verify(entry); checked.add(entry); }
      }
    }
  }
  assert.equal(failures.length, 0, `Unresolved security advisories: ${failures.join(", ")}`);
  return checked.size;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const windows = process.platform === "win32";
  // The Windows shim is a cmd script; the shell command is fixed, with no
  // registry, filename or user-provided text interpolated into it.
  const result = spawnSync(windows ? "pnpm audit --json" : "pnpm", windows ? [] : ["audit", "--json"],
    {cwd: root, encoding: "utf8", shell: windows, maxBuffer: 16 * 1024 * 1024});
  assert.ok(!result.error && [0, 1].includes(result.status), result.stderr || "Audit process failed");
  const checked = inspect(JSON.parse(result.stdout));
  console.log(`Full dependency audit passed; ${checked} installed braces patch verified with hostile-pattern regression tests.`);
}
