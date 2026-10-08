// Runs the compatibility layer against stub extension contexts and checks
// that each fix is actually applied.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../src/compat/compat.js", import.meta.url), "utf8");
const event = () => ({ addListener() {}, removeListener() {}, hasListener: () => false });

function context(kind) {
  const invalid = (name) => new Error(`Invalid call. The 'permissions' value is invalid, because '${name}' is not a valid permission.`);
  const chrome = {
    runtime: {
      id: "abcdefghijklmnopabcdefghijklmnop",
      sent: [],
      async sendNativeMessage(_app, message) {
        this.sent.push(message.api);
        return message.api === "trace" ? false : null;
      },
      connectNative: (application) => {
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
        return port;
      },
      onMessage: event(),
      onConnect: event(),
      getManifest: () => ({ name: "__MSG_name__", version: "1.2", permissions: ["tabs", "https://a.test/*"], options_page: "options.html" }),
      getURL: (path) => `chrome-extension://abcdefghijklmnopabcdefghijklmnop/${path}`,
    },
    i18n: { getMessage: (key) => (key === "name" ? "Probe" : "") },
    webRequest: {
      onBeforeRequest: {
        addListener(_listener, filter) {
          if (filter.urls.some((url) => url.startsWith("ws"))) throw new Error("invalid match pattern");
          this.filters.push(filter.urls);
        },
        filters: [],
      },
    },
    declarativeNetRequest: {
      updateSessionRules(options) {
        const index = options.addRules.findIndex((rule) => rule.custom);
        if (index >= 0) throw new Error(`The 'addRules' value is invalid, because an error with rule at index ${index}: bad header.`);
        this.applied = options.addRules.map((rule) => rule.id);
        return Promise.resolve();
      },
    },
    permissions: {
      contains(request) {
        const unknown = (request.permissions || []).find((name) => name === "tabGroups");
        if (unknown) throw invalid(unknown);
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
    tabs: { onUpdated: event(), onRemoved: event(), sendMessage() {}, query: () => Promise.resolve([{ id: 1 }]) },
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
    assert.equal(await chrome.permissions.request({ permissions: ["privacy"] }), true, kind);
    assert.equal(await chrome.permissions.contains({ permissions: ["proxy"] }), false, kind);
    assert.equal(await chrome.permissions.contains({ permissions: ["tabGroups"] }), false, kind);
    assert.equal(Object.keys(await chrome.storage.managed.get()).length, 0, kind);
    assert.equal(typeof chrome.storage.managed.onChanged.addListener, "function", kind);
    assert.equal(chrome.scripting.ExecutionWorld.ISOLATED, "ISOLATED", kind);
    await chrome.scripting.registerContentScripts([{ id: "a" }]);
    assert.equal(typeof chrome.webNavigation.onHistoryStateUpdated.addListener, "function", kind);
    assert.equal(typeof chrome.notifications.create, "function", kind);
    assert.match(chrome.identity.getRedirectURL("cb"), /^https:\/\/abcdefghijklmnopabcdefghijklmnop\.chromiumapp\.org\/cb$/);
    const saving = await chrome.privacy.services.passwordSavingEnabled.get({});
    assert.equal(saving.value, false, kind);
    assert.equal(saving.levelOfControl, "controlled_by_this_extension", kind);
    await chrome.privacy.services.passwordSavingEnabled.set({ value: true });
    assert.equal((await chrome.privacy.services.passwordSavingEnabled.get({})).value, true, kind);

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
    assert.equal(JSON.stringify(self.permissions), JSON.stringify(["tabs"]), kind);
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
