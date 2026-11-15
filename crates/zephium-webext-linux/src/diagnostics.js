(function diagnosticBinding(binding, worker) {
  'use strict';
  return function (...args) {
    const g = globalThis, apply = Reflect.apply, stringify = JSON.stringify;
    const then = Promise.prototype.then, parse = JSON.parse;
    const namespaces = new Set(Object.keys(args[worker ? 0 : 1]?.namespaces || {}));
    const encoder = new TextEncoder();
    let client, call, boundChrome, boundBrowser, count = 0;
    const early = [];
    const clean = (value) => String(value).slice(0, 4096).replace(/https?:\/\/[^\s"'<>)]*/gi, '<network-url>').replace(/(chrome-extension:\/\/[^?\s#]+)[?#][^\s)]*/g, '$1');
    const text = (value) => {
      try { return clean(value instanceof Error ? value.message + (value.stack ? '\n' + value.stack : '') : typeof value === 'string' ? value : '[non-string]'); }
      catch { return '[unreadable]'; }
    };
    const send = (event) => {
      if (count >= 512) return;
      if (count++ === 511) event = {kind: 'context-truncated'};
      if (!client) { if (early.length < 32) early.push(event); return; }
      try {
        let payload = stringify([event]);
        if (encoder.encode(payload).length > 8192) payload = stringify([{kind: 'record-too-large', originalKind: event.kind}]);
        apply(then, apply(call, client, ['zephium.diagnostic', payload, true]), [undefined, () => {}]);
      } catch {}
    };
    g.addEventListener('error', (e) => {
      if (e.target && e.target !== g) {
        const value = e.target.src || e.target.href;
        const resource = typeof value === 'string' ? value : value?.baseVal;
        send({kind: 'resource-error', resource: clean(resource || e.target.localName || 'unknown'), tag: e.target.localName});
      }
      else send({kind: 'uncaught', message: text(e.message), stack: text(e.error), source: clean(e.filename || ''), line: e.lineno, column: e.colno});
    }, true);
    for (const type of ['pagehide', 'pageshow', 'beforeunload', 'visibilitychange']) g.addEventListener(type, () => send({kind:'document-lifecycle', event:type, visibility:g.document?.visibilityState}));
    g.addEventListener('unhandledrejection', (e) => send({kind: 'unhandled-rejection', message: text(e.reason), stack: text(e.reason)}));
    g.addEventListener('securitypolicyviolation', (e) => send({kind: 'csp', directive: clean(e.effectiveDirective), resource: clean(e.blockedURI), source: clean(e.sourceFile), line: e.lineNumber}));
    for (const level of ['warn', 'error', 'info']) {
      const original = g.console && g.console[level];
      if (typeof original === 'function') g.console[level] = function (...values) {
        send({kind: 'console-' + level, message: values.slice(0, 8).map(text).join(' ').slice(0, 4096), stack: text(new Error())});
        return apply(original, this, values);
      };
    }
    const nativeFetch = g.fetch;
    if (typeof nativeFetch === 'function') g.fetch = function (...values) {
      const source = typeof values[0] === 'string' ? values[0] : values[0]?.url;
      const resource = clean(source || '[request]');
      let origin;
      try { origin = new URL(source, g.location?.href).origin; } catch { origin = '[unknown]'; }
      const stack = text(new Error('fetch call'));
      return apply(then, apply(nativeFetch, this, values), [(response) => {
        if (response.status >= 400) send({kind: 'fetch-http-error', resource, origin, status: response.status, stack});
        return response;
      }, (error) => {
        send({kind: 'fetch-failed', resource, origin, message: text(error), stack});
        throw error;
      }]);
    };
    const identity = () => {
      const n = g.navigator;
      return n ? {userAgent: n.userAgent, appVersion: n.appVersion, vendor: n.vendor,
        platform: n.platform, userAgentData: typeof n.userAgentData} : null;
    };
    const originalIdentity = identity();
    const context = apply(binding, this, args);
    client = context.client; call = client.call;
    for (const event of early.splice(0)) {
      count--;
      send(event);
    }
    const shape = (value, depth = 0) => {
      if (value === null) return 'null';
      if (depth >= 2) return Array.isArray(value) ? 'array' : typeof value;
      if (Array.isArray(value)) return value.slice(0, 16).map(v => shape(v, depth + 1));
      if (typeof value === 'object') return {type: 'object', fields: Object.values(value).slice(0, 16).map(v => shape(v, depth + 1))};
      return typeof value;
    };
    const pending = new Map();
    let callSequence = 0;
    const apiTrace = [], traceStart = Date.now();
    let traceEnded = false;
    const trace = (entry) => {
      if (!worker || traceEnded || Date.now() - traceStart > 20000) return;
      if (apiTrace.length < 2048) apiTrace.push({ms: Date.now() - traceStart, ...entry});
    };
    const commandOf = (name, payload) => {
      if (name !== 'runtime.sendMessage') return undefined;
      try {
        const command = parse(parse(payload)[1]).command;
        return typeof command === 'string' && /^[a-zA-Z][a-zA-Z0-9_-]{0,79}$/.test(command) ? command : undefined;
      } catch { return undefined; }
    };
    const snapshot = (reason) => {
      const methods = {};
      for (const entry of pending.values()) methods[entry.name] = (methods[entry.name] || 0) + 1;
      send({kind: 'api-pending', reason, outstanding: client.outstanding, methods,
        globalsReplaced: {chrome: g.chrome !== boundChrome, browser: g.browser !== boundBrowser},
        oldest: [...pending.values()].slice(0, 4)});
    };
    if (typeof g.setTimeout === 'function') {
      g.setTimeout(() => snapshot('two-seconds'), 2000);
      g.setTimeout(() => snapshot('ten-seconds'), 10000);
      if (worker) g.setTimeout(() => {
        traceEnded = true;
        const truncated = apiTrace.length === 2048;
        let batch = [];
        for (const row of apiTrace) {
          if (encoder.encode(stringify([...batch, row])).length > 7000) {
            send({kind: 'api-trace', records: batch});
            batch = [];
          }
          batch.push(row);
        }
        send({kind: 'api-trace', records: batch, complete: !truncated, truncated, durationMs: 20000});
        apiTrace.length = 0;
      }, 20000);
    }
    client.call = function (name, payload, internal) {
      const stack = text(new Error('API call'));
      const sequence = ++callSequence;
      const command = commandOf(name, payload);
      let tracingName = internal ? undefined : name;
      if (name === 'zephium.addListener' || name === 'zephium.removeListener') {
        try { tracingName = parse(payload)[0] + '.' + name.slice(8); } catch {}
      }
      if (tracingName) {
        let argumentShape;
        try { argumentShape = shape(parse(payload)); } catch { argumentShape = 'invalid-json'; }
        trace({phase:'call', sequence, name:tracingName, command, argumentShape});
      }
      if (pending.size < 4096) pending.set(sequence, {name, internal: !!internal, stack: stack.slice(0, 768)});
      const outstanding = client.outstanding;
      return apply(then, apply(call, this, [name, payload, internal]), [(result) => {
        pending.delete(sequence);
        if (tracingName) trace({phase:'resolved', sequence, name:tracingName, result: result === null || typeof result === 'boolean' || typeof result === 'number' ? result : shape(result)});
        return result;
      }, (error) => {
        if (/Too many extension API calls/.test(String(error))) snapshot('saturation-at-' + outstanding);
        pending.delete(sequence);
        if (tracingName) trace({phase:'rejected', sequence, name:tracingName, message:text(error)});
        if (!internal) {
          let argumentShape;
          try { argumentShape = shape(parse(payload)); } catch { argumentShape = 'invalid-json'; }
          const split = name.lastIndexOf('.');
          send({kind: /unsupported|not supported|unknown method|not available/i.test(String(error)) ? 'api-refusal' : 'api-failure', stack, command, namespace: name.slice(0, split), method: name.slice(split + 1), argumentShape, message: text(error)});
        }
        throw error;
      }]);
    };
    const proxies = new WeakMap();
    const localCalls = new Set(['runtime.getURL', 'runtime.getManifest', 'runtime.connect', 'runtime.connectNative', 'tabs.connect', 'extension.getURL', 'extension.getViews', 'extension.getBackgroundPage', 'dom.openOrClosedShadowRoot', 'i18n.getMessage', 'i18n.getUILanguage']);
    const safeShape = value => {
      try { return value === null ? 'null' : Array.isArray(value) ? value.slice(0, 16).map(item => item === null ? 'null' : typeof item) : typeof value; }
      catch { return 'unreadable'; }
    };
    const wrap = (target, path) => {
      const name = path.slice(path.indexOf('.') + 1);
      if (worker && typeof target === 'function' && localCalls.has(name)) {
        if (proxies.has(target)) return proxies.get(target);
        const proxy = new Proxy(target, {apply(fn, receiver, values) {
          const sequence = ++callSequence;
          trace({phase:'call', sequence, name, argumentShape:safeShape(values)});
          try {
            const result = apply(fn, receiver, values);
            trace({phase:'resolved', sequence, name, result:safeShape(result)});
            return result;
          } catch (error) {
            trace({phase:'rejected', sequence, name, message:text(error)});
            throw error;
          }
        }});
        proxies.set(target, proxy);
        return proxy;
      }
      if (!target || typeof target !== 'object' || Object.isFrozen(target)) return target;
      if (proxies.has(target)) return proxies.get(target);
      if (path !== 'chrome' && path !== 'browser' && !namespaces.has(path.slice(path.indexOf('.') + 1))) return target;
      const proxy = new Proxy(target, {get(object, key) {
        const value = Reflect.get(object, key, object);
        if (typeof key !== 'string') return value;
        if (value === undefined && key !== '__esModule' && !(key === 'lastError' && /\.(runtime|extension)$/.test(path))) send({kind: 'undefined-api', member: clean(path + '.' + key), stack: text(new Error())});
        const descriptor = Object.getOwnPropertyDescriptor(object, key);
        if (descriptor && !descriptor.configurable && descriptor.writable === false) return value;
        return wrap(value, path + '.' + key);
      }});
      proxies.set(target, proxy);
      return proxy;
    };
    g.chrome = wrap(g.chrome, 'chrome');
    g.browser = wrap(g.browser, 'browser');
    boundChrome = g.chrome; boundBrowser = g.browser;
    send({kind: 'compatibility-identity', before: originalIdentity, after: identity()});
    send({kind: 'context-ready'});
    return worker ? context : g.chrome;
  };
})
