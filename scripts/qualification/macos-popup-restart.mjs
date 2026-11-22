// Manual native qualification: three processes, one fresh disposable WebKit
// profile. No marketplace packages, accounts, personal profiles or native hosts.
import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
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
const outcomes = [];
for (const kind of ["classic", "module"]) {
  const profile = randomUUID();
  const output = join(metadata.target_directory, "popup-restart", profile);
  mkdirSync(output, { recursive: true });
  const fixture = join(output, "fixture");
  cpSync(
    join(root, "crates/zephium-webext-macos/fixtures/popup-restart"),
    fixture,
    { recursive: true },
  );
  if (kind === "module") {
    const manifest = JSON.parse(
      readFileSync(join(fixture, "manifest.json"), "utf8"),
    );
    manifest.background = { service_worker: "module.js", type: "module" };
    writeFileSync(join(fixture, "manifest.json"), JSON.stringify(manifest));
  }
  const marker = "popup-restart-fixture-v1";
  const report = `
  const deadline = Date.now() + 5000;
  while (globalThis.popupRestart?.status === 'waiting' && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const previous = (await chrome.storage.local.get('restartMarker')).restartMarker ?? null;
  await chrome.storage.local.set({ restartMarker: ${JSON.stringify(marker)} });
  const localPrevious = localStorage.getItem('restartMarker');
  localStorage.setItem('restartMarker', ${JSON.stringify(marker)});
  const database = await new Promise((resolve, reject) => {
    const request = indexedDB.open('popup-restart-fixture', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('state');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  const indexedPrevious = await new Promise((resolve, reject) => {
    const transaction = database.transaction('state', 'readwrite');
    const store = transaction.objectStore('state');
    let before = null;
    const request = store.get('restartMarker');
    request.onsuccess = () => { before = request.result ?? null; store.put(${JSON.stringify(marker)}, 'restartMarker'); };
    transaction.oncomplete = () => resolve(before);
    transaction.onabort = transaction.onerror = () => reject(transaction.error);
  });
  database.close();
  return { popup: globalThis.popupRestart ?? null, previous, localPrevious, indexedPrevious };
`;
  const steps = [
    {
      load: fixture,
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
      value.localPrevious === expectedPrevious &&
      value.indexedPrevious === expectedPrevious &&
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
      `${kind} ${phase}: ${passed ? "PASS" : "FAIL"} ${JSON.stringify(value ?? null)}`,
    );
    if (!passed) break;
    // An immediate relaunch can mask the failure. Keep the same pause after
    // both shutdown modes; do not infer durability from the child exiting.
    if (phase !== "unclean-restart") await delay(5000);
  }
  // Force a missing startup signal in a separate profile: the raw fixture
  // intentionally has no compatibility layer, while verification is enabled.
  // This makes the waiting/capacity cases independent of WebKit's restart race.
  const waiterProfile = randomUUID();
  const waiterLoad = {
    load: fixture,
    compat: false,
    verify_background_start: true,
  };
  const waiterRuns = [];
  if (runs.length === 3 && runs.every((run) => run.passed)) {
    for (const cancel of [false, true]) {
      await delay(5000);
      const phase = cancel ? "cancel-waiters" : "concurrent-waiters";
      const count = cancel ? 8 : 65;
      const scenario = join(output, `${phase}.json`);
      const probeSteps = [waiterLoad, { background_waiters: count, cancel }];
      if (cancel) {
        // Reload the same extension immediately after cancellation. An old
        // callback must not settle the replacement context's readiness.
        probeSteps.push(waiterLoad, { background: true });
      }
      probeSteps.push(...steps.slice(2));
      writeFileSync(scenario, JSON.stringify(probeSteps, null, 2));
      const result = spawnSync(binary, [scenario], {
        cwd: root,
        env: {
          ...process.env,
          LAB_PROFILE: waiterProfile,
          ZEPHIUM_WEBEXT_TRACE: "1",
        },
        encoding: "utf8",
        timeout: 45_000,
        maxBuffer: 2 * 1024 * 1024,
      });
      writeFileSync(
        join(output, `${phase}.log`),
        `${result.stdout ?? ""}\n${result.stderr ?? ""}`,
      );
      const parse = (label) => {
        const prefix = `RESULT ${label} `;
        const line = result.stdout
          ?.split("\n")
          .find((line) => line.startsWith(prefix));
        try {
          return JSON.parse(line?.slice(prefix.length));
        } catch {
          return null;
        }
      };
      const value = parse("background-waiters");
      const popup = parse("popup-restart");
      const passed =
        !result.error &&
        result.status === 0 &&
        value?.settled === count &&
        value.ok === (cancel ? 0 : 64) &&
        value.errors === (cancel ? 8 : 1) &&
        popup?.popup?.status === "ready" &&
        popup.previous === (cancel ? marker : null) &&
        popup.localPrevious === (cancel ? marker : null) &&
        popup.indexedPrevious === (cancel ? marker : null);
      waiterRuns.push({
        phase,
        passed,
        value,
        popup,
        exit: result.status,
        error: result.error?.message ?? null,
      });
      console.log(
        `${kind} ${phase}: ${passed ? "PASS" : "FAIL"} ${JSON.stringify(value)}`,
      );
    }
  }
  writeFileSync(
    join(output, "results.json"),
    JSON.stringify(
      { kind, profile, waiterProfile, binary, runs, waiterRuns },
      null,
      2,
    ),
  );
  console.log(`Evidence: ${output}`);
  outcomes.push(
    runs.length === 3 &&
      runs.every((run) => run.passed) &&
      waiterRuns.length === 2 &&
      waiterRuns.every((run) => run.passed),
  );
}
process.exitCode = outcomes.every(Boolean) ? 0 : 1;
