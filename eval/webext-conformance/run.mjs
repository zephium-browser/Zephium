import { createRequire } from "node:module";
import {
  mkdtemp,
  mkdir,
  readdir,
  readFile,
  writeFile,
  cp,
  rm,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { normalize } from "./normalize.mjs";
import { compare } from "./diff.mjs";
const root = dirname(fileURLToPath(import.meta.url));
const require = createRequire(
  new URL("../../frame/package.json", import.meta.url),
);
const { chromium } = require("playwright");
const playwrightVersion = require("playwright/package.json").version;
const browsers = JSON.parse(
  await readFile(
    join(
      dirname(
        createRequire(require.resolve("playwright/package.json")).resolve(
          "playwright-core/package.json",
        ),
      ),
      "browsers.json",
    ),
    "utf8",
  ),
);
const chromiumRevision = browsers.browsers.find(
  (browser) => browser.name === "chromium",
).revision;
if (playwrightVersion !== "1.63.0")
  throw new Error(
    `Expected locked Playwright 1.63.0, got ${playwrightVersion}`,
  );
const args = process.argv.slice(2);
const value = (flag) => {
  const index = args.indexOf(flag);
  if (index === -1) return undefined;
  if (!args[index + 1] || args[index + 1].startsWith("--"))
    throw new Error(`Missing value for ${flag}`);
  return args[index + 1];
};
for (let i = 0; i < args.length; i++) {
  if (["--case", "--output"].includes(args[i])) i++;
  else if (!["--check", "--record", "--headed"].includes(args[i]))
    throw new Error(`Unknown argument ${args[i]}`);
}
if (args.includes("--check") && args.includes("--record"))
  throw new Error("Choose --check or --record");
const selected = value("--case");
const output = resolve(value("--output") || join(root, "local-results"));
await mkdir(output, { recursive: true });
const cases = (await readdir(join(root, "cases"), { withFileTypes: true }))
  .filter(
    (entry) => entry.isDirectory() && (!selected || entry.name === selected),
  )
  .map((entry) => entry.name)
  .sort();
if (!cases.length) throw new Error("No matching cases");
const allowlist = JSON.parse(
  await readFile(join(root, "deviations.json"), "utf8"),
);
let failures = 0;
console.log("| Case | Result |");
console.log("| --- | --- |");
for (const id of cases) {
  const directory = await mkdtemp(join(tmpdir(), "zephium-webext-"));
  let context,
    server,
    deadline,
    engine = {
      name: "chromium",
      version: "unavailable",
      playwrightVersion,
      chromiumRevision,
    };
  let origins = {},
    extensionId = "",
    transcript;
  const consoles = [];
  try {
    const source = join(root, "cases", id);
    const metadata = JSON.parse(
      await readFile(join(source, "case.json"), "utf8"),
    );
    if (
      metadata.id !== id ||
      !metadata.title ||
      !Array.isArray(metadata.apis) ||
      !Array.isArray(metadata.permissions)
    )
      throw new Error("Malformed case metadata");
    const token = randomBytes(24).toString("hex");
    let resolveComplete;
    const complete = new Promise((resolve) => {
      resolveComplete = resolve;
    });
    let completed = false;
    let cdp;
    server = createServer(async (request, response) => {
      try {
        const url = new URL(request.url, "http://127.0.0.1");
        if (url.pathname.startsWith(`/${token}/`)) {
          if (request.method !== "POST") {
            response.writeHead(405).end();
            return;
          }
          let body = "";
          for await (const chunk of request) {
            body += chunk;
            if (Buffer.byteLength(body) > 2 * 1024 * 1024)
              throw new Error("Collector body exceeds 2 MiB");
          }
          const data = JSON.parse(body);
          response.setHeader("Content-Type", "application/json");
          if (url.pathname === `/${token}/complete`) {
            if (completed) {
              response.writeHead(409).end("{}");
              return;
            }
            if (
              !["complete", "error"].includes(data.status) ||
              !Array.isArray(data.observations)
            )
              throw new Error("Invalid completion");
            completed = true;
            response.end("{}");
            resolveComplete(data);
            return;
          }
          if (url.pathname === `/${token}/control`) {
            let result;
            if (data.operation === "consoleMessages") result = consoles;
            else if (data.operation === "stopBackground") {
              const session = await context.newCDPSession(
                context
                  .pages()
                  .find((page) =>
                    page.url().startsWith(`chrome-extension://${extensionId}/`),
                  ),
              );
              let workerTimer;
              const runningWorker = new Promise((resolve, reject) => {
                workerTimer = setTimeout(
                  () =>
                    reject(new Error("Extension worker discovery timed out")),
                  5000,
                );
                session.on(
                  "ServiceWorker.workerVersionUpdated",
                  ({ versions }) => {
                    const worker = versions.find(
                      (version) =>
                        version.scriptURL.startsWith(
                          `chrome-extension://${extensionId}/`,
                        ) && version.runningStatus === "running",
                    );
                    if (worker) resolve(worker);
                  },
                );
              });
              try {
                const [, worker] = await Promise.all([
                  session.send("ServiceWorker.enable"),
                  runningWorker,
                ]);
                await session.send("ServiceWorker.stopWorker", {
                  versionId: worker.versionId,
                });
              } finally {
                clearTimeout(workerTimer);
                await session.detach();
              }
              result = { stopped: true };
            } else if (data.operation === "invoke-action") {
              const target = await context
                .pages()
                .find((page) =>
                  page.url().startsWith(`chrome-extension://${extensionId}/`),
                )
                .evaluate(async (tabId) => {
                  await chrome.tabs.update(tabId, { active: true });
                  return (await chrome.tabs.get(tabId)).url;
                }, data.args.tabId);
              const tab = context
                .pages()
                .find((page) => page.url() === (data.args.url || target));
              if (!tab) throw new Error("Action target page missing");
              await tab.bringToFront();
              const browserSession = await context
                .browser()
                .newBrowserCDPSession();
              const { targetInfos } = await browserSession.send(
                "Target.getTargets",
                { filter: [{ type: "tab", exclude: false }] },
              );
              const targetInfo = targetInfos.find(
                (info) => info.url === (data.args.url || target),
              );
              if (!targetInfo) throw new Error("Action tab target missing");
              await browserSession.send("Extensions.triggerAction", {
                id: extensionId,
                targetId: targetInfo.targetId,
              });
              await browserSession.detach();
              result = {
                supported: true,
                method: "CDP Extensions.triggerAction",
              };
            } else
              throw new Error(`Unsupported host operation: ${data.operation}`);
            response.end(JSON.stringify(result));
            return;
          }
          response.writeHead(404).end("{}");
          return;
        }
        response.setHeader("Cache-Control", "no-store");
        const page = `<script>window.pageScriptRan=true;document.documentElement.dataset.pageScript='ran';</script><title>Fixture</title><body><p id="target">fixture</p>`;
        if (url.pathname === "/data.json") {
          response.setHeader("Content-Type", "application/json");
          response.end('{"ok":true}');
        } else if (url.pathname === "/resource.txt") {
          response.setHeader("Content-Type", "text/plain");
          response.end("fixture resource");
        } else if (url.pathname === "/frames.html") {
          response.setHeader("Content-Type", "text/html");
          response.end(
            `<!doctype html>${page}<iframe name="same" src="${origins.a}/frame.html?kind=same"></iframe><iframe name="cross" src="${origins.b}/frame.html?kind=cross"></iframe><iframe name="blank" src="about:blank"></iframe><iframe name="srcdoc" srcdoc="&lt;p&gt;srcdoc&lt;/p&gt;"></iframe>`,
          );
        } else if (url.pathname.endsWith(".html")) {
          response.setHeader("Content-Type", "text/html");
          response.end(`<!doctype html>${page}`);
        } else {
          response.writeHead(404).end("missing fixture");
        }
      } catch (error) {
        if (!response.headersSent)
          response.writeHead(500, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ error: error.message }));
      }
    });
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    origins = {
      a: `http://127.0.0.1:${server.address().port}`,
      b: `http://localhost:${server.address().port}`,
    };
    const extension = join(directory, "extension");
    await cp(source, extension, { recursive: true });
    for (const name of [
      "harness.js",
      "entry.js",
      "runner.html",
      "default-worker.js",
    ])
      await cp(join(root, "shared", name), join(extension, name));
    const manifest = JSON.parse(
      await readFile(join(extension, "manifest.json"), "utf8"),
    );
    manifest.manifest_version = 3;
    manifest.name ||= id;
    manifest.version ||= "1.0.0";
    manifest.host_permissions = [
      ...new Set([...(manifest.host_permissions || []), "http://127.0.0.1/*"]),
    ];
    manifest.background ||= { service_worker: "default-worker.js" };
    await writeFile(
      join(extension, "manifest.json"),
      JSON.stringify(manifest, null, 2),
    );
    await writeFile(
      join(extension, "config.js"),
      `export default ${JSON.stringify({ origins, collector: `${origins.a}/${token}` })};\n`,
    );
    const running = (async () => {
      context = await chromium.launchPersistentContext(
        join(directory, "profile"),
        {
          channel: "chromium",
          headless: !args.includes("--headed"),
          chromiumSandbox: true,
          locale: "en-US",
          viewport: { width: 1000, height: 700 },
          args: [
            `--load-extension=${extension}`,
            `--disable-extensions-except=${extension}`,
            "--lang=en-US",
            "--disable-background-networking",
            "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
          ],
          env: { ...process.env, LANG: "en_US.UTF-8" },
          timeout: 15000,
        },
      );
      engine.version = context.browser().version();
      await context.route("**/*", (route) => {
        const url = new URL(route.request().url());
        return ["chrome-extension:", "about:", "data:", "blob:"].includes(
          url.protocol,
        ) || ["127.0.0.1", "localhost"].includes(url.hostname)
          ? route.continue()
          : route.abort("blockedbyclient");
      });
      context.on("console", (message) =>
        consoles.push({ type: message.type(), text: message.text() }),
      );
      const worker =
        context.serviceWorkers()[0] ||
        (await context.waitForEvent("serviceworker", { timeout: 10000 }));
      extensionId = new URL(worker.url()).host;
      const page = await context.newPage();
      cdp = await context.newCDPSession(page);
      await cdp.send("Log.enable");
      cdp.on("Log.entryAdded", ({ entry }) =>
        consoles.push({ type: entry.level, text: entry.text }),
      );
      await page.goto(`chrome-extension://${extensionId}/runner.html`);
      return complete;
    })();
    transcript = await Promise.race([
      running,
      new Promise((_, reject) => {
        deadline = setTimeout(
          () =>
            reject(
              new Error(`Case timeout (${metadata.timeoutMs || 20000}ms)`),
            ),
          metadata.timeoutMs || 20000,
        );
      }),
    ]);
  } catch (error) {
    transcript = {
      status: "error",
      observations: [],
      error: { message: error.message },
    };
  } finally {
    clearTimeout(deadline);
    if (context) await context.close().catch(() => {});
    if (server) {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
    await rm(directory, { recursive: true, force: true });
  }
  const result = normalize(
    { schemaVersion: 1, caseId: id, engine, ...transcript },
    { origins, extensionId, profile: directory },
  );
  const json = JSON.stringify(result, null, 2) + "\n";
  await writeFile(join(output, `${id}.json`), json);
  let status = result.status === "complete" ? "pass" : "fail";
  if (status === "pass" && args.includes("--record"))
    await writeFile(join(root, "reference", `${id}.json`), json);
  if (status === "pass" && args.includes("--check")) {
    try {
      const reference = JSON.parse(
        await readFile(join(root, "reference", `${id}.json`), "utf8"),
      );
      const comparison = compare(reference, result, allowlist, {
        sameEngine: true,
      });
      status = comparison.status;
      if (status === "fail")
        console.error(JSON.stringify({ caseId: id, ...comparison }));
    } catch (error) {
      status = "fail";
      console.error(error.message);
    }
  }
  if (status === "fail") {
    failures++;
    console.error(
      JSON.stringify({ caseId: id, error: result.error, console: consoles }),
    );
  }
  console.log(`| ${id} | ${status} |`);
}
if (failures) process.exitCode = 1;
