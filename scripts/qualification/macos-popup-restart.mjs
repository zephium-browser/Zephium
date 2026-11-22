// Manual native qualification: three processes, one fresh disposable WebKit
// profile. No marketplace packages, accounts, personal profiles or native hosts.
import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

if (process.platform !== "darwin") {
  throw new Error("This probe requires native macOS WebKit.");
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const cargo = (args) => {
  const result = spawnSync("cargo", args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`cargo ${args[0]} failed (${result.status})`);
  return result.stdout;
};
const metadata = JSON.parse(
  cargo(["metadata", "--no-deps", "--format-version", "1", "--locked"]),
);
cargo([
  "build",
  "-p",
  "zephium-webext-macos",
  "--features",
  "lab",
  "--bin",
  "webext-lab",
  "--locked",
]);
const binary = join(metadata.target_directory, "debug/webext-lab");
const profile = randomUUID();
const output = join(metadata.target_directory, "popup-restart", profile);
mkdirSync(output, { recursive: true });
const marker = "popup-restart-fixture-v1";
const report = `
  const deadline = Date.now() + 5000;
  while (globalThis.popupRestart?.status === 'waiting' && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const previous = (await chrome.storage.local.get('restartMarker')).restartMarker ?? null;
  await chrome.storage.local.set({ restartMarker: ${JSON.stringify(marker)} });
  return { popup: globalThis.popupRestart ?? null, previous };
`;
const steps = [
  {
    load: join(root, "crates/zephium-webext-macos/fixtures/popup-restart"),
    compat: true,
  },
  { background: true },
  { blank: true },
  { sleep: 2500 },
  { action: true },
  { eval: report, as: "popup-restart", timeout: 8000 },
];

const runs = [];
for (const phase of ["first-launch", "clean-restart", "unclean-restart"]) {
  const scenario = join(output, `${phase}.json`);
  // The lab ends with process::exit. Only the control explicitly closes the
  // popup and unloads its context; the second process leaves them live.
  const shutdown =
    phase === "first-launch"
      ? [{ close: "popup" }, { unload: true }, { sleep: 500 }]
      : [];
  writeFileSync(scenario, JSON.stringify([...steps, ...shutdown], null, 2));
  const result = spawnSync(binary, [scenario], {
    cwd: root,
    env: { ...process.env, LAB_PROFILE: profile, ZEPHIUM_WEBEXT_TRACE: "1" },
    encoding: "utf8",
    timeout: 45_000,
    maxBuffer: 2 * 1024 * 1024,
  });
  writeFileSync(
    join(output, `${phase}.log`),
    `${result.stdout ?? ""}\n${result.stderr ?? ""}`,
  );
  const line = result.stdout
    ?.split("\n")
    .find((line) => line.startsWith("RESULT popup-restart "));
  let value;
  try {
    value = JSON.parse(line?.slice("RESULT popup-restart ".length));
  } catch {
    /* A missing/invalid result fails below. */
  }
  const expectedPrevious = phase === "first-launch" ? null : marker;
  const passed =
    !result.error &&
    result.status === 0 &&
    value?.popup?.status === "ready" &&
    typeof value.popup.boot === "string" &&
    value.previous === expectedPrevious &&
    (phase === "first-launch" ||
      value.popup.boot !== runs.at(-1)?.value?.popup?.boot);
  runs.push({
    phase,
    passed,
    value: value ?? null,
    exit: result.status,
    error: result.error?.message ?? null,
  });
  console.log(
    `${phase}: ${passed ? "PASS" : "FAIL"} ${JSON.stringify(value ?? null)}`,
  );
  if (!passed) break;
  // An immediate relaunch can mask the failure. Keep the same pause after
  // both shutdown modes; do not infer durability from the child exiting.
  if (phase !== "unclean-restart") await delay(5000);
}
writeFileSync(
  join(output, "results.json"),
  JSON.stringify({ profile, binary, runs }, null, 2),
);
console.log(`Evidence: ${output}`);
process.exitCode = runs.length === 3 && runs.every((run) => run.passed) ? 0 : 1;
