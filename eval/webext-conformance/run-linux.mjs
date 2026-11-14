// Runs the conformance cases on the WebKitGTK runtime. Start it inside the
// private test session: timeout -k 10 3600 scripts/ci/linux-test.sh node
// eval/webext-conformance/run-linux.mjs --host BINARY --module LIB [--check]
// [--case ID] [--output DIR]. BINARY is the built conformance_host test.
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { cp, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { compare } from "./diff.mjs";
import { normalize } from "./normalize.mjs";

const root = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const value = (flag) => {
  const index = args.indexOf(flag);
  if (index === -1) return undefined;
  if (!args[index + 1] || args[index + 1].startsWith("--"))
    throw new Error(`Missing value for ${flag}`);
  return args[index + 1];
};
for (let i = 0; i < args.length; i++) {
  if (["--case", "--output", "--host", "--module"].includes(args[i])) i++;
  else if (args[i] !== "--check") throw new Error(`Unknown argument ${args[i]}`);
}
const host = value("--host");
const module = value("--module");
if (!host || !module) throw new Error("--host and --module are required");
const selected = value("--case");
const output = resolve(value("--output") || join(root, "local-results-linux"));
await mkdir(output, { recursive: true });
const cases = (await readdir(join(root, "cases"), { withFileTypes: true }))
  .filter((entry) => entry.isDirectory() && (!selected || entry.name === selected))
  .map((entry) => entry.name)
  .sort();
const allowlist = JSON.parse(await readFile(join(root, "deviations.json"), "utf8"));
const engine = { name: "zephium-webkitgtk", version: "native" };

// A stable extension id in Chrome's alphabet, like an unpacked path's.
const extensionIdFor = (path) =>
  [...createHash("sha256").update(path).digest().subarray(0, 16)]
    .map((byte) => String.fromCharCode(97 + (byte >> 4)) + String.fromCharCode(97 + (byte & 15)))
    .join("");

let failures = 0;
console.log("| Case | Result |");
console.log("| --- | --- |");
for (const id of cases) {
  const directory = await mkdtemp(join(tmpdir(), "zephium-webext-linux-"));
  let server, child, deadline;
  let origins = {};
  let transcript;
  const extension = join(directory, "extension");
  const extensionId = extensionIdFor(extension);
  try {
    const source = join(root, "cases", id);
    const metadata = JSON.parse(await readFile(join(source, "case.json"), "utf8"));
    const token = randomBytes(24).toString("hex");
    let resolveComplete;
    const complete = new Promise((resolve) => (resolveComplete = resolve));
    let completed = false;
    const pending = new Map();
    let nextRequest = 1;
    const control = (op, controlArgs) =>
      new Promise((resolve, reject) => {
        const request = nextRequest++;
        pending.set(request, { resolve, reject });
        child.stdin.write(JSON.stringify({ id: request, op, args: controlArgs }) + "\n");
      });
    server = createServer(async (request, response) => {
      try {
        const url = new URL(request.url, "http://127.0.0.1");
        if (url.pathname.startsWith(`/${token}/`)) {
          let body = "";
          for await (const chunk of request) {
            body += chunk;
            if (Buffer.byteLength(body) > 2 * 1024 * 1024) throw new Error("Collector body exceeds 2 MiB");
          }
          const data = JSON.parse(body);
          response.setHeader("Content-Type", "application/json");
          if (url.pathname === `/${token}/complete`) {
            if (completed) return response.writeHead(409).end("{}");
            completed = true;
            response.end("{}");
            resolveComplete(data);
            return;
          }
          if (url.pathname === `/${token}/control`) {
            const result = await control(data.operation, data.args);
            response.end(JSON.stringify(result));
            return;
          }
          return response.writeHead(404).end("{}");
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
        } else response.writeHead(404).end("missing fixture");
      } catch (error) {
        if (!response.headersSent) response.writeHead(500, { "Content-Type": "application/json" });
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
    await cp(source, extension, { recursive: true });
    for (const name of ["harness.js", "entry.js", "runner.html", "default-worker.js"])
      await cp(join(root, "shared", name), join(extension, name));
    const manifest = JSON.parse(await readFile(join(extension, "manifest.json"), "utf8"));
    manifest.manifest_version = 3;
    manifest.name ||= id;
    manifest.version ||= "1.0.0";
    manifest.host_permissions = [...new Set([...(manifest.host_permissions || []), "http://127.0.0.1/*"])];
    manifest.background ||= { service_worker: "default-worker.js" };
    await writeFile(join(extension, "manifest.json"), JSON.stringify(manifest, null, 2));
    await writeFile(
      join(extension, "config.js"),
      `export default ${JSON.stringify({ origins, collector: `${origins.a}/${token}` })};\n`,
    );
    child = spawn(host, ["--ignored", "--exact", "conformance_host", "--nocapture", "--test-threads=1"], {
      env: {
        ...process.env,
        LANG: "en_US.UTF-8",
        ZEPHIUM_WEBEXT_PROBE_PRIVATE_SESSION: "1",
        ZEPHIUM_WEBEXT_MODULE_PATH: module,
        ZEPHIUM_CONFORMANCE_EXTENSION: extension,
        ZEPHIUM_CONFORMANCE_ID: extensionId,
      },
      stdio: ["pipe", "pipe", "inherit"],
    });
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      if (!line.startsWith("ZCH ")) return;
      const message = JSON.parse(line.slice(4));
      if (message.event === "install-failed") resolveComplete({ status: "error", observations: [], error: { message: message.error } });
      const waiting = pending.get(message.id);
      if (!waiting) return;
      pending.delete(message.id);
      if (message.error) waiting.reject(new Error(message.error));
      else waiting.resolve(message.result);
    });
    transcript = await Promise.race([
      complete,
      new Promise((_, reject) => {
        deadline = setTimeout(() => reject(new Error(`Case timeout (${metadata.timeoutMs || 20000}ms)`)), (metadata.timeoutMs || 20000) + 10000);
      }),
    ]);
  } catch (error) {
    transcript = { status: "error", observations: [], error: { message: error.message } };
  } finally {
    clearTimeout(deadline);
    if (child && child.exitCode === null) {
      child.stdin.end(JSON.stringify({ op: "quit" }) + "\n");
      await new Promise((resolve) => {
        const kill = setTimeout(() => child.kill("SIGKILL"), 20000);
        child.once("exit", () => {
          clearTimeout(kill);
          resolve();
        });
      });
    }
    if (server) {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
    await rm(directory, { recursive: true, force: true });
  }
  const result = normalize({ schemaVersion: 1, caseId: id, engine, ...transcript }, { origins, extensionId, profile: directory });
  await writeFile(join(output, `${id}.json`), JSON.stringify(result, null, 2) + "\n");
  let status = result.status === "complete" ? "complete" : "error";
  let detail = "";
  if (status === "complete" && args.includes("--check")) {
    try {
      const reference = JSON.parse(await readFile(join(root, "reference", `${id}.json`), "utf8"));
      const comparison = compare(reference, result, allowlist, { sameEngine: false });
      status = comparison.status;
      if (status !== "pass") detail = JSON.stringify(comparison).slice(0, 2000);
    } catch (error) {
      status = "fail";
      detail = error.message;
    }
  } else if (status === "error") detail = JSON.stringify(result.error ?? {}).slice(0, 500);
  if (status !== "pass" && status !== "complete") failures++;
  console.log(`| ${id} | ${status} |`);
  if (detail) console.error(`${id}: ${detail}`);
}
if (failures) process.exitCode = 1;
