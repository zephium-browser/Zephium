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

  // Release builds report only uncaught errors that point at a missing or
  // broken browser API, without stacks; the browser keeps their wording,
  // stripped of addresses and quoted text, for bug reports.
  if (!config.diagnostics && !isContent && typeof runtime.sendNativeMessage === "function") {
    const gap = /is not a function|is not an object|is not defined|Cannot read propert|Invalid call to|is not supported|not implemented/i;
    let left = 8;
    const where = isWorker ? "worker" : location.pathname;
    const report = (value) => {
      const text = String((value && value.message) || value || "").slice(0, 300);
      if (left <= 0 || !gap.test(text)) return;
      left--;
      try {
        native("log", { level: "error", text: `[gap] [${where}] ${text}` }).catch(() => {});
      } catch {}
    };
    g.addEventListener("error", (event) => report(event.error || event.message));
    g.addEventListener("unhandledrejection", (event) => report(event.reason));
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
  // WebKit finds the API through the global `browser` or `chrome` when it
  // delivers runtime messages, so a context that hides both (Proton Pass
  // swaps them for a locked Proxy) never hears another message. The globals
  // stay the browser's and assignments to them are ignored.
  // WebKit gives `browser` its own copy of the API; making it the same object
  // lets every fix below reach extensions written against `browser.*`. Both
  // accept callbacks and return promises.
  if (g.browser && g.browser !== chromeApi) kept.push(g.browser);
  for (const name of ["chrome", "browser"]) {
    try {
      Object.defineProperty(g, name, { configurable: true, enumerable: true, get: () => chromeApi, set() {} });
    } catch {
      pin(g, name, chromeApi);
    }
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

  // ---- Browser events ------------------------------------------------------
  // Events of the APIs the browser implements reach listening contexts over a
  // port; the browser starts the worker for events it listens to, and resends
  // whatever the worker didn't acknowledge.
  const hub =
    !isContent && typeof runtime.connectNative === "function"
      ? (() => {
          const listeners = new Map();
          let port = null;
          let ready = false;
          let hello = null;
          let pending = false;
          let retry = 0;
          const names = () => [...listeners].filter(([, set]) => set.size > 0).map(([name]) => name);
          const subscribe = () => {
            pending = false;
            if (!port) return connect();
            if (ready) port.postMessage({ op: "subscribe", worker: isWorker, events: names() });
          };
          const receive = (message) => {
            if (!message) return;
            if (message.op === "hello") {
              if (!ready) {
                ready = true;
                retry = 0;
                clearInterval(hello);
                subscribe();
              }
              return;
            }
            if (message.op !== "event") return;
            if (message.seq !== undefined) port.postMessage({ op: "ack", seq: message.seq });
            const set = listeners.get(message.name);
            const args = Array.isArray(message.args) ? message.args : [];
            let replied = !message.reply;
            const reply = (value) => {
              if (replied) return;
              replied = true;
              if (port) port.postMessage({ op: "reply", seq: message.seq, value: value === undefined ? null : value });
            };
            for (const listener of set ? [...set] : []) {
              try {
                const result = listener(...args, ...(message.reply ? [reply] : []));
                if (message.reply && result !== undefined) Promise.resolve(result).then(reply, () => reply(null));
              } catch (error) {
                setTimeout(() => {
                  throw error;
                });
              }
            }
            if (message.reply && !(set && set.size)) reply(null);
          };
          const connect = () => {
            if (port || names().length === 0) return;
            port = runtime.connectNative("app.zephium.events");
            port.onMessage.addListener(receive);
            port.onDisconnect.addListener(() => {
              clearInterval(hello);
              port = null;
              ready = false;
              if (names().length) setTimeout(connect, Math.min(30000, 250 * 2 ** retry++));
            });
            const greet = () => {
              try {
                if (port) port.postMessage({ op: "hello" });
              } catch {}
            };
            greet();
            hello = setInterval(greet, 50);
          };
          const changed = () => {
            if (pending) return;
            pending = true;
            setTimeout(subscribe, 0);
          };
          return {
            event(name, onListen) {
              const set = listeners.get(name) || new Set();
              listeners.set(name, set);
              return {
                addListener(listener) {
                  if (typeof listener !== "function" || set.has(listener)) return;
                  set.add(listener);
                  changed();
                  if (onListen) onListen();
                },
                removeListener(listener) {
                  if (set.delete(listener)) changed();
                },
                hasListener: (listener) => set.has(listener),
                hasListeners: () => set.size > 0,
              };
            },
          };
        })()
      : null;

  // WebKit's event objects can't be replaced on their namespaces; their
  // methods can. `alongside` keeps WebKit's own delivery as well.
  const adopt = (event, ours, alongside = false) => {
    kept.push(event);
    const add = event.addListener;
    const remove = event.removeListener;
    const has = event.hasListener;
    pin(event, "__zephiumAdd", (...args) => add.apply(event, args));
    pin(event, "__zephiumRemove", (listener) => remove.call(event, listener));
    pin(event, "__zephiumHas", (listener) => has.call(event, listener));
    pin(event, "addListener", function (listener, ...rest) {
      if (alongside) add.call(this, listener, ...rest);
      ours.addListener(listener, ...rest);
    });
    pin(event, "removeListener", function (listener) {
      if (alongside) remove.call(this, listener);
      ours.removeListener(listener);
    });
    pin(event, "hasListener", function (listener) {
      return ours.hasListener(listener) || (alongside && has.call(this, listener));
    });
    pin(event, "hasListeners", () => ours.hasListeners());
  };

  // Permissions of the APIs the browser implements; WebKit doesn't know them.
  const BROWSER_PERMISSIONS = new Set(["bookmarks", "downloads", "downloads.open", "history", "idle", "notifications", "privacy", "sessions", "topSites", "webRequestAuthProvider"]);
  const manifest = (() => {
    try {
      return runtime.getManifest() || {};
    } catch {
      return {};
    }
  })();
  const declares = (name) =>
    [].concat(manifest.permissions || [], manifest.optional_permissions || []).includes(name);
  // Optional native-app access is the browser's to grant: WebKit already holds
  // nativeMessaging for the browser's own bridges.
  const browserManaged = (name) =>
    BROWSER_PERMISSIONS.has(name) ||
    (name === "nativeMessaging" && (manifest.optional_permissions || []).includes(name));

  // Chrome answers "not granted" for permissions a browser doesn't know;
  // WebKit throws, which takes down callers such as Bitwarden's popup. The
  // retry stays synchronous so a request keeps the user's gesture.
  const permissions = namespace("permissions");
  if (permissions) {
    const invalid = /'([^']+)' is not a valid permission/;
    // WebKit rejects some unknown names synchronously and others only in the
    // returned promise; names learned either way are filtered up front.
    const unknownNames = new Set(["proxy", "debugger"]);
    const browserState = () => native("permissions.state", {}).catch(() => ({ granted: [] }));
    const browserPart = {
      contains: (names, ok) =>
        ok && names.length ? browserState().then((state) => names.every((name) => state.granted.includes(name))) : ok,
      // As in Chrome, a page asks during a click; a worker asks from the
      // events a click brings it.
      request: (names, ok) => {
        if (!ok || !names.length) return ok;
        const activation = !isWorker && g.navigator && g.navigator.userActivation;
        if (activation && !activation.isActive) {
          return Promise.reject(new Error("This function must be called during a user gesture"));
        }
        return native("permissions.request", { permissions: names });
      },
      remove: (names, ok) => (names.length ? native("permissions.remove", { permissions: names }).then((done) => ok && done) : ok),
    };
    const guard = (method, withUnknown) => {
      const original = permissions[method];
      if (typeof original !== "function") return;
      pin(permissions, method, function (request, callback) {
        const current = Object.assign({}, request);
        const requested = Array.isArray(current.permissions) ? current.permissions : [];
        const browserNames = requested.filter(browserManaged);
        // WebKit refuses match patterns with a port; Chrome matches every port
        // of a host either way.
        if (Array.isArray(current.origins)) {
          current.origins = current.origins.map((origin) => String(origin).replace(/^([^:]+:\/\/[^/:]+):\d+(\/|$)/, "$1$2"));
        }
        let unknown = requested.some((name) => unknownNames.has(name));
        current.permissions = requested.filter((name) => !unknownNames.has(name) && !browserManaged(name));
        let result;
        for (;;) {
          const listed = Array.isArray(current.permissions) ? current.permissions : [];
          const origins = Array.isArray(current.origins) ? current.origins : [];
          if ((unknown || browserNames.length) && listed.length === 0 && origins.length === 0) {
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
        if (unknown) result = result.then(withUnknown);
        return withCallback(result.then((ok) => browserPart[method](browserNames, ok)), callback);
      });
    };
    guard("contains", () => false);
    guard("request", () => false);
    guard("remove", (removed) => removed !== false);
    if (typeof permissions.getAll === "function") {
      const getAll = permissions.getAll;
      pin(permissions, "getAll", function (callback) {
        const all = Promise.all([Promise.resolve(getAll.call(permissions)), browserState()]).then(([found, state]) =>
          Object.assign({}, found, { permissions: [...new Set([...(found.permissions || []), ...state.granted])] }),
        );
        return withCallback(all, callback);
      });
    }
    // Only extensions that can hold the browser's own permissions hear about
    // them; a listening port keeps a worker loaded longer.
    if (hub && ([...BROWSER_PERMISSIONS].some(declares) || browserManaged("nativeMessaging"))) {
      for (const name of ["onAdded", "onRemoved"]) {
        const theirs = permissions[name];
        if (theirs && typeof theirs.addListener === "function") adopt(theirs, hub.event(`permissions.${name}`), true);
      }
    }
  }

  // WebKit raises cookies.onChanged without saying what changed; the
  // browser compares the cookie store and says it, as Chrome does.
  const cookies = hub && namespace("cookies");
  if (cookies && cookies.onChanged) adopt(cookies.onChanged, hub.event("cookies.onChanged"));
  if (cookies && !cookies.OnChangedCause) {
    pin(cookies, "OnChangedCause", Object.freeze({
      EVICTED: "evicted", EXPIRED: "expired", EXPLICIT: "explicit", EXPIRED_OVERWRITE: "expired_overwrite", OVERWRITE: "overwrite",
    }));
  }

  // Sign-in challenges reach listeners that may answer them (blocking or
  // asyncBlocking with webRequestAuthProvider) through the browser, which
  // asks the person only when none answered. Others keep WebKit's observing
  // event.
  const authEvent = hub && declares("webRequestAuthProvider") && chromeApi.webRequest && chromeApi.webRequest.onAuthRequired;
  if (authEvent && typeof authEvent.addListener === "function") {
    const answering = new Map();
    const covers = (pattern, url) => {
      if (pattern === "<all_urls>") return /^(https?|wss?|ftp):/.test(url);
      const match = /^(\*|https?|wss?|ftp):\/\/(\*|\*\.[^/*]+|[^/*]+)(\/.*)$/.exec(pattern);
      if (!match) return false;
      let parsed;
      try {
        parsed = new URL(url);
      } catch {
        return false;
      }
      const [, scheme, host, path] = match;
      const protocol = parsed.protocol.slice(0, -1);
      if (scheme === "*" ? !/^(https?|wss?)$/.test(protocol) : scheme !== protocol) return false;
      const name = host.includes(":") ? parsed.host : parsed.hostname;
      if (host !== "*" && !(host.startsWith("*.") ? name === host.slice(2) || name.endsWith(host.slice(1)) : name === host)) return false;
      const escaped = path.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
      return new RegExp(`^${escaped}$`).test(parsed.pathname + parsed.search);
    };
    const dispatch = hub.event("webRequest.onAuthRequired");
    dispatch.addListener((details, reply) => {
      const chosen = [...answering].filter(([, filter]) =>
        (filter.urls || []).some((pattern) => covers(pattern, details.url)) &&
        (!filter.tabId || filter.tabId === details.tabId),
      );
      if (chosen.length === 0) return reply(null);
      let waiting = chosen.length;
      let settled = false;
      const settle = (answer) => {
        if (settled) return;
        if (answer && (answer.cancel || answer.authCredentials)) {
          settled = true;
          return reply(answer);
        }
        if (--waiting === 0) {
          settled = true;
          reply(null);
        }
      };
      for (const [[listener, callsBack]] of chosen) {
        try {
          const result = callsBack ? listener(details, settle) : listener(details);
          if (!callsBack) settle(result);
        } catch (error) {
          settle(null);
          setTimeout(() => {
            throw error;
          });
        }
      }
    });
    adopt(authEvent, {
      addListener(listener, filter, spec) {
        const blocking = Array.isArray(spec) && (spec.includes("blocking") || spec.includes("asyncBlocking"));
        if (!blocking) return authEvent.__zephiumAdd(listener, filter, spec);
        answering.set([listener, spec.includes("asyncBlocking")], filter || {});
      },
      removeListener(listener) {
        for (const key of answering.keys()) if (key[0] === listener) answering.delete(key);
        authEvent.__zephiumRemove(listener);
      },
      hasListener: (listener) => [...answering.keys()].some((key) => key[0] === listener) || authEvent.__zephiumHas(listener),
      hasListeners: () => answering.size > 0,
    });
  }

  // Locks a vault or pauses work while the person is away from the Mac.
  if (hub && !chromeApi.idle && declares("idle")) {
    let interval = 60;
    const watch = () => native("idle.watch", { interval }).catch(() => {});
    const onStateChanged = hub.event("idle.onStateChanged", watch);
    pin(chromeApi, "idle", {
      IdleState: Object.freeze({ ACTIVE: "active", IDLE: "idle", LOCKED: "locked" }),
      queryState: (seconds, callback) =>
        withCallback(native("idle.query", { interval: Math.max(15, Math.floor(Number(seconds) || 0)) }), callback),
      setDetectionInterval: (seconds) => {
        interval = Math.max(15, Math.floor(Number(seconds) || 60));
        if (onStateChanged.hasListeners()) watch();
      },
      getAutoLockDelay: (callback) => withCallback(Promise.resolve(0), callback),
      onStateChanged,
    });
  }

  // OAuth sign-in: the browser opens the provider's page and returns the
  // https://<id>.chromiumapp.org/ redirect, which never actually loads.
  if (!isContent) {
    const identity = namespace("identity");
    const target = identity || {};
    if (!target.AccountStatus) pin(target, "AccountStatus", Object.freeze({ SYNC: "SYNC", ANY: "ANY" }));
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

  // Password managers turn the browser's own password filling off through
  // chrome.privacy; WebKit has no privacy API. The browser keeps the settings
  // it has, and answers the rest as not controllable.
  if (hub && !chromeApi.privacy && declares("privacy")) {
    const setting = (name) => {
      const onChange = hub.event(`privacy.${name}.onChange`);
      return {
        get: (_details, callback) => withCallback(native("privacy.get", { setting: name }), callback),
        set: (details, callback) =>
          withCallback(native("privacy.set", { setting: name, value: details && details.value }), callback),
        clear: (_details, callback) => withCallback(native("privacy.clear", { setting: name }), callback),
        onChange,
      };
    };
    const group = (prefix, names) => Object.fromEntries(names.map((name) => [name, setting(`${prefix}.${name}`)]));
    pin(chromeApi, "privacy", {
      services: group("services", [
        "passwordSavingEnabled", "autofillEnabled", "autofillAddressEnabled", "autofillCreditCardEnabled",
        "alternateErrorPagesEnabled", "safeBrowsingEnabled", "safeBrowsingExtendedReportingEnabled",
        "searchSuggestEnabled", "spellingServiceEnabled", "translationServiceEnabled",
      ]),
      network: group("network", ["networkPredictionEnabled", "webRTCIPHandlingPolicy"]),
      websites: group("websites", [
        "thirdPartyCookiesAllowed", "hyperlinkAuditingEnabled", "referrersEnabled", "doNotTrackEnabled",
        "protectedContentEnabled", "topicsEnabled", "fledgeEnabled", "adMeasurementEnabled",
      ]),
      IPHandlingPolicy: Object.freeze({
        DEFAULT: "default", DEFAULT_PUBLIC_AND_PRIVATE_INTERFACES: "default_public_and_private_interfaces",
        DEFAULT_PUBLIC_INTERFACE_ONLY: "default_public_interface_only", DISABLE_NON_PROXIED_UDP: "disable_non_proxied_udp",
      }),
    });
  }

  // Browser data lives in the browser's own stores; WebKit has no API for it.
  const dataEvent = (name) => (hub ? hub.event(name) : makeEvent());
  const dataCall = (api) => (details, callback) => withCallback(native(api, { details }), callback);

  if (!isContent && !chromeApi.history && declares("history")) {
    pin(chromeApi, "history", {
      TransitionType: Object.freeze({
        LINK: "link", TYPED: "typed", AUTO_BOOKMARK: "auto_bookmark", AUTO_SUBFRAME: "auto_subframe",
        MANUAL_SUBFRAME: "manual_subframe", GENERATED: "generated", AUTO_TOPLEVEL: "auto_toplevel",
        FORM_SUBMIT: "form_submit", RELOAD: "reload", KEYWORD: "keyword", KEYWORD_GENERATED: "keyword_generated",
      }),
      search: dataCall("history.search"),
      getVisits: dataCall("history.getVisits"),
      addUrl: dataCall("history.addUrl"),
      deleteUrl: dataCall("history.deleteUrl"),
      deleteRange: dataCall("history.deleteRange"),
      deleteAll: (callback) => dataCall("history.deleteAll")({}, callback),
      onVisited: dataEvent("history.onVisited"),
      onVisitRemoved: dataEvent("history.onVisitRemoved"),
    });
  }

  // Recently closed tabs; the browser reopens tabs, not whole windows, and
  // syncs no devices. A reopened tab is the active one, which tabs.query
  // describes in full.
  if (!isContent && !chromeApi.sessions && declares("sessions")) {
    const restore = (sessionId) =>
      native("sessions.restore", { sessionId }).then(async (restored) => {
        try {
          const [tab] = await chromeApi.tabs.query({ active: true, lastFocusedWindow: true });
          if (tab && tab.url === restored.tab.url) restored.tab = tab;
        } catch {}
        return restored;
      });
    pin(chromeApi, "sessions", {
      MAX_SESSION_RESULTS: 25,
      getRecentlyClosed: (filter, callback) =>
        typeof filter === "function"
          ? withCallback(native("sessions.getRecentlyClosed", {}), filter)
          : withCallback(native("sessions.getRecentlyClosed", { filter }), callback),
      getDevices: (filter, callback) =>
        withCallback(Promise.resolve([]), typeof filter === "function" ? filter : callback),
      restore: (sessionId, callback) =>
        typeof sessionId === "function" ? withCallback(restore(), sessionId) : withCallback(restore(sessionId), callback),
      onChanged: dataEvent("sessions.onChanged"),
    });
  }

  // Downloads go through the browser's download manager with the same
  // admission and prompts as the page in front. It keeps a download's origin
  // and file name, never its full address or path.
  if (!isContent && !chromeApi.downloads && declares("downloads")) {
    const MAX_DATA = 32 * 1024 * 1024;
    const ask = (op, fields, callback) => withCallback(native(`downloads.${op}`, fields), callback);
    const byId = (op) => (id, callback) => ask(op, { id }, callback);
    const refuse = (message) => (_id, callback) => withCallback(Promise.reject(new Error(message)), callback);
    // An extension's own blob: addresses can't be read by the page that
    // downloads; they go over as data: addresses.
    const readable = async (url) => {
      if (!/^blob:/i.test(url)) return url;
      const blob = await (await fetch(url)).blob();
      if (blob.size > MAX_DATA) throw new Error("The data is too large to download.");
      return new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result);
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(blob);
      });
    };
    const gesture = () => typeof navigator !== "undefined" && !!(navigator.userActivation && navigator.userActivation.isActive);
    pin(chromeApi, "downloads", {
      State: Object.freeze({ IN_PROGRESS: "in_progress", INTERRUPTED: "interrupted", COMPLETE: "complete" }),
      DangerType: Object.freeze({
        FILE: "file", URL: "url", CONTENT: "content", UNCOMMON: "uncommon", HOST: "host", UNWANTED: "unwanted",
        SAFE: "safe", ACCEPTED: "accepted",
      }),
      FilenameConflictAction: Object.freeze({ UNIQUIFY: "uniquify", OVERWRITE: "overwrite", PROMPT: "prompt" }),
      download: (options, callback) =>
        withCallback(
          readable(String((options && options.url) || "")).then((url) =>
            native("downloads.download", { details: Object.assign({}, options, { url }) }),
          ),
          callback,
        ),
      search: (query, callback) => ask("search", { details: query || {} }, callback),
      pause: byId("pause"),
      resume: byId("resume"),
      cancel: byId("cancel"),
      erase: (query, callback) =>
        withCallback(
          native("downloads.search", { details: query || {} }).then(async (items) => {
            const erased = [];
            for (const item of items) {
              if (item.state === "in_progress") continue;
              try {
                await native("downloads.erase", { id: item.id });
                erased.push(item.id);
              } catch {}
            }
            return erased;
          }),
          callback,
        ),
      // Chrome opens a download only on a user gesture.
      open: (id, callback) =>
        gesture() ? ask("open", { id }, callback) : refuse("User gesture required")(id, callback),
      show: (id) => void native("downloads.show", { id }).catch(() => {}),
      removeFile: refuse("Downloaded files are deleted only by the person."),
      acceptDanger: refuse("Only the person can keep a file held for its type, from the downloads panel."),
      onCreated: dataEvent("downloads.onCreated"),
      onChanged: dataEvent("downloads.onChanged"),
      onErased: dataEvent("downloads.onErased"),
    });
  }

  if (!isContent && !chromeApi.topSites && declares("topSites")) {
    pin(chromeApi, "topSites", { get: (callback) => withCallback(native("topSites.get", {}), callback) });
  }

  // Chrome's root "0" stands above the browser's top level; there is no
  // bookmarks bar or other permanent folder.
  if (!isContent && !chromeApi.bookmarks && declares("bookmarks")) {
    const ask = (op, fields, callback) => withCallback(native(`bookmarks.${op}`, fields), callback);
    const event = (name) => dataEvent(`bookmarks.${name}`);
    pin(chromeApi, "bookmarks", {
      getTree: (callback) => ask("getTree", {}, callback),
      getSubTree: (id, callback) => ask("getSubTree", { id }, callback),
      getChildren: (id, callback) => ask("getChildren", { id }, callback),
      get: (ids, callback) => ask("get", { ids: [].concat(ids) }, callback),
      getRecent: (count, callback) => ask("getRecent", { count }, callback),
      search: (query, callback) => ask("search", { details: query }, callback),
      create: (bookmark, callback) => ask("create", { details: bookmark || {} }, callback),
      update: (id, changes, callback) => ask("update", { id, details: changes || {} }, callback),
      move: (id, destination, callback) => ask("move", { id, details: destination || {} }, callback),
      remove: (id, callback) => ask("remove", { id }, callback),
      removeTree: (id, callback) => ask("removeTree", { id }, callback),
      onCreated: event("onCreated"),
      onChanged: event("onChanged"),
      onMoved: event("onMoved"),
      onRemoved: event("onRemoved"),
      // The browser keeps no custom order apart from moves, which onMoved reports.
      onChildrenReordered: makeEvent(),
      onImportBegan: event("onImportBegan"),
      onImportEnded: event("onImportEnded"),
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

  // Notifications go to the Mac's notification center; clicks, buttons and
  // dismissals come back as events.
  if (hub && declares("notifications")) {
    const target = chromeApi.notifications || {};
    let created = 0;
    const listen = (name) => {
      const ours = hub.event(`notifications.${name}`);
      if (target[name] && typeof target[name].addListener === "function") adopt(target[name], ours);
      else pin(target, name, ours);
    };
    const show = (id, options, callback) => {
      if (typeof id === "object" && id !== null) [id, options, callback] = [undefined, id, options];
      const name = typeof id === "string" && id ? id : `zephium-${Date.now()}-${++created}`;
      return withCallback(native("notifications.create", { id: name, options: options || {} }), callback);
    };
    pin(target, "TemplateType", Object.freeze({ BASIC: "basic", IMAGE: "image", LIST: "list", PROGRESS: "progress" }));
    pin(target, "PermissionLevel", Object.freeze({ GRANTED: "granted", DENIED: "denied" }));
    pin(target, "create", show);
    pin(target, "update", (id, options, callback) =>
      withCallback(native("notifications.update", { id: String(id), options: options || {} }), callback),
    );
    pin(target, "clear", (id, callback) => withCallback(native("notifications.clear", { id: String(id) }), callback));
    pin(target, "getAll", (callback) => withCallback(native("notifications.getAll", {}), callback));
    pin(target, "getPermissionLevel", (callback) => withCallback(native("notifications.permission", {}), callback));
    for (const name of ["onClicked", "onClosed", "onButtonClicked"]) listen(name);
    for (const name of ["onPermissionLevelChanged", "onShowSettings"]) if (!target[name]) pin(target, name, makeEvent());
    if (!chromeApi.notifications) pin(chromeApi, "notifications", target);
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
    if (typeof chromeRuntime.getContexts !== "function") {
      pin(chromeRuntime, "getContexts", (filter, callback) => {
        const wanted = filter || {};
        const keep = (list, value) => !Array.isArray(list) || list.includes(value);
        const found = native("runtime.contexts", {}).then((contexts) =>
          (contexts || []).filter(
            (context) =>
              keep(wanted.contextTypes, context.contextType) &&
              keep(wanted.contextIds, context.contextId) &&
              keep(wanted.tabIds, context.tabId) &&
              keep(wanted.windowIds, context.windowId) &&
              keep(wanted.frameIds, context.frameId) &&
              keep(wanted.documentUrls, context.documentUrl) &&
              keep(wanted.documentOrigins, context.documentOrigin) &&
              (wanted.incognito === undefined || wanted.incognito === context.incognito),
          ),
        );
        return withCallback(found, callback);
      });
    }
    if (typeof chromeRuntime.requestUpdateCheck !== "function") {
      pin(chromeRuntime, "requestUpdateCheck", (callback) => withCallback(Promise.resolve({ status: "no_update" }), callback));
    }
  }

  const webRequest = !isContent && namespace("webRequest");
  if (webRequest && typeof webRequest.handlerBehaviorChanged !== "function") {
    // WebKit keeps no in-memory cache of listener decisions to flush.
    pin(webRequest, "handlerBehaviorChanged", (callback) => withCallback(Promise.resolve(), callback));
    if (webRequest.MAX_HANDLER_BEHAVIOR_CHANGED_CALLS_PER_10_MINUTES === undefined) {
      pin(webRequest, "MAX_HANDLER_BEHAVIOR_CHANGED_CALLS_PER_10_MINUTES", 20);
    }
  }
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
        const types = filter && Array.isArray(filter.types) ? filter.types : null;
        const unknownType = (type) => type === "webbundle" || type === "webtransport";
        if ((!urls || !urls.some((url) => socket.test(url))) && (!types || !types.some(unknownType))) {
          return add.call(this, listener, filter, ...rest);
        }
        const usable = urls ? urls.filter((url) => !socket.test(url)) : urls;
        const known = types ? types.filter((type) => !unknownType(type)) : types;
        if ((usable && usable.length === 0) || (known && known.length === 0)) return undefined;
        const narrowed = Object.assign({}, filter, usable ? { urls: usable } : {}, known ? { types: known } : {});
        return add.call(this, listener, narrowed, ...rest);
      });
    }
  }

  // WebKit rejects a whole rule update over one rule it can't apply, such as
  // one setting a header it doesn't know. Chrome would apply the rest.
  const netRequest = !isContent && namespace("declarativeNetRequest");
  constants(netRequest, {
    RuleActionType: { BLOCK: "block", REDIRECT: "redirect", ALLOW: "allow", UPGRADE_SCHEME: "upgradeScheme", MODIFY_HEADERS: "modifyHeaders", ALLOW_ALL_REQUESTS: "allowAllRequests" },
    ResourceType: {
      MAIN_FRAME: "main_frame", SUB_FRAME: "sub_frame", STYLESHEET: "stylesheet", SCRIPT: "script", IMAGE: "image", FONT: "font",
      OBJECT: "object", XMLHTTPREQUEST: "xmlhttprequest", PING: "ping", CSP_REPORT: "csp_report", MEDIA: "media",
      WEBSOCKET: "websocket", WEBTRANSPORT: "webtransport", WEBBUNDLE: "webbundle", OTHER: "other",
    },
    RequestMethod: { CONNECT: "connect", DELETE: "delete", GET: "get", HEAD: "head", OPTIONS: "options", PATCH: "patch", POST: "post", PUT: "put", OTHER: "other" },
    DomainType: { FIRST_PARTY: "firstParty", THIRD_PARTY: "thirdParty" },
    HeaderOperation: { APPEND: "append", SET: "set", REMOVE: "remove" },
    UnsupportedRegexReason: { SYNTAX_ERROR: "syntaxError", MEMORY_LIMIT_EXCEEDED: "memoryLimitExceeded" },
  });
  if (netRequest) {
    for (const [name, value] of Object.entries({
      GUARANTEED_MINIMUM_STATIC_RULES: 30000, MAX_NUMBER_OF_STATIC_RULESETS: 100, MAX_NUMBER_OF_ENABLED_STATIC_RULESETS: 50,
      MAX_NUMBER_OF_DYNAMIC_RULES: 30000, MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES: 5000, MAX_NUMBER_OF_SESSION_RULES: 5000,
      MAX_NUMBER_OF_UNSAFE_SESSION_RULES: 5000, MAX_NUMBER_OF_REGEX_RULES: 1000, SESSION_RULESET_ID: "_session", DYNAMIC_RULESET_ID: "_dynamic",
    })) {
      if (netRequest[name] === undefined) pin(netRequest, name, value);
    }
    // Chrome counts what is left of its guaranteed minimum; WebKit keeps no
    // such pool, so the minimum is what is offered.
    if (typeof netRequest.getAvailableStaticRuleCount !== "function") {
      pin(netRequest, "getAvailableStaticRuleCount", (callback) =>
        withCallback(Promise.resolve(netRequest.GUARANTEED_MINIMUM_STATIC_RULES), callback),
      );
    }
    // WebKit can't switch off single static rules, so none ever are.
    if (typeof netRequest.getDisabledRuleIds !== "function") {
      pin(netRequest, "getDisabledRuleIds", (_options, callback) => withCallback(Promise.resolve([]), callback));
    }
    if (typeof netRequest.updateStaticRules !== "function") {
      pin(netRequest, "updateStaticRules", (options, callback) => {
        const disabling = options && Array.isArray(options.disableRuleIds) && options.disableRuleIds.length;
        const result = disabling ? Promise.reject(new Error("Disabling single static rules is not supported.")) : Promise.resolve();
        return withCallback(result, callback);
      });
    }
    // WebKit's rule compiler takes JavaScript-like expressions without
    // lookaround or backreferences.
    const regexSupport = (options) => {
      const source = String((options && options.regex) || "");
      try {
        new RegExp(source, options && options.isCaseSensitive === false ? "i" : "");
      } catch {
        return { isSupported: false, reason: "syntaxError" };
      }
      return /\(\?<?[=!]|\\[1-9]/.test(source) ? { isSupported: false, reason: "syntaxError" } : { isSupported: true };
    };
    const isRegexSupported = netRequest.isRegexSupported;
    pin(netRequest, "isRegexSupported", (options, callback) => {
      let native;
      try {
        native = typeof isRegexSupported === "function" ? isRegexSupported.call(netRequest, options) : undefined;
      } catch {}
      const answer = Promise.resolve(native)
        .catch(() => undefined)
        .then((result) => (result && typeof result.isSupported === "boolean" ? result : regexSupport(options)));
      return withCallback(answer, callback);
    });
  }
  if (netRequest) {
    // Rules are checked one rejection at a time, so expressions WebKit's
    // compiler can't take are dropped first, in one pass.
    const rejected = /rule at index (\d+)/;
    const supported = async (rules) => {
      const answers = new Map();
      for (const rule of rules) {
        const regex = rule && rule.condition && rule.condition.regexFilter;
        if (typeof regex !== "string" || answers.has(regex)) continue;
        answers.set(regex, netRequest.isRegexSupported({ regex }).then((answer) => answer.isSupported, () => true));
      }
      const unsupported = new Set();
      for (const [regex, answer] of answers) if (!(await answer)) unsupported.add(regex);
      return unsupported.size ? rules.filter((rule) => !unsupported.has(rule && rule.condition && rule.condition.regexFilter)) : rules;
    };
    for (const method of ["updateSessionRules", "updateDynamicRules"]) {
      const original = netRequest[method];
      if (typeof original !== "function") continue;
      pin(netRequest, method, function (options, callback) {
        const rules = options && Array.isArray(options.addRules) ? options.addRules : [];
        const skipped = [];
        let dropped = 0;
        const run = (current) => {
          let result;
          try {
            result = Promise.resolve(original.call(netRequest, current));
          } catch (error) {
            result = Promise.reject(error);
          }
          return result.catch((error) => {
            const index = Number((rejected.exec(String(error && error.message)) || [])[1]);
            const kept = Array.isArray(current.addRules) ? current.addRules : [];
            if (!(index < kept.length)) throw error;
            dropped++;
            skipped.push(error.message);
            return run(Object.assign({}, current, { addRules: kept.filter((_, i) => i !== index) }));
          });
        };
        const done = (rules.length ? supported(rules) : Promise.resolve(rules))
          .then((usable) => {
            dropped = rules.length - usable.length;
            if (dropped) skipped.push(`${dropped} with a regexFilter WebKit can't compile`);
            return run(rules.length ? Object.assign({}, options, { addRules: usable }) : options);
          })
          .finally(() => {
            if (dropped && Z.report) Z.report("warning", `skipped ${dropped} network rule(s) WebKit can't apply: ${skipped.slice(0, 3).join("; ")}`);
          });
        return withCallback(done, callback);
      });
    }
    // WebKit fails a change that leaves no rules at all while its first
    // compile of the static rulesets is still running; Chrome applies it.
    const updateEnabledRulesets = netRequest.updateEnabledRulesets;
    if (typeof updateEnabledRulesets === "function") {
      const leavesNoRules = async (options) => {
        const enabled = new Set(await netRequest.getEnabledRulesets());
        for (const id of options.disableRulesetIds || []) enabled.delete(id);
        for (const id of options.enableRulesetIds || []) enabled.add(id);
        if (enabled.size) return false;
        const [dynamic, session] = await Promise.all([netRequest.getDynamicRules(), netRequest.getSessionRules()]);
        return !dynamic.length && !session.length;
      };
      pin(netRequest, "updateEnabledRulesets", function (options, callback) {
        const run = (delay) =>
          Promise.resolve()
            .then(() => updateEnabledRulesets.call(netRequest, options))
            .catch(async (error) => {
              if (delay > 16000 || !/Failed to apply rules\.$/.test(String(error && error.message)) || !(await leavesNoRules(options || {}))) throw error;
              await new Promise((resolve) => setTimeout(resolve, delay));
              return run(delay * 2);
            });
        return withCallback(run(500), callback);
      });
    }
  }

  // Every enabled extension's action is shown in Zephium's toolbar.
  const action = !isContent && namespace("action");
  // Zephium draws badges in its own toolbar colors; the text color an
  // extension picks is kept and answered back.
  if (action && typeof action.setBadgeTextColor !== "function") {
    const colors = new Map();
    pin(action, "setBadgeTextColor", (details, callback) => {
      colors.set((details && details.tabId) ?? null, details && details.color);
      return withCallback(Promise.resolve(), callback);
    });
    pin(action, "getBadgeTextColor", (details, callback) => {
      const color = colors.get((details && details.tabId) ?? null) ?? colors.get(null) ?? [255, 255, 255, 255];
      return withCallback(Promise.resolve(Array.isArray(color) ? color : [255, 255, 255, 255]), callback);
    });
  }
  if (action && typeof action.getUserSettings !== "function") {
    pin(action, "getUserSettings", (callback) => withCallback(Promise.resolve({ isOnToolbar: true }), callback));
    if (!action.onUserSettingsChanged) pin(action, "onUserSettingsChanged", makeEvent());
  }

  // WebKit rejects a menu item whose URL patterns name ftp://; such pages
  // never load here, so those patterns are dropped.
  for (const name of ["contextMenus", "menus"]) {
    const menus = !isContent && namespace(name);
    if (!menus) continue;
    const clean = (properties) => {
      if (!properties || typeof properties !== "object") return properties;
      const next = Object.assign({}, properties);
      for (const key of ["documentUrlPatterns", "targetUrlPatterns"]) {
        if (!Array.isArray(next[key]) || !next[key].some((pattern) => /^ftp:/i.test(pattern))) continue;
        const kept = next[key].filter((pattern) => !/^ftp:/i.test(pattern));
        next[key] = kept.length ? kept : ["https://ftp.invalid/*"];
      }
      return next;
    };
    for (const method of ["create", "update"]) {
      const original = menus[method];
      if (typeof original !== "function") continue;
      pin(menus, method, function (...args) {
        const at = method === "create" ? 0 : 1;
        args[at] = clean(args[at]);
        return original.apply(menus, args);
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

  // Chrome's popup is in no tab; WebKit answers with the tab under it, and
  // popups that size themselves (Proton Pass) then fill a tab's width.
  if (tabsApi && typeof tabsApi.getCurrent === "function") {
    const getCurrent = tabsApi.getCurrent;
    const extension = namespace("extension");
    pin(tabsApi, "getCurrent", function (callback) {
      const views = extension && typeof extension.getViews === "function" ? extension.getViews({ type: "popup" }) : [];
      if (views.includes(globalThis)) return withCallback(Promise.resolve(undefined), callback);
      return getCurrent.apply(this, arguments);
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
  if (!isContent && !chromeApi.offscreen && declares("offscreen")) {
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

  // ---- Detached calls ------------------------------------------------------
  // Chrome's API functions work apart from their namespace (`const { query }
  // = chrome.tabs`); WebKit's answer nothing without it. Every method of the
  // API, nested areas and events included, is bound to its owner.
  const bound = new WeakSet();
  const bindAll = (target, depth) => {
    if (!target || typeof target !== "object" || bound.has(target) || depth > 3) return;
    bound.add(target);
    const names = new Set();
    for (let level = target; level && level !== Object.prototype; level = Object.getPrototypeOf(level)) {
      for (const name of Object.getOwnPropertyNames(level)) names.add(name);
    }
    for (const name of names) {
      if (name === "constructor" || name === "lastError" || name.startsWith("__zephium")) continue;
      let value;
      try {
        value = target[name];
      } catch {
        continue;
      }
      if (typeof value === "function") {
        if (!bound.has(value)) {
          const method = value.bind(target);
          bound.add(method);
          if (pin(target, name, method)) kept.push(target);
        }
      } else if (value && typeof value === "object" && !Object.isFrozen(value)) {
        bindAll(value, depth + 1);
      }
    }
  };
  bindAll(chromeApi, 0);
})();
