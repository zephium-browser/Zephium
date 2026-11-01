// Runs the compatibility layer against stub extension contexts and checks
// that each fix is actually applied.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../src/compat/compat.js", import.meta.url), "utf8");
const event = () => ({ addListener() {}, removeListener() {}, hasListener: () => false });

function context(kind, declared = []) {
  const invalid = (name) => new Error(`Invalid call. The 'permissions' value is invalid, because '${name}' is not a valid permission.`);
  const chrome = {
    runtime: {
      id: "abcdefghijklmnopabcdefghijklmnop",
      sent: [],
      granted: ["privacy"],
      settings: {},
      async sendNativeMessage(_app, message) {
        this.sent.push(message.api);
        this.messages.push(message);
        switch (message.api) {
          case "trace":
            return false;
          case "permissions.state":
            return { granted: this.granted, declared: ["privacy", "idle"] };
          case "permissions.request":
            for (const name of message.permissions) if (!this.granted.includes(name)) this.granted.push(name);
            return true;
          case "permissions.remove":
            this.granted = this.granted.filter((name) => !message.permissions.includes(name));
            return true;
          case "privacy.get":
            return message.setting in this.settings
              ? { value: this.settings[message.setting], levelOfControl: "controlled_by_this_extension" }
              : { value: true, levelOfControl: "controllable_by_this_extension" };
          case "privacy.set":
            this.settings[message.setting] = message.value;
            return null;
          case "notifications.create":
            return message.id;
          case "notifications.update":
            return message.id === "n1";
          case "idle.query":
            return message.interval >= 15 ? "active" : "invalid";
          case "sessions.getRecentlyClosed":
            return [{ lastModified: 1, tab: { sessionId: "s1", url: "https://a.example/", title: "A" } }];
          case "sessions.restore":
            return { lastModified: 1, tab: { sessionId: message.sessionId || "s1", url: "https://example.com/", title: "A" } };
          case "downloads.download":
            return 5;
          case "downloads.search":
            return [{ id: 5, state: "complete" }, { id: 6, state: "in_progress" }];
          case "topSites.get":
            return [{ url: "https://a.example/", title: "A" }];
          case "bookmarks.create":
            return { id: "9", parentId: message.details.parentId, index: 0, title: message.details.title };
          case "history.search":
            return [{ id: "1", url: "https://a.example/", title: "A", lastVisitTime: 1, visitCount: 1, typedCount: 0 }];
          default:
            return null;
        }
      },
      messages: [],
      ports: [],
      connectNative(application) {
        const listeners = new Set();
        const port = {
          application,
          posted: [],
          onMessage: {
            addListener: (listener) => void listeners.add(listener),
            removeListener: (listener) => void listeners.delete(listener),
            hasListener: (listener) => listeners.has(listener),
          },
          onDisconnect: event(),
          postMessage(message) {
            this.posted.push(message);
          },
          disconnect() {},
          deliver: (message) => listeners.forEach((listener) => listener(message)),
        };
        this.ports.push(port);
        return port;
      },
      onMessage: event(),
      onConnect: event(),
      getManifest: () => ({
        name: "__MSG_name__",
        version: "1.2",
        permissions: ["tabs", "https://a.test/*", "privacy", "notifications", "offscreen", ...declared],
        optional_permissions: ["idle"],
        options_page: "options.html",
      }),
      getURL: (path) => `chrome-extension://abcdefghijklmnopabcdefghijklmnop/${path}`,
    },
    i18n: { getMessage: (key) => (key === "name" ? "Probe" : "") },
    webRequest: {
      onBeforeRequest: {
        addListener(_listener, filter) {
          if (filter.urls.some((url) => url.startsWith("ws"))) throw new Error("invalid match pattern");
          if ((filter.types || []).includes("webbundle")) throw new Error("invalid resource type");
          this.filters.push(filter.urls);
          this.types.push(filter.types);
        },
        filters: [],
        types: [],
      },
    },
    contextMenus: {
      created: [],
      create(properties) {
        if ((properties.documentUrlPatterns || []).some((pattern) => pattern.startsWith("ftp"))) throw new Error("invalid pattern");
        this.created.push(properties);
        return properties.id;
      },
    },
    declarativeNetRequest: {
      updateSessionRules(options) {
        const index = options.addRules.findIndex((rule) => rule.custom);
        if (index >= 0) throw new Error(`The 'addRules' value is invalid, because an error with rule at index ${index}: bad header.`);
        this.applied = options.addRules.map((rule) => rule.id);
        return Promise.resolve();
      },
      updateDynamicRules(options) {
        if (options.addRules.some((rule) => rule.condition && /\(\?[=!]/.test(rule.condition.regexFilter))) throw new Error("unexpected regexFilter");
        return this.updateSessionRules(options);
      },
      enabled: ["a"],
      compiling: 2,
      attempts: 0,
      getEnabledRulesets() {
        return Promise.resolve([...this.enabled]);
      },
      getDynamicRules: () => Promise.resolve([]),
      getSessionRules: () => Promise.resolve([]),
      updateEnabledRulesets(options) {
        this.attempts++;
        const next = this.enabled.filter((id) => !(options.disableRulesetIds || []).includes(id)).concat(options.enableRulesetIds || []);
        if (this.broken || (!next.length && this.compiling-- > 0)) return Promise.reject(new Error("Invalid call to declarativeNetRequest.updateEnabledRulesets(). Failed to apply rules."));
        this.enabled = next;
        return Promise.resolve();
      },
    },
    permissions: {
      contains(request) {
        const unknown = (request.permissions || []).find((name) => name === "tabGroups");
        if (unknown) throw invalid(unknown);
        if ((request.origins || []).some((origin) => /:\d+\//.test(origin))) throw new Error("invalid match pattern");
        return Promise.resolve(true);
      },
      request: () => Promise.resolve(true),
      remove: () => Promise.resolve(true),
    },
    storage: {
      local: {
        set(items) {
          if (Object.getPrototypeOf(items) !== Object.prototype) throw new Error("The 'items' value is invalid, because an object is expected.");
          this.saved = items;
          return Promise.resolve();
        },
      },
      onChanged: event(),
    },
    scripting: {
      registerContentScripts: () => Promise.reject(new Error("Duplicate ID 'a'.")),
      updateContentScripts: () => Promise.resolve(),
    },
    webNavigation: { onCommitted: event() },
    tabs: { onUpdated: event(), onRemoved: event(), sendMessage() {}, query: () => Promise.resolve([{ id: 1 }]), getCurrent: () => Promise.resolve({ id: 1 }) },
    extension: { popups: [], getViews({ type }) { return type === "popup" ? this.popups : []; } },
  };
  const g = {
    chrome,
    browser: { ...chrome },
    // As in WebKit, the user agent lives on Navigator.prototype.
    navigator: Object.create({ userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15" }),
    addEventListener() {},
    setTimeout: (callback) => (queueMicrotask(callback), 0),
    setInterval: () => 0,
    clearInterval() {},
    console: { error() {}, warn() {} },
    WebAssembly: { instantiateStreaming() {}, compileStreaming() {} },
    Symbol,
    Promise,
    Object,
    Array,
    String,
    Boolean,
    Error,
    Set,
    WeakMap,
    WeakSet,
    Map,
    JSON,
    Date,
    Math,
    RegExp,
    Number,
    queueMicrotask,
  };
  if (kind === "worker") {
    g.ServiceWorkerGlobalScope = class {
      static [Symbol.hasInstance]() {
        return true;
      }
    };
    g.location = { protocol: "chrome-extension:", href: "chrome-extension://x/sw.js" };
    g.EventTarget = class {};
  } else if (kind === "page") {
    g.location = { protocol: "chrome-extension:", pathname: "/popup.html", href: "chrome-extension://x/popup.html" };
  } else {
    g.location = { protocol: "https:", pathname: "/", href: "https://example.com/" };
  }
  g.globalThis = g;
  vm.createContext(g);
  vm.runInContext(source, g);
  return g;
}

test("pages and workers get every API fix", async () => {
  for (const kind of ["page", "worker"]) {
    const { chrome, browser } = context(kind);
    assert.equal(browser, chrome, `${kind}: browser aliases chrome`);
    assert.equal(await chrome.permissions.contains({ permissions: ["privacy"] }), true, kind);
    assert.equal(await chrome.permissions.contains({ permissions: ["idle"] }), false, kind);
    assert.equal(await chrome.permissions.request({ permissions: ["idle"] }), true, kind);
    assert.equal(await chrome.permissions.contains({ permissions: ["idle", "privacy"] }), true, kind);
    assert.equal(await chrome.idle.queryState(20), "active", kind);
    assert.equal(await chrome.permissions.contains({ permissions: ["proxy"] }), false, kind);
    assert.equal(await chrome.permissions.contains({ permissions: ["tabGroups"] }), false, kind);
    assert.equal(Object.keys(await chrome.storage.managed.get()).length, 0, kind);
    assert.equal(typeof chrome.storage.managed.onChanged.addListener, "function", kind);
    assert.equal(chrome.scripting.ExecutionWorld.ISOLATED, "ISOLATED", kind);
    await chrome.scripting.registerContentScripts([{ id: "a" }]);
    assert.equal(typeof chrome.webNavigation.onHistoryStateUpdated.addListener, "function", kind);
    assert.equal(typeof chrome.notifications.create, "function", kind);
    assert.equal(await chrome.notifications.create("n1", { title: "t", message: "m" }), "n1", kind);
    assert.match(await chrome.notifications.create({ title: "t", message: "m" }), /^zephium-/, kind);
    assert.equal(await chrome.notifications.update("n1", { progress: 50 }), true, kind);
    assert.equal(await chrome.notifications.update("gone", { progress: 50 }), false, kind);
    assert.match(chrome.identity.getRedirectURL("cb"), /^https:\/\/abcdefghijklmnopabcdefghijklmnop\.chromiumapp\.org\/cb$/);
    const saving = await chrome.privacy.services.passwordSavingEnabled.get({});
    assert.equal(saving.value, true, kind);
    assert.equal(saving.levelOfControl, "controllable_by_this_extension", kind);
    await chrome.privacy.services.passwordSavingEnabled.set({ value: false });
    const set = await chrome.privacy.services.passwordSavingEnabled.get({});
    assert.equal(set.value, false, kind);
    assert.equal(set.levelOfControl, "controlled_by_this_extension", kind);

    assert.equal(chrome.runtime.OnInstalledReason.INSTALL, "install", kind);
    assert.equal(typeof chrome.runtime.onUpdateAvailable.addListener, "function", kind);
    assert.equal((await chrome.runtime.requestUpdateCheck()).status, "no_update", kind);

    assert.equal(chrome.webRequest.OnHeadersReceivedOptions.EXTRA_HEADERS, "extraHeaders", kind);
    chrome.webRequest.onBeforeRequest.addListener(() => {}, { urls: ["ws://*/*", "https://*/*"] });
    chrome.webRequest.onBeforeRequest.addListener(() => {}, { urls: ["wss://*/*"] });
    assert.equal(JSON.stringify(chrome.webRequest.onBeforeRequest.filters.at(-1)), JSON.stringify(["https://*/*"]), kind);

    await chrome.declarativeNetRequest.updateSessionRules({ addRules: [{ id: 1 }, { id: 2, custom: true }, { id: 3 }] });
    assert.equal(JSON.stringify(chrome.declarativeNetRequest.applied), JSON.stringify([1, 3]), kind);

    assert.equal(JSON.stringify(await chrome.tabs.query({ windowType: "app" })), JSON.stringify([]), kind);
    assert.equal((await chrome.tabs.query({ active: true })).length, 1, kind);

    class State {
      constructor() {
        this.vault = { locked: true };
      }
    }
    await chrome.storage.local.set(new State());
    assert.equal(JSON.stringify(chrome.storage.local.saved), JSON.stringify({ vault: { locked: true } }), kind);

    // A worker's first word to the browser is that it started.
    assert.equal(chrome.runtime.sent[0] === "worker.started", kind === "worker", kind);

    assert.equal(chrome.offscreen.Reason.CLIPBOARD, "CLIPBOARD", kind);
    assert.equal(await chrome.offscreen.hasDocument(), false, kind);

    const self = await chrome.management.getSelf();
    assert.equal(self.name, "Probe", kind);
    assert.equal(self.installType, "normal", kind);
    assert.equal(JSON.stringify(self.permissions), JSON.stringify(["tabs", "privacy", "notifications", "offscreen"]), kind);
    assert.equal(self.optionsUrl, "chrome-extension://abcdefghijklmnopabcdefghijklmnop/options.html", kind);
  }
});

test("silent authentication fails without a native call in promise and callback forms", async () => {
  for (const kind of ["page", "worker"]) {
    const { chrome } = context(kind);
    const before = chrome.runtime.sent.length;
    for (const details of [undefined, {}, { interactive: false }, { interactive: "true" }]) {
      await assert.rejects(chrome.identity.launchWebAuthFlow(details), /Non-interactive authentication is not supported/);
    }
    let callbackError;
    let callbackResult;
    assert.equal(chrome.identity.launchWebAuthFlow({ interactive: false }, result => {
      callbackResult = result;
      callbackError = chrome.runtime.lastError?.message;
    }), undefined);
    await Promise.resolve();
    await Promise.resolve();
    assert.equal(callbackResult, undefined);
    assert.match(callbackError, /Non-interactive authentication is not supported/);
    assert.equal(chrome.runtime.lastError, undefined);
    assert.equal(chrome.runtime.sent.length, before);
  }
});

test("interactive authentication retains the complete native redirect result", async () => {
  const { chrome } = context("page");
  const redirect = chrome.identity.getRedirectURL("cb") + "?code=synthetic&state=fixture#fragment";
  let request;
  chrome.runtime.sendNativeMessage = async (application, message) => {
    assert.equal(application, "app.zephium.webext");
    request = message;
    return { url: redirect };
  };
  assert.equal(await chrome.identity.launchWebAuthFlow({ interactive: true, url: "https://accounts.example/login" }), redirect);
  assert.equal(request.api, "identity.launch");
  assert.equal(request.interactive, true);
  assert.equal(request.url, "https://accounts.example/login");
});

test("workers present a Chrome identity and bridge WebSockets", () => {
  const g = context("worker");
  assert.match(g.navigator.userAgent, / Chrome\/\d+/);
  assert.equal(typeof g.WebSocket, "function");
});

test("content scripts keep the page's identity", () => {
  const g = context("content");
  assert.doesNotMatch(g.navigator.userAgent, / Chrome\//);
  assert.equal(typeof g.chrome.storage.managed, "object");
});

test("browser events subscribe, acknowledge and answer over one port", async () => {
  const { chrome } = context("worker");
  const states = [];
  chrome.idle.onStateChanged.addListener((state) => states.push(state));
  await new Promise((resolve) => setTimeout(resolve, 5));
  const port = chrome.runtime.ports.find((p) => p.application === "app.zephium.events");
  assert.ok(port, "a port opens for the first listener");
  assert.equal(port.posted[0].op, "hello");
  port.deliver({ op: "hello" });
  const subscribe = port.posted.find((m) => m.op === "subscribe");
  assert.equal(JSON.stringify([subscribe.worker, subscribe.events]), JSON.stringify([true, ["idle.onStateChanged"]]));
  assert.ok(chrome.runtime.sent.includes("idle.watch"), "listening starts the browser's watch");

  port.deliver({ op: "event", name: "idle.onStateChanged", args: ["idle"], seq: 7 });
  assert.equal(JSON.stringify(states), JSON.stringify(["idle"]));
  assert.ok(port.posted.some((m) => m.op === "ack" && m.seq === 7));

  port.deliver({ op: "event", name: "permissions.onAdded", args: [{}], seq: 8, reply: true });
  assert.ok(port.posted.some((m) => m.op === "reply" && m.seq === 8 && m.value === null), "unheard events answer null");
});

test("the API globals survive an extension hiding them", () => {
  const g = context("worker");
  const api = g.chrome;
  vm.runInContext('"use strict"; for (const name of ["chrome", "browser"]) globalThis[name] = new Proxy({}, { get: () => undefined });', g);
  assert.equal(g.chrome, api);
  assert.equal(g.browser, api);
});

test("filters and patterns WebKit can't read are narrowed, not refused", async () => {
  const { chrome } = context("worker");
  chrome.webRequest.onBeforeRequest.addListener(() => {}, { urls: ["<all_urls>"], types: ["script", "webbundle"] });
  assert.equal(JSON.stringify(chrome.webRequest.onBeforeRequest.types.at(-1)), JSON.stringify(["script"]));
  chrome.contextMenus.create({ id: "m", documentUrlPatterns: ["https://*/*", "ftp://*/*"] });
  assert.equal(JSON.stringify(chrome.contextMenus.created.at(-1).documentUrlPatterns), JSON.stringify(["https://*/*"]));
  assert.equal(await chrome.permissions.contains({ origins: ["http://localhost:8080/*"] }), true);
  assert.equal(chrome.declarativeNetRequest.RuleActionType.BLOCK, "block");
  assert.equal(chrome.identity.AccountStatus.ANY, "ANY");
});

test("API methods work apart from their namespace", async () => {
  const { chrome } = context("page");
  const { query } = chrome.tabs;
  const { getURL } = chrome.runtime;
  assert.equal((await query({ active: true })).length, 1);
  assert.match(getURL("x"), /\/x$/);
});

test("a page asks for the browser's permissions only during a click", async () => {
  const g = context("page");
  g.navigator.userActivation = { isActive: false };
  await assert.rejects(g.chrome.permissions.request({ permissions: ["idle"] }), /user gesture/);
  g.navigator.userActivation = { isActive: true };
  assert.equal(await g.chrome.permissions.request({ permissions: ["idle"] }), true);
});

test("a popup is in no tab, as in Chrome", async () => {
  const g = context("page");
  const { chrome } = g;
  assert.equal((await chrome.tabs.getCurrent()).id, 1);
  chrome.extension.popups.push(vm.runInContext("globalThis", g));
  assert.equal(await chrome.tabs.getCurrent(), undefined);
});

test("network rule updates keep what WebKit can apply", async () => {
  const { chrome } = context("worker");
  const net = chrome.declarativeNetRequest;
  await net.updateDynamicRules({ addRules: [{ id: 1 }, { id: 2, condition: { regexFilter: "^https://a(?=b)" } }, { id: 3, custom: true }, { id: 4 }] });
  assert.equal(JSON.stringify(net.applied), JSON.stringify([1, 4]));

  await net.updateEnabledRulesets({ disableRulesetIds: ["a"] });
  assert.equal(JSON.stringify(net.enabled), JSON.stringify([]));
  assert.equal(net.attempts, 3);
  net.enabled = ["a", "b"];
  net.broken = true;
  await assert.rejects(net.updateEnabledRulesets({ disableRulesetIds: ["a"] }), /Failed to apply rules/);
  assert.equal(net.attempts, 4);

  assert.equal(JSON.stringify(await net.getDisabledRuleIds({ rulesetId: "a" })), "[]");
  await net.updateStaticRules({ rulesetId: "a", enableRuleIds: [] });
  await assert.rejects(net.updateStaticRules({ rulesetId: "a", disableRuleIds: [1] }), /not supported/);
});

test("browser data namespaces exist only when declared and pass Chrome's arguments through", async () => {
  const plain = context("worker");
  assert.equal(plain.chrome.history, undefined);
  for (const kind of ["page", "worker"]) {
    const { chrome } = context(kind, ["history"]);
    assert.equal(chrome.history.TransitionType.LINK, "link", kind);
    const found = await chrome.history.search({ text: "a", maxResults: 5 });
    assert.equal(found[0].url, "https://a.example/", kind);
    const sent = chrome.runtime.messages.at(-1);
    assert.equal(JSON.stringify(sent), JSON.stringify({ api: "history.search", details: { text: "a", maxResults: 5 } }), kind);
    let called = false;
    chrome.history.deleteAll(() => (called = true));
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.ok(called, kind);
    assert.equal(chrome.runtime.messages.at(-1).api, "history.deleteAll", kind);
    assert.equal(typeof chrome.history.onVisited.addListener, "function", kind);
    assert.equal(typeof chrome.history.onVisitRemoved.hasListeners, "function", kind);
  }
  assert.equal(context("content", ["history"]).chrome.history, undefined, "not in content scripts");
});

test("closed tabs restore as the active tab chrome.tabs describes", async () => {
  assert.equal(context("page").chrome.sessions, undefined);
  const { chrome } = context("page", ["sessions"]);
  assert.equal((await chrome.sessions.getRecentlyClosed({ maxResults: 1 }))[0].tab.sessionId, "s1");
  assert.equal(JSON.stringify(chrome.runtime.messages.at(-1)), JSON.stringify({ api: "sessions.getRecentlyClosed", filter: { maxResults: 1 } }));
  let listed;
  chrome.sessions.getRecentlyClosed((sessions) => (listed = sessions.length));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(listed, 1);
  chrome.tabs.query = () => Promise.resolve([{ id: 7, url: "https://example.com/", title: "A", active: true }]);
  const restored = await chrome.sessions.restore("s2");
  assert.equal(restored.tab.id, 7);
  assert.equal(chrome.runtime.messages.at(-1).sessionId, "s2");
  assert.equal(JSON.stringify(await chrome.sessions.getDevices()), "[]");
  assert.equal(typeof chrome.sessions.onChanged.addListener, "function");
});

test("downloads pass options through and keep Chrome's gesture rule for opening", async () => {
  assert.equal(context("page").chrome.downloads, undefined);
  const { chrome } = context("page", ["downloads", "downloads.open"]);
  assert.equal(await chrome.downloads.download({ url: "https://a.example/f.zip", filename: "f.zip" }), 5);
  assert.equal(
    JSON.stringify(chrome.runtime.messages.at(-1)),
    JSON.stringify({ api: "downloads.download", details: { url: "https://a.example/f.zip", filename: "f.zip" } }),
  );
  assert.equal(JSON.stringify(await chrome.downloads.erase({ state: "complete" })), "[5]", "only finished downloads are erased");
  assert.equal(chrome.runtime.messages.at(-1).api, "downloads.erase");
  await assert.rejects(chrome.downloads.open(5), /User gesture required/);
  assert.equal(chrome.runtime.messages.at(-1).api, "downloads.erase", "no gesture, no call");
  await assert.rejects(chrome.downloads.removeFile(5));
  chrome.downloads.show(5);
  assert.equal(chrome.runtime.messages.at(-1).api, "downloads.show");
  for (const name of ["onCreated", "onChanged", "onErased"]) assert.equal(typeof chrome.downloads[name].addListener, "function");
});

test("top sites are read from the browser", async () => {
  assert.equal(context("worker").chrome.topSites, undefined);
  const { chrome } = context("worker", ["topSites"]);
  assert.equal((await chrome.topSites.get())[0].url, "https://a.example/");
  let heard;
  chrome.topSites.get((sites) => (heard = sites.length));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(heard, 1);
});

test("bookmarks pass ids and details through as Chrome takes them", async () => {
  assert.equal(context("page").chrome.bookmarks, undefined);
  const { chrome } = context("page", ["bookmarks"]);
  const made = await chrome.bookmarks.create({ parentId: "4", title: "T", url: "https://t.example/" });
  assert.equal(made.id, "9");
  assert.equal(
    JSON.stringify(chrome.runtime.messages.at(-1)),
    JSON.stringify({ api: "bookmarks.create", details: { parentId: "4", title: "T", url: "https://t.example/" } }),
  );
  await chrome.bookmarks.get("4");
  assert.equal(JSON.stringify(chrome.runtime.messages.at(-1)), JSON.stringify({ api: "bookmarks.get", ids: ["4"] }));
  await chrome.bookmarks.move("9", { index: 0 });
  assert.equal(JSON.stringify(chrome.runtime.messages.at(-1)), JSON.stringify({ api: "bookmarks.move", id: "9", details: { index: 0 } }));
  await chrome.bookmarks.search("docs");
  assert.equal(chrome.runtime.messages.at(-1).details, "docs");
  for (const name of ["onCreated", "onChanged", "onMoved", "onRemoved", "onChildrenReordered", "onImportBegan", "onImportEnded"]) {
    assert.equal(typeof chrome.bookmarks[name].addListener, "function", name);
  }
});
