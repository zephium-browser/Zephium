// The background: a dedicated Worker started by a hidden owner page, made to
// look like an extension service worker. `chrome.*` in the worker talks to the
// owner over a MessagePort; the owner relays frames to the native bridge and
// reports worker errors to the host.
//
// `registration` is left undefined: Bitwarden only builds its Web Push
// service when `self.registration` is non-null and otherwise takes its
// "unsupported" path, which is the only one that can work without a real
// push subscription; no other target reads it.
(function workerScope(Z) {
  'use strict';
  const I = Z.I;
  const T = Z.transport;

  const PORT_MARKER = 'zephium.worker.port.v1';
  const WORKER_ID_LIMIT = 0x7fffffff;
  const LIFECYCLE_TIMEOUT_MS = 5 * 60 * 1000;
  const HANDLER_EVENTS = [
    'install',
    'activate',
    'fetch',
    'push',
    'pushsubscriptionchange',
    'sync',
    'periodicsync',
    'notificationclick',
    'notificationclose',
    'backgroundfetchsuccess',
    'backgroundfetchfail',
    'backgroundfetchabort',
    'backgroundfetchclick',
    'cookiechange',
  ];
  const DEDICATED_ONLY = ['postMessage', 'close', 'name', 'requestAnimationFrame', 'cancelAnimationFrame'];

  const own = (object, key) =>
    object !== null && typeof object === 'object' && I.hasOwn(object, key) ? object[key] : undefined;
  const hidden = (target, key, value) =>
    I.defineProperty(target, key, { value, writable: true, enumerable: false, configurable: true });
  const illegal = () => {
    throw new I.TypeError('Illegal constructor');
  };

  const diagnosticText = (value, limit) => {
    const string = typeof value === 'string' ? value : value === undefined || value === null ? '' : I.String(value);
    const bytes = I.encodeUtf8(I.StringSlice(string, 0, limit));
    let end = I.TypedArrayLength(bytes);
    if (end > limit) end = limit;
    while (end > 0) {
      try {
        return I.decodeUtf8(I.TypedArraySubarray(bytes, 0, end));
      } catch {
        end--;
      }
    }
    return '';
  };

  function defineEventClasses(global) {
    const Base = I.Event || global.Event;
    const states = new I.WeakMap();
    const DOMException = I.DOMException;
    class ExtendableEvent extends Base {
      constructor(type, init) {
        super(type, init);
        I.WeakMapSet(states, this, { __proto__: null, dispatching: false, pending: 0, promises: [] });
      }
      waitUntil(promise) {
        const state = I.WeakMapGet(states, this);
        if (state === undefined) throw new I.TypeError('Illegal invocation');
        if (!state.dispatching && state.pending === 0) {
          const message =
            "Failed to execute 'waitUntil' on 'ExtendableEvent': The event handler is already finished and no extend lifetime promises are outstanding.";
          throw DOMException ? new DOMException(message, 'InvalidStateError') : new I.Error(message);
        }
        state.pending++;
        const settled = new I.Promise((resolve) => {
          I.PromiseThen(
            I.PromiseResolve(promise),
            () => {
              state.pending--;
              resolve(true);
            },
            () => {
              state.pending--;
              resolve(false);
            },
          );
        });
        I.ArrayPush(state.promises, settled);
      }
    }
    class InstallEvent extends ExtendableEvent {}
    // Dispatches and resolves once every waitUntil promise settled: true when
    // none rejected.
    const run = (target, event) => {
      const state = I.WeakMapGet(states, event);
      state.dispatching = true;
      try {
        I.dispatchEvent(target, event);
      } finally {
        state.dispatching = false;
      }
      return new I.Promise((resolve) => {
        let ok = true;
        const next = (index) => {
          if (index >= state.promises.length) return resolve(ok);
          I.PromiseThen(state.promises[index], (fine) => {
            ok = ok && fine;
            next(index + 1);
          });
          return undefined;
        };
        next(0);
      });
    };
    return { __proto__: null, ExtendableEvent, InstallEvent, run };
  }

  function defineClients(chrome) {
    const Clients = function Clients() {
      illegal();
    };
    const proto = Clients.prototype;
    hidden(proto, 'claim', function claim() {
      return I.PromiseResolve(undefined);
    });
    hidden(proto, 'matchAll', function matchAll() {
      return I.PromiseResolve([]);
    });
    hidden(proto, 'get', function get() {
      return I.PromiseResolve(undefined);
    });
    hidden(proto, 'openWindow', function openWindow(url) {
      const tabs = chrome && chrome.tabs;
      if (!tabs || typeof tabs.create !== 'function') {
        return I.PromiseReject(
          I.DOMException
            ? new I.DOMException('Not allowed to open a window.', 'InvalidAccessError')
            : new I.Error('Not allowed to open a window.'),
        );
      }
      return I.PromiseThen(tabs.create({ url: I.String(url) }), () => null);
    });
    I.defineProperty(proto, I.toStringTag, { value: 'Clients', configurable: true });
    return { __proto__: null, Clients, clients: I.create(proto) };
  }

  function workerLocation(global, scriptURL) {
    let url;
    try {
      url = new I.URL(scriptURL);
    } catch {
      return;
    }
    const Location = global.WorkerLocation;
    const location = I.create(Location ? Location.prototype : I.ObjectPrototype);
    const fields = {
      href: I.URLHref(url),
      origin: I.URLOrigin(url),
      protocol: I.URLProtocol(url),
      host: I.URLHost(url),
      hostname: I.URLHostname(url),
      port: I.URLPort(url),
      pathname: I.URLPathname(url),
      search: I.URLSearch(url),
      hash: I.URLHash(url),
    };
    const names = I.keys(fields);
    for (let i = 0; i < names.length; i++) {
      I.defineProperty(location, names[i], { value: fields[names[i]], enumerable: true, configurable: true });
    }
    const href = fields.href;
    hidden(location, 'toString', function toString() {
      return href;
    });
    I.defineProperty(global, 'location', { value: location, enumerable: true, configurable: true, writable: false });
  }

  function storageManager(global, details) {
    const navigator = global.navigator;
    if (!navigator || navigator.storage) return;
    const quota = typeof own(details, 'quota') === 'number' ? details.quota : 0;
    const usage = typeof own(details, 'usage') === 'number' ? details.usage : 0;
    const persisted = own(details, 'persisted') === true;
    const StorageManager = function StorageManager() {
      illegal();
    };
    const proto = StorageManager.prototype;
    hidden(proto, 'estimate', function estimate() {
      return I.PromiseResolve({ quota, usage, usageDetails: {} });
    });
    hidden(proto, 'persisted', function persistedState() {
      return I.PromiseResolve(persisted);
    });
    hidden(proto, 'persist', function persist() {
      return I.PromiseResolve(persisted);
    });
    I.defineProperty(proto, I.toStringTag, { value: 'StorageManager', configurable: true });
    if (!global.StorageManager) hidden(global, 'StorageManager', StorageManager);
    I.defineProperty(navigator, 'storage', { value: I.create(proto), enumerable: true, configurable: true });
  }

  function workerImports(global, config) {
    const nativeImport = global.importScripts;
    const module = own(config, 'type') === 'module';
    let installing = own(config, 'install') === true;
    const loaded = new I.Set();
    const resolve = (value) => {
      const url = I.URLHref(new I.URL(I.String(value), own(config, 'scriptURL')));
      const hash = I.StringIndexOf(url, '#');
      return hash === -1 ? url : I.StringSlice(url, 0, hash);
    };
    if (typeof own(config, 'scriptURL') === 'string') I.SetAdd(loaded, resolve(config.scriptURL));
    const installed = own(config, 'installedScripts');
    if (I.isArray(installed)) {
      for (let i = 0; i < installed.length; i++) {
        if (typeof installed[i] === 'string') I.SetAdd(loaded, resolve(installed[i]));
      }
    }
    const importScripts = function importScripts(...urls) {
      if (module) {
        throw new I.TypeError(
          "Failed to execute 'importScripts' on 'WorkerGlobalScope': Module scripts don't support importScripts().",
        );
      }
      const resolved = [];
      for (let i = 0; i < urls.length; i++) I.ArrayPush(resolved, resolve(urls[i]));
      for (let i = 0; i < resolved.length; i++) {
        const url = resolved[i];
        if (!installing && !I.SetHas(loaded, url)) {
          const message = `Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '${url}' failed to load.`;
          throw I.DOMException ? new I.DOMException(message, 'NetworkError') : new I.Error(message);
        }
        I.apply(nativeImport, global, [url]);
        I.SetAdd(loaded, url);
      }
    };
    hidden(global, 'importScripts', importScripts);
    const scope = global.WorkerGlobalScope;
    if (scope && scope.prototype) hidden(scope.prototype, 'importScripts', importScripts);
    return () => {
      installing = false;
    };
  }

  // Runs in the worker before the extension's script. `options.global` and
  // `options.link` are for tests.
  function worker(surface, bootstrap, options) {
    options = options || {};
    const global = options.global || I.global;
    const config = own(bootstrap, 'worker') || {};
    const link = options.link || T.portLink(T.messageLimit('background'));
    let attached = false;
    const onInit = (event) => {
      if (attached || I.MessageEventData(event) !== PORT_MARKER) return;
      const ports = I.MessageEventPorts(event);
      if (!ports || ports.length !== 1) return;
      I.stopImmediatePropagation(event);
      attached = link.attach(ports[0]);
    };
    if (!options.link) I.addEventListener(global, 'message', onInit, true);

    const context = Z.api.create(surface, bootstrap, link, { global, idLimit: WORKER_ID_LIMIT });
    const chrome = context.chrome;
    context.hub.hold();

    const WorkerGlobalScope = global.WorkerGlobalScope;
    const Dedicated = global.DedicatedWorkerGlobalScope;
    const ServiceWorkerGlobalScope = function ServiceWorkerGlobalScope() {
      illegal();
    };
    const scopeProto = I.create(
      WorkerGlobalScope ? WorkerGlobalScope.prototype : I.getPrototypeOf(global) || I.ObjectPrototype,
    );
    hidden(scopeProto, 'constructor', ServiceWorkerGlobalScope);
    I.defineProperty(scopeProto, I.toStringTag, { value: 'ServiceWorkerGlobalScope', configurable: true });
    I.defineProperty(ServiceWorkerGlobalScope, 'prototype', { value: scopeProto });
    I.defineProperty(ServiceWorkerGlobalScope, I.hasInstance, {
      value: (value) => {
        if (value === global) return true;
        for (
          let proto = value !== null && typeof value === 'object' ? I.getPrototypeOf(value) : null;
          proto !== null;
          proto = I.getPrototypeOf(proto)
        ) {
          if (proto === scopeProto) return true;
        }
        return false;
      },
    });
    let spliced = false;
    if (Dedicated && Dedicated.prototype) {
      try {
        spliced = I.setPrototypeOf(Dedicated.prototype, scopeProto);
      } catch {
        spliced = false;
      }
    }
    const members = spliced ? scopeProto : global;
    hidden(global, 'ServiceWorkerGlobalScope', ServiceWorkerGlobalScope);
    try {
      I.defineProperty(global, I.toStringTag, { value: 'ServiceWorkerGlobalScope', configurable: true });
    } catch {
      // A non-extensible global keeps its own tag.
    }
    for (let i = 0; i < DEDICATED_ONLY.length; i++) {
      const name = DEDICATED_ONLY[i];
      if (Dedicated && Dedicated.prototype) I.deleteProperty(Dedicated.prototype, name);
      I.deleteProperty(global, name);
    }
    I.deleteProperty(global, 'DedicatedWorkerGlobalScope');

    const classes = defineEventClasses(global);
    hidden(global, 'ExtendableEvent', classes.ExtendableEvent);
    hidden(global, 'InstallEvent', classes.InstallEvent);
    const handlers = { __proto__: null };
    for (let i = 0; i < HANDLER_EVENTS.length; i++) {
      const type = HANDLER_EVENTS[i];
      let attachedHandler = false;
      I.defineProperty(members, `on${type}`, {
        configurable: true,
        enumerable: true,
        get: () => (handlers[type] === undefined ? null : handlers[type]),
        set: (value) => {
          handlers[type] = typeof value === 'function' || (value !== null && typeof value === 'object') ? value : null;
          if (!attachedHandler) {
            attachedHandler = true;
            I.addEventListener(global, type, (event) => {
              const handler = handlers[type];
              if (typeof handler === 'function') I.apply(handler, global, [event]);
            });
          }
        },
      });
    }
    const clients = defineClients(chrome);
    hidden(global, 'Clients', clients.Clients);
    I.defineProperty(members, 'clients', { get: () => clients.clients, configurable: true, enumerable: true });
    hidden(members, 'skipWaiting', function skipWaiting() {
      return I.PromiseResolve(undefined);
    });
    const finishImports = workerImports(global, config);
    if (typeof own(config, 'scriptURL') === 'string') workerLocation(global, config.scriptURL);
    storageManager(global, own(config, 'storage'));

    let startupError = null;
    const onStartupError = (event) => {
      if (startupError === null)
        startupError = event && typeof event.message === 'string' ? event.message : 'Script evaluation failed';
    };
    I.addEventListener(global, 'error', onStartupError);

    let finished = false;
    const report = (state, detail) => {
      return context.client.call('zephium.workerLifecycle', I.JSONStringify([state, detail === null ? null : diagnosticText(detail, 1024)]), true);
    };
    const finish = (state, detail) => {
      if (finished) return;
      finished = true;
      finishImports();
      I.PromiseThen(report(state, detail), () => {
        if (state === 'activated') context.hub.release();
      }, () => {});
    };
    const lifecycle = () => {
      I.removeEventListener(global, 'error', onStartupError);
      if (startupError !== null) return finish('failed', startupError);
      if (own(config, 'install') !== true) return finish('activated', null);
      const timer = I.setTimeout(() => finish('timedOut', null), LIFECYCLE_TIMEOUT_MS);
      I.PromiseThen(classes.run(global, new classes.InstallEvent('install')), (installed) => {
        if (finished) return;
        finishImports();
        if (!installed) {
          I.clearTimeout(timer);
          return finish('failed', 'install');
        }
        context.hub.release('runtime.onInstalled');
        return I.PromiseThen(
          report('installed', null),
          () => {
            if (finished) return;
            return I.PromiseThen(classes.run(global, new classes.ExtendableEvent('activate')), (activated) => {
              I.clearTimeout(timer);
              finish(activated ? 'activated' : 'failed', activated ? null : 'activate');
            });
          },
          (error) => {
            I.clearTimeout(timer);
            finish('failed', Z.errors.messageOf(error));
          },
        );
      });
      return undefined;
    };
    I.setTimeout(lifecycle, 0);
    return context;
  }


  return I.freeze({ __proto__: null, worker, PORT_MARKER });
})
