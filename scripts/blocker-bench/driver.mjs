import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

const DEFAULT_TIMEOUT_MS = 60_000;
const QUIT_GRACE_MS = 10_000;

const FIREFOX_PREFS = {
  "browser.aboutwelcome.enabled": false,
  "browser.shell.checkDefaultBrowser": false,
  "browser.shell.skipDefaultBrowserCheckOnFirstRun": true,
  "browser.shell.didSkipDefaultBrowserCheckOnFirstRun": true,
  "browser.startup.homepage": "about:blank",
  "browser.startup.page": 0,
  "browser.startup.homepage_override.mstone": "ignore",
  "browser.startup.upgradeDialog.enabled": false,
  "browser.laterrun.enabled": false,
  "browser.uitour.enabled": false,
  "browser.messaging-system.whatsNewPanel.enabled": false,
  "browser.discovery.enabled": false,
  "browser.tabs.warnOnClose": false,
  "browser.warnOnQuit": false,
  "browser.sessionstore.resume_from_crash": false,
  "browser.crashReports.unsubmittedCheck.enabled": false,
  "browser.newtabpage.activity-stream.feeds.telemetry": false,
  "browser.newtabpage.activity-stream.telemetry": false,
  "browser.newtabpage.activity-stream.feeds.section.topstories": false,
  "browser.newtabpage.activity-stream.showSponsored": false,
  "browser.newtabpage.activity-stream.showSponsoredTopSites": false,
  "startup.homepage_welcome_url": "",
  "startup.homepage_welcome_url.additional": "",
  "trailhead.firstrun.didSeeAboutWelcome": true,
  "datareporting.policy.dataSubmissionEnabled": false,
  "datareporting.policy.dataSubmissionPolicyBypassNotification": true,
  "datareporting.policy.firstRunURL": "",
  "datareporting.healthreport.uploadEnabled": false,
  "toolkit.telemetry.enabled": false,
  "toolkit.telemetry.unified": false,
  "toolkit.telemetry.archive.enabled": false,
  "toolkit.telemetry.reportingpolicy.firstRun": false,
  "app.update.auto": false,
  "app.update.enabled": false,
  "app.update.service.enabled": false,
  "app.update.staging.enabled": false,
  "app.normandy.enabled": false,
  "extensions.update.enabled": false,
  "network.captive-portal-service.enabled": false,
  "network.connectivity-service.enabled": false,
  "remote.active-protocols": 1,
};

const out = value => process.stdout.write(`${JSON.stringify(value)}\n`);
const log = (...parts) => console.error("[driver]", ...parts);
const sleep = ms => new Promise(done => setTimeout(done, ms));
const elapsed = since => Math.round((performance.now() - since) * 10) / 10;
const exit = code => process.stdout.write("", () => process.exit(code));

function parseArgs(argv) {
  const args = { extensions: [], settleMs: 0, width: 1280, height: 900 };
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (value === undefined) throw new Error(`missing value for ${key}`);
    if (key === "--browser") args.browser = value;
    else if (key === "--executable") args.executable = value;
    else if (key === "--profile") args.profile = resolve(value);
    else if (key === "--extension") args.extensions.push(resolve(value));
    else if (key === "--settle-ms") args.settleMs = Number(value);
    else if (key === "--width") args.width = Number(value);
    else if (key === "--height") args.height = Number(value);
    else throw new Error(`unknown argument ${key}`);
  }
  if (args.browser !== "chromium" && args.browser !== "firefox") throw new Error("--browser must be chromium or firefox");
  if (!args.executable) throw new Error("--executable is required");
  if (!args.profile) throw new Error("--profile is required");
  if (!(args.settleMs >= 0 && args.width > 0 && args.height > 0)) throw new Error("--settle-ms, --width and --height must be numbers");
  return args;
}

function killTree(pid) {
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(pid), "/t", "/f"], { stdio: "ignore" });
    return;
  }
  // The browser leads its own process group so its helper processes go with it.
  for (const target of [-pid, pid]) {
    try {
      process.kill(target, "SIGKILL");
      return;
    } catch {}
  }
}

async function raceTimeout(promise, ms) {
  let timer;
  const timedOut = new Promise(done => { timer = setTimeout(done, ms, true); });
  try {
    return await Promise.race([promise.then(() => false), timedOut]);
  } finally {
    clearTimeout(timer);
    promise.catch(() => {});
  }
}

async function guarded(promise, ms, what) {
  if (await raceTimeout(promise, ms)) throw new Error(`${what} timed out after ${ms} ms`);
  return promise;
}

// One socket speaking both protocols: CDP answers carry `error.message`, BiDi answers carry `type: "error"`.
class Rpc {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Set();
    socket.onmessage = event => this.dispatch(JSON.parse(event.data));
    socket.onclose = () => {
      for (const { reject } of this.pending.values()) reject(new Error("browser connection closed"));
      this.pending.clear();
    };
  }

  static connect(url) {
    return new Promise((done, fail) => {
      const socket = new WebSocket(url);
      socket.onopen = () => done(new Rpc(socket));
      socket.onerror = () => fail(new Error(`cannot connect to ${url}`));
    });
  }

  dispatch(message) {
    if (message.id === undefined) {
      for (const listener of this.listeners) listener(message);
      return;
    }
    const entry = this.pending.get(message.id);
    if (!entry) return;
    this.pending.delete(message.id);
    if (message.type === "error") entry.reject(new Error(`${message.error}: ${message.message}`));
    else if (message.error) entry.reject(new Error(message.error.message ?? JSON.stringify(message.error)));
    else entry.resolve(message.result ?? {});
  }

  send(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify(sessionId ? { id, method, params, sessionId } : { id, method, params }));
    });
  }

  listen(listener) {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
}

// Both protocols run document-start scripts in subframes too.
const mainFrameOnly = source => `if (window.top === window.self) {\n${source}\n}`;

class Driver {
  constructor(rpc) {
    this.rpc = rpc;
  }

  async loadNew(tab, url, timeoutMs) {
    try {
      return { tab, ...(await this.load(tab, url, timeoutMs)) };
    } catch (error) {
      await this.close(tab).catch(() => {});
      throw error;
    }
  }
}

class Chromium extends Driver {
  constructor(rpc) {
    super(rpc);
    this.sessions = new Map();
    this.preloads = [];
  }

  session(tab) {
    const sessionId = this.sessions.get(tab);
    if (!sessionId) throw new Error(`unknown tab ${tab}`);
    return sessionId;
  }

  async open(url, timeoutMs, background = false) {
    const { targetId } = await this.rpc.send("Target.createTarget", { url: "about:blank", background });
    const { sessionId } = await this.rpc.send("Target.attachToTarget", { targetId, flatten: true });
    this.sessions.set(targetId, sessionId);
    await this.rpc.send("Page.enable", {}, sessionId);
    await this.rpc.send("Page.setLifecycleEventsEnabled", { enabled: true }, sessionId);
    // A background tab is throttled and loads differently, so only a tab
    // asked for behind the current one stays there.
    if (!background) await this.rpc.send("Page.bringToFront", {}, sessionId);
    for (const source of this.preloads) await this.rpc.send("Page.addScriptToEvaluateOnNewDocument", { source }, sessionId);
    return this.loadNew(targetId, url, timeoutMs);
  }

  async preload(source) {
    const wrapped = mainFrameOnly(source);
    this.preloads.push(wrapped);
    await Promise.all([...this.sessions.values()].map(sessionId =>
      this.rpc.send("Page.addScriptToEvaluateOnNewDocument", { source: wrapped }, sessionId)));
  }

  // The lifecycle "load" event fires alongside Page.loadEventFired, and its loaderId keeps a late event of the
  // previous document, or of a subframe, from ending the wait early. A navigation the browser restarts commits
  // under another loaderId, so the main frame's documents committed after Page.navigate replied count too, and
  // the reply says how many did. A commit seen before that reply may belong to an earlier navigation.
  async load(tab, url, timeoutMs) {
    const sessionId = this.session(tab);
    const loaded = new Set();
    const committed = new Set();
    let navigated;
    let wake = () => {};
    const stop = this.rpc.listen(message => {
      if (message.sessionId !== sessionId) return;
      if (message.method === "Page.frameNavigated" && !message.params.frame.parentId) {
        if (navigated) committed.add(message.params.frame.loaderId);
      } else if (message.method === "Page.lifecycleEvent" && message.params.name === "load") loaded.add(message.params.loaderId);
      else return;
      wake();
    });
    const start = performance.now();
    const done = () => navigated.loaderId && (loaded.has(navigated.loaderId) || [...committed].some(id => loaded.has(id)));
    try {
      const timedOut = await raceTimeout((async () => {
        navigated = await this.rpc.send("Page.navigate", { url }, sessionId);
        if (navigated.errorText) throw new Error(navigated.errorText);
        while (navigated.loaderId && !done()) await new Promise(resolve => { wake = resolve; });
      })(), timeoutMs);
      const restarted = [...committed].filter(id => id !== navigated?.loaderId).length;
      const loadWait = { committed: committed.size, restarted, loads: loaded.size };
      if (!timedOut) return restarted > 0 ? { loadMs: elapsed(start), loadWait } : { loadMs: elapsed(start) };
      return { loadMs: elapsed(start), timedOut: true, loadWait };
    } finally {
      stop();
    }
  }

  async evaluate(tab, expression) {
    const result = await this.rpc.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, this.session(tab));
    if (result.exceptionDetails) {
      const details = result.exceptionDetails;
      throw new Error(details.exception?.description ?? details.exception?.value ?? details.text);
    }
    return result.result.value;
  }

  async close(tab) {
    await this.rpc.send("Target.closeTarget", { targetId: tab });
    this.sessions.delete(tab);
  }

  async quit() {
    await this.rpc.send("Browser.close");
  }
}

class Firefox extends Driver {
  async open(url, timeoutMs, background = false) {
    const { context } = await this.rpc.send("browsingContext.create", { type: "tab", background });
    if (!background) await this.rpc.send("browsingContext.activate", { context });
    return this.loadNew(context, url, timeoutMs);
  }

  async load(tab, url, timeoutMs) {
    const start = performance.now();
    const timedOut = await raceTimeout(this.rpc.send("browsingContext.navigate", { context: tab, url, wait: "complete" }), timeoutMs);
    return { loadMs: elapsed(start), ...(timedOut && { timedOut: true }) };
  }

  async evaluate(tab, expression) {
    const result = await this.rpc.send("script.evaluate", {
      expression,
      target: { context: tab },
      awaitPromise: true,
      resultOwnership: "none",
    });
    if (result.type === "exception") {
      const details = result.exceptionDetails;
      throw new Error(details.text || details.exception?.value || "script exception");
    }
    return result.result.type === "string" ? result.result.value : undefined;
  }

  async preload(source) {
    await this.rpc.send("script.addPreloadScript", { functionDeclaration: `() => {\n${mainFrameOnly(source)}\n}` });
  }

  async close(tab) {
    await this.rpc.send("browsingContext.close", { context: tab });
  }

  async quit() {
    await this.rpc.send("browser.close");
  }

  async install(path) {
    await this.rpc.send("webExtension.install", { extensionData: { type: "archivePath", path } });
  }
}

const state = { child: null, driver: null, quitting: false, gone: false, exited: null };

function spawnBrowser(args, argv, stderr) {
  const child = spawn(args.executable, argv, {
    stdio: ["ignore", "ignore", stderr],
    detached: process.platform !== "win32",
  });
  state.child = child;
  state.exited = new Promise(done => {
    child.once("exit", (code, signal) => {
      state.gone = true;
      done();
      if (state.quitting) return;
      out({ event: "exited", code, ...(signal && { signal }) });
      exit(1);
    });
  });
  child.once("error", error => {
    log("cannot start browser:", error.message);
    out({ event: "exited", code: null, error: error.message });
    exit(1);
  });
  return child;
}

async function launchChromium(args) {
  const portFile = join(args.profile, "DevToolsActivePort");
  rmSync(portFile, { force: true });
  spawnBrowser(args, [
    "--remote-debugging-port=0",
    `--user-data-dir=${args.profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    `--window-size=${args.width},${args.height}`,
    "about:blank",
  ], "inherit");
  const deadline = Date.now() + DEFAULT_TIMEOUT_MS;
  let lines = [];
  while (lines.length < 2) {
    if (Date.now() > deadline) throw new Error("browser did not publish DevToolsActivePort");
    await sleep(50);
    try {
      lines = readFileSync(portFile, "utf8").split(/\r?\n/).filter(Boolean);
    } catch {}
  }
  const rpc = await Rpc.connect(`ws://127.0.0.1:${lines[0]}${lines[1]}`);
  await rpc.send("Browser.getVersion");
  return new Chromium(rpc);
}

async function launchFirefox(args) {
  const prefs = Object.entries(FIREFOX_PREFS).map(([name, value]) => `user_pref(${JSON.stringify(name)}, ${JSON.stringify(value)});`);
  writeFileSync(join(args.profile, "user.js"), `${prefs.join("\n")}\n`);
  const child = spawnBrowser(args, [
    "--remote-debugging-port", "0",
    "--profile", args.profile,
    "--no-remote",
    "--new-instance",
    "-width", String(args.width),
    "-height", String(args.height),
  ], "pipe");
  const endpoint = await new Promise((done, fail) => {
    const timer = setTimeout(() => fail(new Error("Firefox did not report a WebDriver BiDi endpoint")), DEFAULT_TIMEOUT_MS);
    createInterface({ input: child.stderr }).on("line", line => {
      console.error(line);
      const match = /WebDriver BiDi listening on (ws:\/\/\S+)/.exec(line);
      if (match) {
        clearTimeout(timer);
        done(match[1]);
      }
    });
  });
  const rpc = await Rpc.connect(`${endpoint.replace(/\/$/, "")}/session`);
  await rpc.send("session.new", { capabilities: {} });
  const driver = new Firefox(rpc);
  for (const path of args.extensions) await driver.install(path);
  return driver;
}

async function quit(driver) {
  state.quitting = true;
  const timer = setTimeout(() => killTree(state.child.pid), QUIT_GRACE_MS);
  driver.quit().catch(() => {});
  await state.exited;
  clearTimeout(timer);
}

const toolTimeout = command => (command.timeoutMs > 0 ? Number(command.timeoutMs) : DEFAULT_TIMEOUT_MS);

async function run(driver, command) {
  const timeoutMs = toolTimeout(command);
  switch (command.op) {
    case "open":
      return driver.open(command.url, timeoutMs, command.background === true);
    case "navigate":
      return driver.load(command.tab, command.url, timeoutMs);
    case "eval": {
      const expression = `(async () => {\n${command.js}\n})().then(v => JSON.stringify(v === undefined ? null : v))`;
      const text = await guarded(driver.evaluate(command.tab, expression), timeoutMs, "eval");
      return { value: text === undefined ? null : JSON.parse(text) };
    }
    case "preload":
      await guarded(driver.preload(command.js), timeoutMs, "preload");
      return {};
    case "close":
      await guarded(driver.close(command.tab), timeoutMs, "close");
      return {};
    default:
      throw new Error(`unknown op ${command.op}`);
  }
}

async function handle(driver, line) {
  let command;
  try {
    command = JSON.parse(line);
  } catch {
    out({ id: null, ok: false, error: "invalid JSON" });
    return;
  }
  const id = command?.id ?? null;
  if (command?.op === "quit") {
    await quit(driver);
    out({ id, ok: true });
    exit(0);
    return;
  }
  try {
    out({ id, ok: true, ...(await run(driver, command)) });
  } catch (error) {
    out({ id, ok: false, error: error?.message ?? String(error) });
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.browser === "chromium" && args.extensions.length) throw new Error("--extension needs --browser firefox");
  mkdirSync(args.profile, { recursive: true });
  const started = performance.now();
  const driver = await (args.browser === "firefox" ? launchFirefox : launchChromium)(args);
  const startupMs = Math.round(performance.now() - started);
  await sleep(args.settleMs);
  out({ event: "ready", pid: state.child.pid, startupMs });
  state.driver = driver;

  let queue = Promise.resolve();
  const lines = createInterface({ input: process.stdin });
  lines.on("line", line => { if (line.trim()) queue = queue.then(() => handle(driver, line)); });
  lines.on("close", () => { queue = queue.then(async () => { await quit(driver); exit(0); }); });
}

// The browser is detached, so a terminal's signal does not reach it. Before
// ready, exiting is enough: the exit hook below kills it.
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => (state.driver ? quit(state.driver) : Promise.resolve()).then(() => exit(1)));
}

process.on("exit", () => {
  if (state.child && !state.gone) killTree(state.child.pid);
});

main().catch(error => {
  log(error.message);
  state.quitting = true;
  out({ event: "exited", code: null, error: error.message });
  exit(1);
});
