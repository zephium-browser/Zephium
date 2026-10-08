// Zephium's compatibility layer for Chrome extensions on WebKit. It runs first
// in every extension context and defines only what WebKit lacks or gets wrong,
// so anything WebKit later implements properly takes precedence.
(() => {
  "use strict";
  const g = globalThis;
  const KEY = Symbol.for("zephium.compat");
  if (g[KEY]) return;
  const chromeApi = g.chrome;
  const runtime = chromeApi && chromeApi.runtime;
  // Absent in page-world scripts and sandboxed pages: nothing to adapt.
  if (!runtime || !runtime.id) return;

  const CHROME_VERSION = "152.0.0.0";
  const isWorker =
    typeof ServiceWorkerGlobalScope !== "undefined" && g instanceof ServiceWorkerGlobalScope;
  const isPage =
    !isWorker &&
    typeof location !== "undefined" &&
    (location.protocol === "chrome-extension:" || location.protocol === "webkit-extension:");
  const isContent = !isWorker && !isPage;
  const config = typeof __zephiumConfig === "object" && __zephiumConfig ? __zephiumConfig : {};
  const Z = { isWorker, isPage, isContent, config };
  Object.defineProperty(g, KEY, { value: Z });

  // WebKit prefixes the browser's errors with its own; extensions compare
  // against Chrome's text.
  const native = (api, fields) =>
    runtime.sendNativeMessage("app.zephium.webext", Object.assign({ api }, fields)).catch((error) => {
      throw new Error(String((error && error.message) || error).replace(/^Invalid call to runtime\.sendNativeMessage\(\)\. /, ""));
    });

  // A new worker means the previous one is gone, along with whatever it had
  // open through the browser; this is the first thing it sends.
  if (isWorker && typeof runtime.sendNativeMessage === "function") {
    try {
      native("worker.started").catch(() => {});
    } catch {}
  }

  // ---- Diagnostics ---------------------------------------------------------
  // Errors inside workers and extension pages are otherwise invisible to the
  // browser; development builds report them, bounded, so failures have a
  // cause on record.
  if (config.diagnostics && !isContent && typeof runtime.sendNativeMessage === "function") {
    let budget = 60;
    let since = 0;
    const describe = (value) => {
      if (value instanceof Error) return value.stack ? `${value}\n${value.stack}` : String(value);
      if (typeof value === "object" && value !== null) {
        try {
          return JSON.stringify(value).slice(0, 500);
        } catch {
          return Object.prototype.toString.call(value);
        }
      }
      return String(value);
    };
    const where = isWorker ? "worker" : location.pathname;
    const report = (level, text) => {
      const now = Date.now();
      if (now - since > 10000) {
        since = now;
        budget = 60;
      }
      if (budget-- <= 0) return;
      try {
        native("log", { level, text: `[${where}] ${text}` }).catch(() => {});
      } catch {}
    };
    Z.report = report;
    g.addEventListener("error", (event) => {
      const origin = event.filename ? ` @ ${event.filename}:${event.lineno}:${event.colno}` : "";
      report("error", `${event.message}${origin}${event.error && event.error.stack ? "\n" + event.error.stack : ""}`);
    });
    g.addEventListener("unhandledrejection", (event) => {
      report("error", `unhandled rejection: ${describe(event.reason)}`);
    });
    const consoleError = console.error;
    console.error = function (...args) {
      report("error", args.map(describe).join(" "));
      return consoleError.apply(this, args);
    };
    const consoleWarn = console.warn;
    console.warn = function (...args) {
      report("warning", args.map(describe).join(" "));
      return consoleWarn.apply(this, args);
    };
  }

  // ---- Language gaps -------------------------------------------------------
  if (typeof g.scheduler !== "object" || g.scheduler === null) {
    try {
      Object.defineProperty(g, "scheduler", { value: {}, configurable: true, writable: true });
    } catch {}
  }
  if (g.scheduler && typeof g.scheduler.yield !== "function") {
    g.scheduler.yield = () => new Promise((resolve) => setTimeout(resolve, 0));
  }
  if (typeof Symbol.dispose !== "symbol") {
    Object.defineProperty(Symbol, "dispose", { value: Symbol.for("Symbol.dispose") });
  }
  if (typeof Symbol.asyncDispose !== "symbol") {
    Object.defineProperty(Symbol, "asyncDispose", { value: Symbol.for("Symbol.asyncDispose") });
  }

  // ---- Browser identity ----------------------------------------------------
  // The native user agent must stay identical to web tabs' (see the runtime),
  // so extensions are told they run in Chrome here. Content scripts share the
  // page's navigator and keep the real value.
  if (!isContent && !/ Chrome\//.test(navigator.userAgent)) {
    const base = navigator.userAgent.replace(/ Version\/\S+/, "").replace(/ Safari\/\S+$/, "");
    const chromeAgent = `${base} Chrome/${CHROME_VERSION} Safari/537.36`;
    const major = CHROME_VERSION.split(".")[0];
    const brands = [
      { brand: "Chromium", version: major },
      { brand: "Google Chrome", version: major },
      { brand: "Not=A?Brand", version: "24" },
    ];
    const platformVersion = (/Mac OS X (\d+)[_.](\d+)/.exec(navigator.userAgent) || []).slice(1).join(".") || "15.0";
    const agentData = {
      brands,
      mobile: false,
      platform: "macOS",
      getHighEntropyValues: async (hints) => {
        const values = {
          brands,
          mobile: false,
          platform: "macOS",
          architecture: "arm",
          bitness: "64",
          model: "",
          platformVersion,
          uaFullVersion: CHROME_VERSION,
          fullVersionList: brands.map((b) => ({ brand: b.brand, version: b.brand === "Not=A?Brand" ? "24.0.0.0" : CHROME_VERSION })),
        };
        const result = { brands, mobile: false, platform: "macOS" };
        for (const hint of hints || []) if (hint in values) result[hint] = values[hint];
        return result;
      },
      toJSON() {
        return { brands, mobile: false, platform: "macOS" };
      },
    };
    const proto = Object.getPrototypeOf(navigator);
    const define = (name, value) => {
      try {
        Object.defineProperty(proto, name, { configurable: true, get: () => value });
      } catch {}
    };
    define("userAgent", chromeAgent);
    define("appVersion", chromeAgent.replace(/^Mozilla\//, ""));
    define("vendor", "Google Inc.");
    define("userAgentData", agentData);
  }

  // ---- API gaps ------------------------------------------------------------
  // WebKit recreates its API wrapper objects after they are collected, which
  // would drop anything defined on them: patched namespaces stay referenced
  // and pinned as data properties.
  const kept = [];
  const pin = (target, key, value) => {
    try {
      Object.defineProperty(target, key, { value, configurable: true, enumerable: true, writable: true });
      return true;
    } catch {
      return false;
    }
  };
  pin(g, "chrome", chromeApi);
  // WebKit gives `browser` its own copy of the API; making it the same object
  // lets every fix below reach extensions written against `browser.*`. Both
  // accept callbacks and return promises.
  if (g.browser && g.browser !== chromeApi) {
    kept.push(g.browser);
    pin(g, "browser", chromeApi);
  }
  const namespace = (name) => {
    let value;
    try {
      value = chromeApi[name];
    } catch {
      return undefined;
    }
    if (value) {
      kept.push(value);
      pin(chromeApi, name, value);
    }
    return value;
  };
  const withCallback = (promise, callback) => {
    if (typeof callback !== "function") return promise;
    promise.then(
      (value) => callback(value),
      (error) => {
        if (Z.report) Z.report("warning", `callback API failed: ${error}`);
        // Chrome exposes callback failures only during the callback. Keep the
        // previous descriptor intact for nested calls and native WebKit APIs.
        const previous = Object.getOwnPropertyDescriptor(runtime, "lastError");
        let exposed = false;
        try {
          Object.defineProperty(runtime, "lastError", {
            configurable: true,
            value: { message: String((error && error.message) || error) },
          });
          exposed = true;
        } catch {}
        try {
          callback(undefined);
        } finally {
          if (exposed) {
            if (previous) Object.defineProperty(runtime, "lastError", previous);
            else delete runtime.lastError;
          }
        }
      },
    );
  };

  const makeEvent = () => {
    const listeners = new Set();
    return {
      addListener: (listener) => void listeners.add(listener),
      removeListener: (listener) => void listeners.delete(listener),
      hasListener: (listener) => listeners.has(listener),
      hasListeners: () => listeners.size > 0,
      dispatch: (...args) => {
        for (const listener of listeners) {
          try {
            listener(...args);
          } catch (error) {
            setTimeout(() => {
              throw error;
            });
          }
        }
      },
    };
  };

  // Chrome answers "not granted" for permissions a browser doesn't know;
  // WebKit throws, which takes down callers such as Bitwarden's popup. The
  // retry stays synchronous so a request keeps the user's gesture.
  const permissions = namespace("permissions");
  if (permissions) {
    const invalid = /'([^']+)' is not a valid permission/;
    // WebKit rejects some unknown names synchronously and others only in the
    // returned promise; names learned either way are filtered up front.
    const unknownNames = new Set(["proxy", "debugger"]);
    // Implemented by this layer, so always granted.
    const emulated = new Set(["privacy"]);
    const guard = (method, withUnknown) => {
      const original = permissions[method];
      if (typeof original !== "function") return;
      pin(permissions, method, function (request, callback) {
        const current = Object.assign({}, request);
        const requested = Array.isArray(current.permissions) ? current.permissions : [];
        let unknown = requested.some((name) => unknownNames.has(name));
        const onlyEmulated = requested.some((name) => emulated.has(name));
        current.permissions = requested.filter((name) => !unknownNames.has(name) && !emulated.has(name));
        let result;
        for (;;) {
          const listed = Array.isArray(current.permissions) ? current.permissions : [];
          const origins = Array.isArray(current.origins) ? current.origins : [];
          if ((unknown || onlyEmulated) && listed.length === 0 && origins.length === 0) {
            result = Promise.resolve(true);
            break;
          }
          try {
            result = Promise.resolve(original.call(permissions, current));
            break;
          } catch (error) {
            const name = (invalid.exec(String(error && error.message)) || [])[1];
            if (!name || !listed.includes(name)) {
              result = Promise.reject(error);
              break;
            }
            unknown = true;
            unknownNames.add(name);
            current.permissions = listed.filter((permission) => permission !== name);
          }
        }
        result = result.catch((error) => {
          const name = (invalid.exec(String(error && error.message)) || [])[1];
          if (!name) throw error;
          unknownNames.add(name);
          return withUnknown();
        });
        return withCallback(unknown ? result.then(withUnknown) : result, callback);
      });
    };
    guard("contains", () => false);
    guard("request", () => false);
    guard("remove", (removed) => removed !== false);
  }

  // OAuth sign-in: the browser opens the provider's page and returns the
  // https://<id>.chromiumapp.org/ redirect, which never actually loads.
  if (!isContent) {
    const identity = namespace("identity");
    const target = identity || {};
    if (typeof target.launchWebAuthFlow !== "function") {
      pin(target, "getRedirectURL", (path) =>
        `https://${runtime.id}.chromiumapp.org/${String(path || "").replace(/^\//, "")}`,
      );
      pin(target, "launchWebAuthFlow", (details, callback) =>
        withCallback(
          !(details && details.interactive === true)
            ? Promise.reject(new Error("Non-interactive authentication is not supported."))
            : native("identity.launch", {
            url: String((details && details.url) || ""),
            interactive: Boolean(details && details.interactive),
          }).then((result) => {
            if (!result || typeof result.url !== "string") throw new Error("The user did not approve access.");
            return result.url;
          }),
          callback,
        ),
      );
      if (typeof target.getAuthToken !== "function") {
        pin(target, "getAuthToken", (_details, callback) =>
          withCallback(Promise.reject(new Error("The user is not signed in to a Google account in this browser.")), callback),
        );
        pin(target, "removeCachedAuthToken", (_details, callback) => withCallback(Promise.resolve(), callback));
        pin(target, "clearAllCachedAuthTokens", (callback) => withCallback(Promise.resolve(), callback));
        pin(target, "getProfileUserInfo", (_details, callback) =>
          withCallback(Promise.resolve({ email: "", id: "" }), typeof _details === "function" ? _details : callback),
        );
        if (!target.onSignInChanged) pin(target, "onSignInChanged", makeEvent());
      }
      if (!identity) pin(chromeApi, "identity", target);
    }
  }

  // Password managers turn off the browser's own password saving through
  // chrome.privacy; WebKit has no privacy API. The settings answer as
  // controllable by the extension and remember what it sets.
  if (!isContent && !chromeApi.privacy) {
    const setting = (initial, levelOfControl = "controllable_by_this_extension") => {
      let value = initial;
      const changed = makeEvent();
      const details = () => ({ value, levelOfControl });
      return {
        get: (_details, callback) => withCallback(Promise.resolve(details()), callback),
        set: (next, callback) => {
          if (next && "value" in next) value = next.value;
          return withCallback(Promise.resolve(), callback);
        },
        clear: (_details, callback) => {
          value = initial;
          return withCallback(Promise.resolve(), callback);
        },
        onChange: changed,
      };
    };
    pin(chromeApi, "privacy", {
      services: {
        // Zephium has no password or form store of its own, so these are off
        // and left to the extension, as when it has overridden Chrome's.
        passwordSavingEnabled: setting(false, "controlled_by_this_extension"),
        autofillEnabled: setting(false, "controlled_by_this_extension"),
        autofillAddressEnabled: setting(false, "controlled_by_this_extension"),
        autofillCreditCardEnabled: setting(false, "controlled_by_this_extension"),
        alternateErrorPagesEnabled: setting(false),
        safeBrowsingEnabled: setting(false),
        searchSuggestEnabled: setting(true),
        spellingServiceEnabled: setting(false),
        translationServiceEnabled: setting(false),
      },
      network: {
        networkPredictionEnabled: setting(true),
        webRTCIPHandlingPolicy: setting("default"),
      },
      websites: {
        thirdPartyCookiesAllowed: setting(true),
        hyperlinkAuditingEnabled: setting(false),
        referrersEnabled: setting(true),
        doNotTrackEnabled: setting(false),
        protectedContentEnabled: setting(true),
      },
    });
  }

  // Chrome always has the enterprise-policy storage area, empty when no
  // policy is set; 1Password and Grammarly read it at startup.
  const storage = namespace("storage");
  if (storage && !storage.managed) {
    pin(storage, "managed", {
      get: (keys, callback) => withCallback(Promise.resolve({}), typeof keys === "function" ? keys : callback),
      getBytesInUse: (keys, callback) => withCallback(Promise.resolve(0), typeof keys === "function" ? keys : callback),
      set: () => Promise.reject(new Error("storage.managed is read-only")),
      remove: () => Promise.reject(new Error("storage.managed is read-only")),
      clear: () => Promise.reject(new Error("storage.managed is read-only")),
      onChanged: makeEvent(),
    });
  }

  // Chrome serializes whatever object it's given; WebKit accepts only plain
  // ones, rejecting class instances and proxies such as MetaMask's state.
  if (storage) {
    for (const name of ["local", "session", "sync"]) {
      const area = storage[name];
      if (!area || typeof area.set !== "function") continue;
      kept.push(area);
      const set = area.set;
      pin(area, "set", function (items, callback) {
        const retry = (error) => {
          if (!/an object is expected/.test(String(error && error.message))) throw error;
          return set.call(area, JSON.parse(JSON.stringify(items)));
        };
        let result;
        try {
          result = Promise.resolve(set.call(area, items)).catch(retry);
        } catch (error) {
          result = Promise.resolve().then(() => retry(error));
        }
        return withCallback(result, callback);
      });
    }
  }

  const scripting = namespace("scripting");
  if (scripting && !scripting.ExecutionWorld) {
    pin(scripting, "ExecutionWorld", Object.freeze({ ISOLATED: "ISOLATED", MAIN: "MAIN" }));
  }
  // WebKit keeps dynamically registered scripts across restarts, which Chrome
  // extensions registering at every startup don't expect.
  if (scripting && typeof scripting.registerContentScripts === "function" && typeof scripting.updateContentScripts === "function") {
    const register = scripting.registerContentScripts;
    pin(scripting, "registerContentScripts", function (scripts, callback) {
      const attempt = register.call(scripting, scripts).catch((error) => {
        if (!/Duplicate ID/.test(String(error && error.message))) throw error;
        return scripting.updateContentScripts(scripts);
      });
      return withCallback(attempt, callback);
    });
  }

  // WebKit implements webNavigation's load events but not these; code that
  // subscribes to them at startup would otherwise throw and kill the worker.
  const webNavigation = namespace("webNavigation");
  if (webNavigation) {
    const added = {};
    for (const name of ["onHistoryStateUpdated", "onReferenceFragmentUpdated", "onCreatedNavigationTarget", "onTabReplaced"]) {
      if (!webNavigation[name]) {
        added[name] = makeEvent();
        pin(webNavigation, name, added[name]);
      }
    }
    const used = (event) => (config.events || []).includes(`webNavigation.${event}`);
    const tabs = chromeApi.tabs;
    // Same-document navigations change a tab's URL without a commit. Only
    // extensions that listen for them pay for following every tab update.
    if (
      isWorker &&
      tabs &&
      webNavigation.onCommitted &&
      ((added.onHistoryStateUpdated && used("onHistoryStateUpdated")) ||
        (added.onReferenceFragmentUpdated && used("onReferenceFragmentUpdated")))
    ) {
      const committed = new Map();
      webNavigation.onCommitted.addListener((details) => {
        if (details.frameId === 0) committed.set(details.tabId, details.url);
      });
      tabs.onRemoved.addListener((tabId) => committed.delete(tabId));
      tabs.onUpdated.addListener((tabId, change) => {
        if (!change.url) return;
        const previous = committed.get(tabId);
        committed.set(tabId, change.url);
        if (previous === undefined || previous === change.url) return;
        const details = {
          tabId,
          url: change.url,
          frameId: 0,
          parentFrameId: -1,
          processId: -1,
          timeStamp: Date.now(),
          transitionType: "link",
          transitionQualifiers: [],
        };
        const fragmentOnly = previous.split("#")[0] === change.url.split("#")[0];
        const event = fragmentOnly ? added.onReferenceFragmentUpdated : added.onHistoryStateUpdated;
        if (event) event.dispatch(details);
      });
    }
  }

  if (!isContent && !chromeApi.notifications) {
    const event = makeEvent;
    let created = 0;
    pin(chromeApi, "notifications", {
      TemplateType: Object.freeze({ BASIC: "basic", IMAGE: "image", LIST: "list", PROGRESS: "progress" }),
      PermissionLevel: Object.freeze({ GRANTED: "granted", DENIED: "denied" }),
      create(id, options, callback) {
        if (typeof id === "object" && id !== null) {
          callback = options;
          options = id;
          id = undefined;
        }
        const name = typeof id === "string" && id ? id : `zephium-${++created}`;
        const shown = native("notify", {
          id: name,
          title: String((options && options.title) || ""),
          message: String((options && options.message) || ""),
        })
          .catch(() => {})
          .then(() => name);
        return withCallback(shown, callback);
      },
      update: (_id, _options, callback) => withCallback(Promise.resolve(false), callback),
      clear: (_id, callback) => withCallback(Promise.resolve(true), callback),
      getAll: (callback) => withCallback(Promise.resolve({}), callback),
      getPermissionLevel: (callback) => withCallback(Promise.resolve("granted"), callback),
      onClicked: event(),
      onClosed: event(),
      onButtonClicked: event(),
      onPermissionLevelChanged: event(),
      onShowSettings: event(),
    });
  }

  // Constants Chrome always defines. Extensions read them at startup, before
  // registering listeners, so a missing one takes the whole worker down.
  const constants = (target, values) => {
    if (!target) return;
    for (const [name, value] of Object.entries(values)) {
      if (target[name] === undefined) pin(target, name, Object.freeze(value));
    }
  };
  const chromeRuntime = namespace("runtime");
  constants(chromeRuntime, {
    OnInstalledReason: { INSTALL: "install", UPDATE: "update", CHROME_UPDATE: "chrome_update", SHARED_MODULE_UPDATE: "shared_module_update" },
    OnRestartRequiredReason: { APP_UPDATE: "app_update", OS_UPDATE: "os_update", PERIODIC: "periodic" },
    PlatformOs: { MAC: "mac", WIN: "win", ANDROID: "android", CROS: "cros", LINUX: "linux", OPENBSD: "openbsd", FUCHSIA: "fuchsia" },
    PlatformArch: { ARM: "arm", ARM64: "arm64", X86_32: "x86-32", X86_64: "x86-64", MIPS: "mips", MIPS64: "mips64", RISCV64: "riscv64" },
    PlatformNaclArch: { ARM: "arm", X86_32: "x86-32", X86_64: "x86-64", MIPS: "mips", MIPS64: "mips64" },
    RequestUpdateCheckStatus: { THROTTLED: "throttled", NO_UPDATE: "no_update", UPDATE_AVAILABLE: "update_available" },
    ContextType: { TAB: "TAB", POPUP: "POPUP", BACKGROUND: "BACKGROUND", OFFSCREEN_DOCUMENT: "OFFSCREEN_DOCUMENT", SIDE_PANEL: "SIDE_PANEL", DEVELOPER_TOOLS: "DEVELOPER_TOOLS" },
  });
  if (chromeRuntime && !isContent) {
    // Updates are applied by the browser, which reloads the extension.
    for (const name of ["onUpdateAvailable", "onRestartRequired", "onBrowserUpdateAvailable"]) {
      if (!chromeRuntime[name]) pin(chromeRuntime, name, makeEvent());
    }
    if (typeof chromeRuntime.requestUpdateCheck !== "function") {
      pin(chromeRuntime, "requestUpdateCheck", (callback) => withCallback(Promise.resolve({ status: "no_update" }), callback));
    }
  }

  const webRequest = !isContent && namespace("webRequest");
  if (webRequest) {
    const headers = { REQUEST_HEADERS: "requestHeaders", RESPONSE_HEADERS: "responseHeaders" };
    const BLOCKING = "blocking";
    const EXTRA_HEADERS = "extraHeaders";
    constants(webRequest, {
      OnBeforeRequestOptions: { BLOCKING, REQUEST_BODY: "requestBody", EXTRA_HEADERS },
      OnBeforeSendHeadersOptions: { REQUEST_HEADERS: headers.REQUEST_HEADERS, BLOCKING, EXTRA_HEADERS },
      OnSendHeadersOptions: { REQUEST_HEADERS: headers.REQUEST_HEADERS, EXTRA_HEADERS },
      OnHeadersReceivedOptions: { BLOCKING, RESPONSE_HEADERS: headers.RESPONSE_HEADERS, EXTRA_HEADERS },
      OnAuthRequiredOptions: { RESPONSE_HEADERS: headers.RESPONSE_HEADERS, BLOCKING, ASYNC_BLOCKING: "asyncBlocking", EXTRA_HEADERS },
      OnResponseStartedOptions: { RESPONSE_HEADERS: headers.RESPONSE_HEADERS, EXTRA_HEADERS },
      OnBeforeRedirectOptions: { RESPONSE_HEADERS: headers.RESPONSE_HEADERS, EXTRA_HEADERS },
      OnCompletedOptions: { RESPONSE_HEADERS: headers.RESPONSE_HEADERS, EXTRA_HEADERS },
      OnErrorOccurredOptions: { EXTRA_HEADERS },
      ResourceType: {
        MAIN_FRAME: "main_frame", SUB_FRAME: "sub_frame", STYLESHEET: "stylesheet", SCRIPT: "script", IMAGE: "image",
        FONT: "font", OBJECT: "object", XMLHTTPREQUEST: "xmlhttprequest", PING: "ping", CSP_REPORT: "csp_report",
        MEDIA: "media", WEBSOCKET: "websocket", WEBBUNDLE: "webbundle", OTHER: "other",
      },
    });
    // WebKit doesn't parse ws:// and wss:// patterns and rejects the whole
    // filter; it doesn't report WebSocket requests either way.
    const socket = /^wss?:/i;
    for (const name of Object.keys(webRequest)) {
      const event = webRequest[name];
      if (!/^on[A-Z]/.test(name) || !event || typeof event.addListener !== "function") continue;
      kept.push(event);
      const add = event.addListener;
      pin(event, "addListener", function (listener, filter, ...rest) {
        const urls = filter && Array.isArray(filter.urls) ? filter.urls : null;
        if (!urls || !urls.some((url) => socket.test(url))) return add.call(this, listener, filter, ...rest);
        const usable = urls.filter((url) => !socket.test(url));
        if (usable.length === 0) return undefined;
        return add.call(this, listener, Object.assign({}, filter, { urls: usable }), ...rest);
      });
    }
  }

  // WebKit rejects a whole rule update over one rule it can't apply, such as
  // one setting a header it doesn't know. Chrome would apply the rest.
  const netRequest = !isContent && namespace("declarativeNetRequest");
  if (netRequest) {
    const rejected = /rule at index (\d+)/;
    for (const method of ["updateSessionRules", "updateDynamicRules"]) {
      const original = netRequest[method];
      if (typeof original !== "function") continue;
      pin(netRequest, method, function (options, callback) {
        let current = Object.assign({}, options);
        const run = (attempts) => {
          let result;
          try {
            result = Promise.resolve(original.call(netRequest, current));
          } catch (error) {
            result = Promise.reject(error);
          }
          return result.catch((error) => {
            const index = Number((rejected.exec(String(error && error.message)) || [])[1]);
            const rules = Array.isArray(current.addRules) ? current.addRules : [];
            if (!(index < rules.length) || attempts <= 0) throw error;
            if (Z.report) Z.report("warning", `skipped a network rule WebKit can't apply: ${error.message}`);
            current = Object.assign({}, current, { addRules: rules.filter((_, i) => i !== index) });
            return run(attempts - 1);
          });
        };
        return withCallback(run(50), callback);
      });
    }
  }

  // Only normal and popup windows exist here; Chrome answers other types
  // with no tabs, WebKit throws.
  const tabsApi = !isContent && namespace("tabs");
  if (tabsApi && typeof tabsApi.query === "function") {
    const query = tabsApi.query;
    pin(tabsApi, "query", function (info, callback) {
      const type = info && info.windowType;
      if (type !== undefined && type !== "normal" && type !== "popup") return withCallback(Promise.resolve([]), callback);
      return query.apply(this, arguments);
    });
  }

  // WebKit declares tabs.detectLanguage but reports it unimplemented; the
  // page's own declared language answers most of what translators ask.
  if (tabsApi && chromeApi.scripting && typeof chromeApi.scripting.executeScript === "function") {
    const detect = async (tabId) => {
      if (typeof tabId !== "number") {
        const [active] = await tabsApi.query({ active: true, currentWindow: true });
        tabId = active && active.id;
      }
      const [frame] = await chromeApi.scripting.executeScript({
        target: { tabId },
        func: () => document.documentElement.lang || (document.querySelector('meta[http-equiv="content-language" i]') || {}).content || "",
      });
      const tag = String((frame && frame.result) || "").split(",")[0].trim();
      return tag ? tag.split(/[-_]/)[0].toLowerCase() : "und";
    };
    pin(tabsApi, "detectLanguage", function (tabId, callback) {
      if (typeof tabId === "function") [tabId, callback] = [undefined, tabId];
      return withCallback(detect(tabId).catch(() => "und"), callback);
    });
  }

  // Extensions check how they were installed and read their own details.
  if (!isContent) {
    const management = namespace("management") || {};
    if (typeof management.getSelf !== "function") {
      const self = () => {
        const manifest = chromeRuntime.getManifest();
        const text = (value) => {
          const key = /^__MSG_(\w+)__$/.exec(String(value || ""));
          return key && chromeApi.i18n ? chromeApi.i18n.getMessage(key[1]) || "" : String(value || "");
        };
        const options = (manifest.options_ui && manifest.options_ui.page) || manifest.options_page;
        const icons = Object.entries(manifest.icons || {}).map(([size, path]) => ({ size: Number(size), url: chromeRuntime.getURL(path) }));
        return {
          id: chromeRuntime.id,
          name: text(manifest.name),
          shortName: text(manifest.short_name || manifest.name),
          description: text(manifest.description),
          version: manifest.version,
          versionName: manifest.version_name,
          mayDisable: true,
          mayEnable: true,
          enabled: true,
          isApp: false,
          type: "extension",
          installType: "normal",
          offlineEnabled: Boolean(manifest.offline_enabled),
          homepageUrl: manifest.homepage_url || "",
          updateUrl: manifest.update_url || "",
          optionsUrl: options ? chromeRuntime.getURL(options) : "",
          permissions: (manifest.permissions || []).filter((name) => !name.includes("://") && name !== "<all_urls>"),
          hostPermissions: manifest.host_permissions || [],
          icons,
        };
      };
      pin(management, "getSelf", (callback) => withCallback(Promise.resolve().then(self), callback));
      constants(management, {
        ExtensionInstallType: { ADMIN: "admin", DEVELOPMENT: "development", NORMAL: "normal", SIDELOAD: "sideload", OTHER: "other" },
        ExtensionType: { EXTENSION: "extension", HOSTED_APP: "hosted_app", PACKAGED_APP: "packaged_app", LEGACY_PACKAGED_APP: "legacy_packaged_app", THEME: "theme", LOGIN_SCREEN_EXTENSION: "login_screen_extension" },
        ExtensionDisabledReason: { UNKNOWN: "unknown", PERMISSIONS_INCREASE: "permissions_increase" },
      });
      if (!chromeApi.management) pin(chromeApi, "management", management);
    }
  }
  Z.kept = kept;

  // WebKit serves packaged .wasm without the application/wasm type that the
  // streaming compilers require.
  if (typeof WebAssembly === "object" && typeof WebAssembly.instantiateStreaming === "function") {
    const packaged = (response) =>
      response && typeof response.url === "string" && /^(chrome|webkit)-extension:/.test(response.url);
    const instantiate = WebAssembly.instantiateStreaming;
    const compile = WebAssembly.compileStreaming;
    WebAssembly.instantiateStreaming = async function (source, imports) {
      const response = await source;
      return packaged(response)
        ? WebAssembly.instantiate(await response.arrayBuffer(), imports)
        : instantiate.call(this, response, imports);
    };
    WebAssembly.compileStreaming = async function (source) {
      const response = await source;
      return packaged(response)
        ? WebAssembly.compile(await response.arrayBuffer())
        : compile.call(this, response);
    };
  }

  // ---- Tracing ---------------------------------------------------------------
  // Development builds started with ZEPHIUM_WEBEXT_TRACE=1 record the names of
  // messages and ports the worker handles, never their content. Listeners
  // are wrapped at startup, when extensions register them.
  if (isWorker && Z.report) {
    let tracing = false;
    native("trace", {})
      .then((enabled) => (tracing = enabled === true))
      .catch(() => {});
    const label = (message) =>
      message && typeof message === "object"
        ? String(message.command || message.type || message.name || Object.keys(message)[0] || "?").slice(0, 60)
        : typeof message;
    const origin = (sender) =>
      sender && sender.tab
        ? `tab ${sender.tab.id} frame ${sender.frameId}`
        : sender && sender.url
          ? sender.url.replace(/^[a-z-]+:\/\/[^/]+/, "")
          : "?";
    // Each listener sees the same delivery; report it once.
    let lastReported = "";
    const reportOnce = (text) => {
      if (text === lastReported) return;
      lastReported = text;
      setTimeout(() => (lastReported = ""), 0);
      Z.report("info", text);
    };
    const traced = (event, describe) => {
      if (!event || typeof event.addListener !== "function") return;
      kept.push(event);
      const wrappers = new WeakMap();
      const add = event.addListener;
      const remove = event.removeListener;
      const has = event.hasListener;
      pin(event, "addListener", function (listener, ...rest) {
        if (typeof listener !== "function") return add.call(this, listener, ...rest);
        let wrapper = wrappers.get(listener);
        if (!wrapper) {
          wrapper = function (...args) {
            if (tracing) reportOnce(describe(...args));
            return listener.apply(this, args);
          };
          wrappers.set(listener, wrapper);
        }
        return add.call(this, wrapper, ...rest);
      });
      pin(event, "removeListener", function (listener) {
        return remove.call(this, wrappers.get(listener) || listener);
      });
      pin(event, "hasListener", function (listener) {
        return has.call(this, wrappers.get(listener) || listener);
      });
    };
    namespace("runtime");
    traced(runtime.onMessage, (message, sender) => `message ${label(message)} from ${origin(sender)}`);
    const watched = new WeakSet();
    traced(runtime.onConnect, (port) => {
      if (port && !watched.has(port)) {
        watched.add(port);
        const opened = Date.now();
        try {
          port.onDisconnect.addListener(() =>
            Z.report("info", `port ${port.name} disconnected after ${Date.now() - opened}ms`),
          );
        } catch {}
      }
      return `port ${port && port.name} from ${origin(port && port.sender)}`;
    });
    const tabs = namespace("tabs");
    if (tabs && typeof tabs.sendMessage === "function") {
      const send = tabs.sendMessage;
      pin(tabs, "sendMessage", function (tabId, message, ...rest) {
        if (tracing) Z.report("info", `tabs.sendMessage ${label(message)} to tab ${tabId}`);
        return send.call(this, tabId, message, ...rest);
      });
    }
  }

  // ---- Offscreen documents and the clipboard -------------------------------
  if (!isContent && !chromeApi.offscreen) {
    const reasons = [
      "TESTING", "AUDIO_PLAYBACK", "IFRAME_SCRIPTING", "DOM_SCRAPING", "BLOBS", "DOM_PARSER", "USER_MEDIA",
      "DISPLAY_MEDIA", "WEB_RTC", "CLIPBOARD", "LOCAL_STORAGE", "WORKERS", "BATTERY_STATUS", "MATCH_MEDIA",
      "GEOLOCATION",
    ];
    pin(chromeApi, "offscreen", {
      Reason: Object.freeze(Object.fromEntries(reasons.map((reason) => [reason, reason]))),
      createDocument: (parameters, callback) =>
        withCallback(
          native("offscreen.create", { url: String((parameters && parameters.url) || "") }).then(() => undefined),
          callback,
        ),
      closeDocument: (callback) =>
        withCallback(
          native("offscreen.close", {}).then((closed) => {
            if (!closed) throw new Error("No current offscreen document.");
          }),
          callback,
        ),
      hasDocument: (callback) => withCallback(native("offscreen.has", {}).then(Boolean), callback),
    });
  }
  // Chrome lets extensions with clipboard permissions use the clipboard from
  // any of their pages, focused or not; WebKit requires a user gesture, which
  // offscreen and background-driven pages never have.
  if (isPage && typeof document !== "undefined") {
    const declared = (chromeRuntime && chromeRuntime.getManifest().permissions) || [];
    const canWrite = declared.includes("clipboardWrite");
    const canRead = declared.includes("clipboardRead");
    const clipboard = typeof navigator !== "undefined" ? navigator.clipboard : undefined;
    if (clipboard && canWrite) {
      clipboard.writeText = (text) => native("clipboard.write", { text: String(text) }).then(() => undefined);
    }
    if (clipboard && canRead) {
      clipboard.readText = () => native("clipboard.read", {}).then((text) => (typeof text === "string" ? text : ""));
    }
    if (canWrite && typeof Document === "function") {
      const execCommand = Document.prototype.execCommand;
      Document.prototype.execCommand = function (command, ...rest) {
        if (String(command).toLowerCase() !== "copy") return execCommand.call(this, command, ...rest);
        const field = this.activeElement;
        const text =
          field && typeof field.value === "string" && typeof field.selectionStart === "number"
            ? field.value.slice(field.selectionStart, field.selectionEnd)
            : String(this.getSelection ? this.getSelection() : "");
        native("clipboard.write", { text }).catch(() => {});
        return true;
      };
    }
  }

  // ---- Worker WebSockets ---------------------------------------------------
  // A WebSocket opened in an extension worker deadlocks it in WebKit; connect
  // through the browser instead.
  if (isWorker && typeof runtime.connectNative === "function" && !config.nativeWebSockets) {
    const encode = (buffer) => {
      const bytes = new Uint8Array(buffer);
      let text = "";
      for (let i = 0; i < bytes.length; i += 0x8000) {
        text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      }
      return btoa(text);
    };
    const decode = (text) => {
      const binary = atob(text);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      return bytes.buffer;
    };
    const states = { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 };
    class WebSocket extends EventTarget {
      #port;
      #ready = false;
      #queue = [];
      #hello;
      #sending = Promise.resolve();
      #binaryType = "blob";
      constructor(url, protocols) {
        super();
        let parsed;
        try {
          parsed = new URL(url, location.href);
        } catch {
          throw new DOMException(`Invalid URL '${url}'`, "SyntaxError");
        }
        if (parsed.protocol === "http:") parsed.protocol = "ws:";
        if (parsed.protocol === "https:") parsed.protocol = "wss:";
        if (parsed.protocol !== "ws:" && parsed.protocol !== "wss:") {
          throw new DOMException(`Invalid URL scheme '${parsed.protocol}'`, "SyntaxError");
        }
        this.url = parsed.href;
        this.readyState = states.CONNECTING;
        this.protocol = "";
        this.extensions = "";
        this.bufferedAmount = 0;
        this.onopen = this.onmessage = this.onerror = this.onclose = null;
        const port = runtime.connectNative("app.zephium.socket");
        this.#port = port;
        port.onMessage.addListener((message) => this.#receive(message));
        port.onDisconnect.addListener(() => this.#finish(1006, ""));
        const hello = () => {
          try {
            port.postMessage({ op: "hello" });
          } catch {}
        };
        hello();
        this.#hello = setInterval(hello, 50);
        const list = protocols === undefined ? [] : [].concat(protocols).map(String);
        this.#post({ op: "open", url: this.url, protocols: list, userAgent: navigator.userAgent });
      }
      get binaryType() {
        return this.#binaryType;
      }
      set binaryType(value) {
        if (value === "blob" || value === "arraybuffer") this.#binaryType = value;
      }
      send(data) {
        if (this.readyState === states.CONNECTING) {
          throw new DOMException("Still in CONNECTING state.", "InvalidStateError");
        }
        if (this.readyState !== states.OPEN) return;
        const post = (message) => this.#post(message);
        if (typeof data === "string") {
          this.#sending = this.#sending.then(() => post({ op: "send", text: data }));
        } else if (data instanceof Blob) {
          this.#sending = this.#sending.then(() =>
            data.arrayBuffer().then((buffer) => post({ op: "send", base64: encode(buffer) })),
          );
        } else if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) {
          const view = data instanceof ArrayBuffer ? data : data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
          this.#sending = this.#sending.then(() => post({ op: "send", base64: encode(view) }));
        } else {
          this.#sending = this.#sending.then(() => post({ op: "send", text: String(data) }));
        }
      }
      close(code, reason) {
        if (this.readyState >= states.CLOSING) return;
        this.readyState = states.CLOSING;
        this.#post({ op: "close", code: code === undefined ? 1000 : code, reason: reason || "" });
      }
      #post(message) {
        if (this.#ready) this.#port.postMessage(message);
        else this.#queue.push(message);
      }
      #receive(message) {
        switch (message && message.op) {
          case "hello":
            if (!this.#ready) {
              this.#ready = true;
              clearInterval(this.#hello);
              for (const queued of this.#queue.splice(0)) this.#port.postMessage(queued);
            }
            break;
          case "open":
            this.readyState = states.OPEN;
            this.protocol = message.protocol || "";
            this.#fire(new Event("open"));
            break;
          case "message": {
            let data = message.text;
            if (data === undefined) {
              const buffer = decode(message.base64 || "");
              data = this.#binaryType === "arraybuffer" ? buffer : new Blob([buffer]);
            }
            this.#fire(new MessageEvent("message", { data, origin: new URL(this.url).origin }));
            break;
          }
          case "error":
            this.#fire(new Event("error"));
            break;
          case "close":
            this.#finish(message.code, message.reason);
            break;
        }
      }
      #fire(event) {
        const handler = this["on" + event.type];
        if (typeof handler === "function") {
          try {
            handler.call(this, event);
          } catch (error) {
            setTimeout(() => {
              throw error;
            });
          }
        }
        this.dispatchEvent(event);
      }
      #finish(code, reason) {
        if (this.readyState === states.CLOSED) return;
        clearInterval(this.#hello);
        this.readyState = states.CLOSED;
        this.#fire(new CloseEvent("close", { code: code || 1006, reason: reason || "", wasClean: code === 1000 }));
        try {
          this.#port.disconnect();
        } catch {}
      }
    }
    Object.assign(WebSocket, states);
    Object.assign(WebSocket.prototype, states);
    g.WebSocket = WebSocket;
  }
})();
