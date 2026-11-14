import { readFile, writeFile, mkdir, access, readdir } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { RustPeer } from "./node-peer.mjs";
import { normalize } from "./normalize.mjs";
import { compare } from "./diff.mjs";
import {
  supportedCases,
  omittedCalls,
  inventory,
  projectReference,
} from "./node-plan.mjs";
const root = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  return at < 0 ? undefined : args[at + 1];
};
const runtime = resolve(option("--runtime-root") || join(root, "../.."));
const output = resolve(option("--output") || join(root, "local-results/node"));
const supported = supportedCases;
const selected = option("--case");
if (selected && !supported.includes(selected))
  throw new Error(
    `Node dispatcher host cannot run ${selected}; supported: ${supported.join(", ")}`,
  );
try {
  await access(join(runtime, "crates/zephium-webext-runtime/tests/js_e2e.rs"));
} catch {
  console.error(
    "Node runner unavailable: JS-to-Rust dispatcher harness has not landed in this checkout. The selected runtime checkout must contain tests/js_e2e.rs. No case executed.",
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
const origins = { a: "http://127.0.0.1:1", b: "http://localhost:1" };
const caseIds = (await readdir(join(root, "cases"))).sort();
await mkdir(join(output, "coverage"), { recursive: true });
await writeFile(
  join(output, "coverage/inventory.json"),
  JSON.stringify(inventory(caseIds), null, 2) + "\n",
);
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
    manifest.name ||= id;
    manifest.version ||= "1.0";
    manifest.host_permissions = [
      ...new Set([...(manifest.host_permissions || []), "http://127.0.0.1/*"]),
    ];
    const resources = [];
    const catalogs = {};
    async function scan(directory, prefix = "") {
      for (const entry of await readdir(directory, { withFileTypes: true })) {
        const name = `${prefix}${entry.name}`;
        if (entry.isDirectory())
          await scan(join(directory, entry.name), `${name}/`);
        else {
          resources.push(name);
          const locale = /^_locales\/([^/]+)\/messages\.json$/.exec(name)?.[1];
          if (locale)
            catalogs[locale] = JSON.parse(
              await readFile(join(directory, entry.name), "utf8"),
            );
        }
      }
    }
    await scan(join(root, "cases", id));
    const bootstrap = await peer.command("configure", {
      manifest,
      resources,
      catalogs,
    });
    const background = resources.includes("background.js")
      ? await peer.context(binding, bootstrap.manifest, {
          label: "background",
          bootstrap,
        })
      : null;
    if (background)
      background.run(
        await readFile(join(root, "cases", id, "background.js"), "utf8"),
      );
    const realm = await peer.context(binding, bootstrap.manifest, {
      label: "runner",
      kind: "extension",
      bootstrap,
    });
    realm.run(
      "globalThis.navigator = {userActivation:{isActive:false,hasBeenActive:false}}",
    );
    realm.run("Date.now = () => 1000000");
    const global = realm.run("globalThis");
    global.__host = async (operation) => {
      if (operation === "consoleMessages")
        return [
          ...peer.console,
          ...(background
            ? background.logs.error.map((text) => ({ type: "error", text }))
            : []),
          ...realm.logs.error.map((text) => ({ type: "error", text })),
          ...realm.logs.warn.map((text) => ({ type: "warning", text })),
        ];
      throw new Error(`Unsupported Node host operation ${operation}`);
    };
    realm.run(
      `${harness}\nglobalThis.h=createHarness({origins:{a:'http://127.0.0.1:1',b:'http://localhost:1'}});h.host=__host;`,
    );
    if (id === "storage-session-access") {
      global.__attachContent = async (tabId) => {
        const content = await peer.context(binding, bootstrap.manifest, {
          label: `content-${tabId}`,
          kind: "contentScript",
          tabId,
          bootstrap,
        });
        content.run(
          await readFile(join(root, "cases", id, "content.js"), "utf8"),
        );
      };
      realm.run(
        "const conformanceOpenTab=h.openTab;h.openTab=async url=>{const tab=await conformanceOpenTab(url);await __attachContent(tab.id);return tab}",
      );
    }
    if (omittedCalls[id]) {
      global.__omittedCalls = omittedCalls[id];
      realm.run(
        "const conformanceCall=h.call; h.call=(label,invoke)=>__omittedCalls.includes(label)?Promise.resolve(undefined):conformanceCall(label,invoke)",
      );
    }
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
        hostContext: "extension-page",
      },
      ...transcript,
      coverage: omittedCalls[id]
        ? { kind: "partial", omittedCalls: omittedCalls[id] }
        : { kind: "full" },
    },
    { extensionId: binding.EXTENSION_ID, origins },
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
      omittedCalls[id]
        ? projectReference(
            JSON.parse(
              await readFile(join(root, "reference", `${id}.json`), "utf8"),
            ),
            omittedCalls[id],
          )
        : JSON.parse(
            await readFile(join(root, "reference", `${id}.json`), "utf8"),
          ),
      result,
      JSON.parse(await readFile(join(root, "deviations.json"), "utf8")),
    );
  console.log(
    `| ${id} | ${omittedCalls[id] ? "partial-" : ""}${comparison.status} | ${comparison.differences.length} |`,
  );
  if (comparison.status === "fail") {
    failures++;
    console.error(
      JSON.stringify({ caseId: id, ...comparison, error: result.error }),
    );
  }
}
if (failures) process.exitCode = 1;
