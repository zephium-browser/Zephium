import { readFile, writeFile, mkdir, access } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { RustPeer } from "./node-peer.mjs";
import { normalize } from "./normalize.mjs";
import { compare } from "./diff.mjs";
const root = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  return at < 0 ? undefined : args[at + 1];
};
const runtime = resolve(option("--runtime-root") || join(root, "../.."));
const output = resolve(option("--output") || join(root, "local-results/node"));
const supported = [
  "alarms-create-forms",
  "runtime-last-error-unchecked",
  "storage-bytes",
  "storage-get-forms",
  "storage-on-changed",
  "storage-quota",
];
const selected = option("--case");
if (selected && !supported.includes(selected))
  throw new Error(
    `Node dispatcher host cannot run ${selected}; supported: ${supported.join(", ")}`,
  );
try {
  await access(join(runtime, "crates/zephium-webext-runtime/tests/js_e2e.rs"));
} catch {
  console.error(
    "Node runner unavailable: JS-to-Rust dispatcher harness has not landed in this checkout. Pass --runtime-root PATH to a checkout containing feat/webext-js-integration commit 18c14044a or its descendant. No case executed.",
  );
  process.exit(2);
}
const revision = spawnSync("git", ["-C", runtime, "rev-parse", "HEAD"], {
  encoding: "utf8",
});
if (revision.status !== 0) throw new Error("Cannot identify runtime revision");
let executable = option("--host");
if (!executable) {
  const build = spawnSync(
    "cargo",
    [
      "test",
      "-p",
      "zephium-webext-runtime",
      "--test",
      "js_e2e",
      "--no-run",
      "--locked",
      "--message-format=json",
    ],
    {
      cwd: runtime,
      encoding: "utf8",
      timeout: 300000,
      maxBuffer: 16 * 1024 * 1024,
      env: process.env,
    },
  );
  process.stderr.write(build.stderr || "");
  if (build.status !== 0)
    throw new Error(
      `Rust dispatcher build failed: ${build.error?.message || build.status}`,
    );
  const artifacts = build.stdout
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter(
      (row) =>
        row.reason === "compiler-artifact" &&
        row.target?.name === "js_e2e" &&
        row.executable,
    );
  executable = artifacts.at(-1)?.executable;
  if (!executable) throw new Error("No dispatcher executable in Cargo output");
}
const binding = await import(
  pathToFileURL(
    join(runtime, "crates/zephium-webext-runtime/js/test/harness.mjs"),
  )
);
const harness = (
  await readFile(join(root, "shared/harness.js"), "utf8")
).replace("export function createHarness", "function createHarness");
await mkdir(output, { recursive: true });
let failures = 0;
console.log("| Case | Result | Differences |");
console.log("| --- | --- | --- |");
for (const id of selected ? [selected] : supported) {
  const peer = new RustPeer(executable);
  let timer, transcript;
  try {
    const manifest = JSON.parse(
      await readFile(join(root, "cases", id, "manifest.json"), "utf8"),
    );
    const realm = await peer.context(binding, manifest);
    realm.run("Date.now = () => 1000000");
    const global = realm.run("globalThis");
    global.__host = async (operation) => {
      if (operation === "consoleMessages")
        return [
          ...realm.logs.error.map((text) => ({ type: "error", text })),
          ...realm.logs.warn.map((text) => ({ type: "warning", text })),
        ];
      throw new Error(`Unsupported Node host operation ${operation}`);
    };
    realm.run(
      `${harness}\nglobalThis.h=createHarness({origins:{a:'http://127.0.0.1:1',b:'http://localhost:1'}});h.host=__host;`,
    );
    const source = (
      await readFile(join(root, "cases", id, "case.js"), "utf8")
    ).replace("export async function run", "async function run");
    realm.run(source);
    await Promise.race([
      realm.run("run(h)"),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error("Node case timeout (20000ms)")),
          20000,
        );
      }),
    ]);
    if (peer.error) throw peer.error;
    transcript = {
      status: "complete",
      observations: JSON.parse(realm.run("JSON.stringify(h.observations)")),
    };
  } catch (error) {
    transcript = {
      status: "error",
      observations: [],
      error: { message: error.message },
    };
  } finally {
    clearTimeout(timer);
    await peer.close();
  }
  const result = normalize(
    {
      schemaVersion: 1,
      caseId: id,
      engine: {
        name: "node-rust",
        version: revision.stdout.trim(),
        node: process.version,
        hostContext: "background",
      },
      ...transcript,
    },
    { extensionId: binding.EXTENSION_ID },
  );
  await writeFile(
    join(output, `${id}.json`),
    JSON.stringify(result, null, 2) + "\n",
  );
  let comparison = {
    status: result.status === "complete" ? "pass" : "fail",
    differences: [],
  };
  if (args.includes("--check") && result.status === "complete")
    comparison = compare(
      JSON.parse(await readFile(join(root, "reference", `${id}.json`), "utf8")),
      result,
      JSON.parse(await readFile(join(root, "deviations.json"), "utf8")),
    );
  console.log(
    `| ${id} | ${comparison.status} | ${comparison.differences.length} |`,
  );
  if (comparison.status === "fail") {
    failures++;
    console.error(
      JSON.stringify({ caseId: id, ...comparison, error: result.error }),
    );
  }
}
if (failures) process.exitCode = 1;
