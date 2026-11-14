// Builds `chrome` (and `browser`, the same object) for one extension context
// from the surface data and the context's bootstrap blob. Every async method
// takes a trailing callback (with `runtime.lastError` set during it) or
// returns a promise; synchronous members are served from the bootstrap.
//
// Surface (`surface.generated.js`, one JSON value):
//   { version: 1, namespaces: { "<dotted name>": Namespace } }
//   Namespace { contexts?: Kind[], permissions?: string[] (any one suffices), manifest?: string[],
//               functions?: { name: Function }, events?: { name: Event },
//               properties?: { name: JSON constant or enum } }
//   Function  { parameters?: Param[] (no callback), returns?: "async" | "sync",
//               callback?: boolean, callbackRequired?: true, promise?: false, result?: false,
//               optionalResult?: true,
//               signature?: string, setting?: ValueSchema, contexts?, permissions? }
//   ValueSchema { type, optional?, enum?, properties?, items? } for ChromeSetting.set
//   Param     { name, type: Type | Type[], optional?: true, serializeFunctions?: string[] }
//   Type      string integer number boolean object array function binary any
//   Event     { contexts?, permissions?, filter?: "url" | "request" | "window", blocking?: true, rules?: true }
//   Kind      background popup options extensionTab offscreen extensionFrame
//             contentScript userScript externalPage
// Omitted `contexts` means every kind. "sync" functions exist only where the
// binding implements them (LOCAL below). A sync return with callback:true is
// mixed mode and requires a custom implementation; it is currently omitted.
//
// Bootstrap: { extensionId, scheme, contextKind, manifest (as getManifest
// returns it), messages ({ lowercase name: text, placeholders applied }),
// uiLanguage, incognito, permissions (granted API permissions), worker? }.
//
// Host protocol beyond plain `namespace.method` calls (arguments are the
// parameters in order, null for an omitted optional one; u64 ids are decimal
// strings): `zephium.addListener [event, listenerId, filter|null,
// extraInfoSpec|null]` and `zephium.removeListener [event, listenerId]`
// report each listener; filtered events carry the matched listener ids as a last
// argument; `zephium.eventResult [seq, listenerId, value]` for blocking
// listeners; `runtime.sendMessage [id|null, messageJson, options|null]`,
// `tabs.sendMessage [tabId, messageJson, options|null]` and
// `runtime.sendNativeMessage [app, messageJson]` reply null (no response) or
// `{response: json|null}`; message events carry `[messageJson, sender,
// messageId, listenerIds]`, answered by
// `zephium.respond [messageId, listenerId, {response}|{error}|{async:'callback'|'promise'}|null]`;
// `runtime.connect`, `tabs.connect` and `runtime.connectNative` reply with the
// port id, and connect events carry `[portId, name, sender]`; the host sends
// `zephium.onInvalidated [message|null]` and
// `zephium.onPermissionsChanged [permissions]` as events.
(function api(Z) {
  'use strict';
  const I = Z.I;
  const E = Z.errors;
  const T = Z.transport;
  const EV = Z.events;
  const P = Z.ports;

  const KINDS = [
    'background',
    'popup',
    'options',
    'extensionTab',
    'offscreen',
    'extensionFrame',
    'contentScript',
    'userScript',
    'externalPage',
  ];
  const TYPES = ['string', 'integer', 'number', 'boolean', 'object', 'array', 'function', 'binary', 'any'];
  const INT32_MIN = -2147483648;
  const INT32_MAX = 2147483647;
  const RTL = ['ar', 'dv', 'fa', 'he', 'iw', 'ku', 'ps', 'sd', 'ug', 'ur', 'yi'];

  const own = (object, key) =>
    object !== null && typeof object === 'object' && I.hasOwn(object, key) ? object[key] : undefined;
  const contains = (list, value) => I.isArray(list) && I.ArrayIndexOf(list, value) !== -1;

  const isArrayBuffer = (value) => T.arrayBufferLength(value) >= 0;
  function typeName(value) {
    if (value === null) return 'null';
    if (value === undefined) return 'undefined';
    if (typeof value === 'number')
      return I.isInteger(value) && value >= INT32_MIN && value <= INT32_MAX ? 'integer' : 'number';
    if (typeof value === 'object') {
      if (I.isArray(value)) return 'array';
      if (isArrayBuffer(value)) return 'binary';
      return 'object';
    }
    return typeof value;
  }
  function typeMatches(type, value) {
    if (I.isArray(type)) {
      for (let i = 0; i < type.length; i++) if (typeMatches(type[i], value)) return true;
      return false;
    }
    const found = typeName(value);
    switch (type) {
      case 'any':
        return true;
      case 'number':
        return found === 'number' || found === 'integer';
      case 'object':
        return found === 'object';
      default:
        return found === type;
    }
  }
  const typeLabel = (type) => (I.isArray(type) ? `[${I.ArrayJoin(type, '|')}]` : type || 'any');

  function matchesSetting(spec, value, depth) {
    if (depth >= 32) return false;
    if (value === undefined || value === null) return own(spec, 'optional') === true;
    if (!typeMatches(own(spec, 'type'), value)) return false;
    const choices = own(spec, 'enum');
    if (I.isArray(choices) && !contains(choices, value)) return false;
    const properties = own(spec, 'properties');
    if (properties !== undefined) {
      const names = I.keys(value);
      for (let i = 0; i < names.length; i++) if (!I.hasOwn(properties, names[i])) return false;
      const expected = I.keys(properties);
      for (let i = 0; i < expected.length; i++) {
        const name = expected[i];
        if (!matchesSetting(properties[name], own(value, name), depth + 1)) return false;
      }
    }
    const items = own(spec, 'items');
    if (items !== undefined) {
      for (let i = 0; i < value.length; i++) if (!matchesSetting(items, value[i], depth + 1)) return false;
    }
    return true;
  }

  function signatureOf(params, async) {
    const parts = [];
    for (let i = 0; i < params.length; i++) {
      const p = params[i];
      I.ArrayPush(parts, `${p.optional ? 'optional ' : ''}${typeLabel(p.type)} ${p.name}`);
    }
    if (async) I.ArrayPush(parts, 'optional function callback');
    return I.ArrayJoin(parts, ', ');
  }

  // Chrome's argument matching: optional parameters whose type does not fit
  // are skipped, null/undefined stand for an omitted optional parameter, and
  // anything left over or missing is "No matching signature.".
  function resolveArguments(name, signature, params, args) {
    const values = [];
    let at = 0;
    for (let i = 0; i < params.length; i++) {
      const p = params[i];
      if (at >= args.length) {
        if (!p.optional) throw E.invocation(name, signature, 'No matching signature.');
        I.ArrayPush(values, null);
        continue;
      }
      const value = args[at];
      if (value === null || value === undefined) {
        if (!p.optional) throw E.invocation(name, signature, 'No matching signature.');
        I.ArrayPush(values, null);
        at++;
        continue;
      }
      if (!typeMatches(p.type, value)) {
        if (!p.optional) {
          const detail = I.isArray(p.type)
            ? `Error at parameter '${p.name}': Value did not match any choice.`
            : `Error at parameter '${p.name}': Invalid type: expected ${typeLabel(p.type)}, found ${typeName(value)}.`;
          throw E.invocation(name, signature, detail);
        }
        I.ArrayPush(values, null);
        continue;
      }
      I.ArrayPush(values, value);
      at++;
    }
    if (at < args.length) throw E.invocation(name, signature, 'No matching signature.');
    return values;
  }

  const FIRE_AND_FORGET = I.freeze({ __proto__: null, promise: false });
  const CALLBACK = I.freeze({ __proto__: null, name: 'callback', type: 'function', optional: true });

  function normalizeParams(raw) {
    const params = [];
    if (!I.isArray(raw)) return params;
    for (let i = 0; i < raw.length; i++) {
      const p = raw[i];
      const type = own(p, 'type');
      const valid = I.isArray(type) ? type.length > 0 : contains(TYPES, type);
      I.ArrayPush(params, {
        __proto__: null,
        name: typeof own(p, 'name') === 'string' ? p.name : `arg${i}`,
        type: valid ? type : 'any',
        optional: own(p, 'optional') === true,
        serializeFunctions: I.isArray(own(p, 'serializeFunctions')) ? p.serializeFunctions : null,
      });
    }
    return params;
  }

  // `scripting.executeScript({func})` and the like send functions as source.
  function prepare(values, params) {
    const out = [];
    for (let i = 0; i < values.length; i++) {
      let value = values[i];
      const names = params[i] ? params[i].serializeFunctions : null;
      if (names !== null && value !== null && typeof value === 'object' && !I.isArray(value)) {
        const copy = { __proto__: null };
        const keys = I.keys(value);
        for (let k = 0; k < keys.length; k++) copy[keys[k]] = value[keys[k]];
        for (let n = 0; n < names.length; n++) {
          if (typeof copy[names[n]] === 'function') copy[names[n]] = I.FunctionToString(copy[names[n]]);
        }
        value = copy;
      }
      I.ArrayPush(out, value);
    }
    return out;
  }

  function deepFreeze(value) {
    if (value === null || typeof value !== 'object') return value;
    const keys = I.keys(value);
    for (let i = 0; i < keys.length; i++) deepFreeze(value[keys[i]]);
    return I.freeze(value);
  }

  const define = (target, key, value) =>
    I.defineProperty(target, key, { value, writable: true, enumerable: true, configurable: true });

  // Chrome's base::ReplaceStringPlaceholders: `$1`-`$9` substitute, `$$`
  // runs keep all but one `$`, any other `$x` is dropped.
  function substitute(message, substitutions) {
    let out = '';
    for (let i = 0; i < message.length; i++) {
      const c = message[i];
      if (c !== '$') {
        out += c;
        continue;
      }
      if (i + 1 >= message.length) break;
      i++;
      if (message[i] === '$') {
        while (i < message.length && message[i] === '$') {
          out += '$';
          i++;
        }
        i--;
        continue;
      }
      const code = I.StringCharCodeAt(message, i);
      if (code < 0x31 || code > 0x39) continue;
      const index = code - 0x31;
      if (index < substitutions.length) out += substitutions[index];
    }
    return out;
  }

  function create(surface, bootstrap, link, options) {
    const global = own(options, 'global') || I.global;
    const kind = own(bootstrap, 'contextKind');
    if (!contains(KINDS, kind)) throw new I.TypeError(`Unknown extension context kind: ${kind}`);
    const extensionId = I.String(own(bootstrap, 'extensionId'));
    const scheme = typeof own(bootstrap, 'scheme') === 'string' ? bootstrap.scheme : 'chrome-extension';
    const manifestText = I.JSONStringify(own(bootstrap, 'manifest') || {});
    const manifest = I.JSONParse(manifestText);
    const messages = own(bootstrap, 'messages') || {};
    const uiLanguage = typeof own(bootstrap, 'uiLanguage') === 'string' ? bootstrap.uiLanguage : 'en-US';
    const incognito = own(bootstrap, 'incognito') === true;
    let granted = I.isArray(own(bootstrap, 'permissions')) ? I.ArraySlice(bootstrap.permissions) : [];
    const namespaces = own(surface, 'namespaces') || {};
    const lastError = E.createLastError();
    let invalid = false;

    const handlers = { __proto__: null };
    const dead = { __proto__: null, send: () => false, listen() {} };
    const client = T.createClient(link || dead, {
      maxMessageBytes: T.messageLimit(kind),
      idBase: own(options, 'idBase'),
      idLimit: own(options, 'idLimit'),
      handlers: {
        __proto__: null,
        event: (seq, name, text) => handlers.event(seq, name, text),
        portMessage: (port, payload) => handlers.portMessage(port, payload),
        portDisconnect: (port, error) => handlers.portDisconnect(port, error),
      },
    });
    if (!link) client.invalidate(T.ERRORS.invalidated);
    const callJson = (name, args, internal) => client.call(name, I.JSONStringify(args), internal);
    const hub = EV.createHub({
      __proto__: null,
      call: (name, args) => callJson(name, args, true),
      ack: (seq) => client.ack(seq),
      lastError,
    });
    const eventObjects = new I.Map();
    const chrome = {};
    const objects = new I.Map();
    const presentEvents = new I.Set();
    const memberCache = new I.Map();
    const messaging = P.createMessaging({
      __proto__: null,
      client,
      hub,
      lastError,
      messageLimit: client.maxMessageBytes,
      eventFor: (name) => (I.SetHas(presentEvents, name) ? I.MapGet(eventObjects, name) : undefined),
    });
    handlers.event = hub.deliver;
    handlers.portMessage = messaging.portMessage;
    handlers.portDisconnect = messaging.portDisconnect;

    const permitted = (list) => {
      if (!I.isArray(list) || list.length === 0) return true;
      for (let i = 0; i < list.length; i++) if (contains(granted, list[i])) return true;
      return false;
    };
    const available = (node) => {
      const contexts = own(node, 'contexts');
      const features = own(node, 'manifest');
      if (I.isArray(features)) {
        for (let i = 0; i < features.length; i++) if (!I.hasOwn(manifest, features[i]) || manifest[features[i]] === null) return false;
      }
      return (contexts === undefined || contains(contexts, kind)) && permitted(own(node, 'permissions'));
    };

    function getURL(path) {
      try {
        const absolute = new I.URL(path);
        return I.URLHref(absolute);
      } catch {
        // Relative: resolved as a standard URL under the extension origin.
      }
      const resolved = I.URLHref(new I.URL(path, `https://${extensionId}/`));
      return `${scheme}://${I.StringSlice(resolved, 8)}`;
    }

    const predefined = () => {
      const language = I.StringToLowerCase(I.StringSlice(uiLanguage, 0, 2));
      const rtl = contains(RTL, language);
      let locale = '';
      for (let i = 0; i < uiLanguage.length; i++) locale += uiLanguage[i] === '-' ? '_' : uiLanguage[i];
      return {
        __proto__: null,
        '@@extension_id': extensionId,
        '@@ui_locale': locale,
        '@@bidi_dir': rtl ? 'rtl' : 'ltr',
        '@@bidi_reversed_dir': rtl ? 'ltr' : 'rtl',
        '@@bidi_start_edge': rtl ? 'right' : 'left',
        '@@bidi_end_edge': rtl ? 'left' : 'right',
      };
    };

    function getMessage(values) {
      const name = I.StringToLowerCase(I.String(values[0]));
      let message = own(messages, name);
      if (typeof message !== 'string') message = predefined()[name];
      if (typeof message !== 'string') return '';
      const raw = values[1];
      let substitutions = [];
      if (I.isArray(raw)) {
        if (raw.length > 9) return undefined;
        for (let i = 0; i < raw.length; i++) I.ArrayPush(substitutions, I.String(raw[i]));
      } else if (raw !== null && raw !== undefined) {
        substitutions = [I.String(raw)];
      }
      if (own(values[2], 'escapeLt') === true) {
        let escaped = '';
        for (let i = 0; i < message.length; i++) escaped += message[i] === '<' ? '&lt;' : message[i];
        message = escaped;
      }
      return substitute(message, substitutions);
    }

    const checkConnectInfo = (name, signature, info) => {
      if (info === null) return '';
      const portName = own(info, 'name');
      if (portName === undefined) return '';
      if (typeof portName !== 'string') {
        throw E.invocation(
          name,
          signature,
          `Error at parameter 'connectInfo': Error at property 'name': Invalid type: expected string, found ${typeName(portName)}.`,
        );
      }
      return portName;
    };

    const shadowRoot = (values, name, signature) => {
      const element = values[0];
      if (!I.isElement(element)) {
        throw E.invocation(
          name,
          signature,
          "Error at parameter 'element': Value must be an instance of HTMLElement.",
        );
      }
      return I.ElementShadowRoot ? I.ElementShadowRoot(element) : null;
    };

    // Members the binding serves itself. Sync ones return a value; async ones
    // return a promise (or `P.NO_RESPONSE`).
    const LOCAL = {
      __proto__: null,
      'runtime.getURL': (v) => getURL(I.String(v[0])),
      'extension.getURL': (v) => getURL(I.String(v[0])),
      'runtime.getManifest': () => I.JSONParse(manifestText),
      'i18n.getMessage': getMessage,
      'i18n.getUILanguage': () => uiLanguage,
      'extension.getViews': () => [],
      'extension.getBackgroundPage': () => null,
      'dom.openOrClosedShadowRoot': shadowRoot,
      'runtime.connect': (v, name, signature) =>
        messaging.connect('runtime.connect', [v[0], v[1]], checkConnectInfo(name, signature, v[1])),
      'tabs.connect': (v, name, signature) =>
        messaging.connect('tabs.connect', [v[0], v[1]], checkConnectInfo(name, signature, v[1])),
      'runtime.connectNative': (v) => messaging.connect('runtime.connectNative', [v[0]], ''),
      'runtime.sendMessage': (v) => messaging.send('runtime.sendMessage', [v[0]], v[1], [v[2]]),
      'tabs.sendMessage': (v) => messaging.send('tabs.sendMessage', [v[0]], v[1], [v[2]]),
      'runtime.sendNativeMessage': (v) => messaging.send('runtime.sendNativeMessage', [v[0]], v[1], []),
    };

    // Chrome's MassageSendMessageArguments: a two-argument call is
    // (message, options) only when the first cannot be an extension id.
    const massageSendMessage = (args) => {
      args = I.ArraySlice(args);
      let callback = null;
      if (args.length > 0 && typeof args[args.length - 1] === 'function')
        callback = I.ArraySplice(args, args.length - 1, 1)[0];
      let target = null;
      let message;
      let messageOptions = null;
      let extra = false;
      switch (args.length) {
        case 0:
          break;
        case 1:
          message = args[0];
          break;
        case 2:
          if (typeof args[0] === 'string' || args[0] === null || args[0] === undefined) {
            target = args[0];
            message = args[1];
          } else {
            message = args[0];
            messageOptions = args[1];
          }
          break;
        case 3:
          target = args[0];
          message = args[1];
          messageOptions = args[2];
          break;
        default:
          extra = true;
      }
      return { __proto__: null, values: [target, message, messageOptions, callback], extra };
    };

    const sendMessageParams = [
      { __proto__: null, name: 'extensionId', type: 'string', optional: true },
      { __proto__: null, name: 'message', type: 'any', optional: false },
      { __proto__: null, name: 'options', type: 'object', optional: true },
      CALLBACK,
    ];
    const massaged = (name, signature, args) => {
      const shaped = massageSendMessage(args);
      if (shaped.extra) throw E.invocation(name, signature, 'No matching signature.');
      const values = shaped.values;
      for (let i = 0; i < sendMessageParams.length; i++) {
        const p = sendMessageParams[i];
        const value = values[i];
        if (value === null || value === undefined) {
          if (!p.optional) throw E.invocation(name, signature, 'No matching signature.');
          values[i] = null;
        } else if (!typeMatches(p.type, value)) {
          throw E.invocation(
            name,
            signature,
            `Error at parameter '${p.name}': Invalid type: expected ${p.type}, found ${typeName(value)}.`,
          );
        }
      }
      return values;
    };

    const ensureValid = () => {
      if (invalid) throw new I.Error(T.ERRORS.invalidated);
    };

    function settle(promise, callback, spec) {
      const withResult = own(spec, 'result') !== false;
      const optionalResult = own(spec, 'optionalResult') === true;
      const resultValue = (value) => optionalResult && value === null ? undefined : value;
      if (callback !== null) {
        I.PromiseThen(
          promise,
          (value) => {
            if (value === P.NO_RESPONSE) return E.invokeCallback(lastError, callback, [], P.PORT_CLOSED);
            const args = withResult && !(optionalResult && value === null) ? [value] : [];
            return E.invokeCallback(lastError, callback, args, null);
          },
          (failure) => E.invokeCallback(lastError, callback, [], E.messageOf(failure)),
        );
        return undefined;
      }
      if (own(spec, 'promise') === false) {
        I.PromiseThen(promise, undefined, (failure) => lastError.run(E.messageOf(failure), () => undefined));
        return undefined;
      }
      return new I.Promise((resolve, reject) => {
        I.PromiseThen(
          promise,
          (value) => resolve(value === P.NO_RESPONSE || !withResult ? undefined : resultValue(value)),
          (failure) => reject(new I.Error(E.messageOf(failure))),
        );
      });
    }

    function makeFunction(fullName, spec) {
      const returns = own(spec, 'returns') === 'sync' ? 'sync' : 'async';
      if (returns === 'sync' && own(spec, 'callback') === true) return undefined;
      const local = LOCAL[fullName];
      if (returns === 'sync' && local === undefined) return undefined;
      const params = normalizeParams(own(spec, 'parameters'));
      const async = returns === 'async';
      const takesCallback = async && own(spec, 'callback') !== false;
      const displayName = I.StringStartsWith(fullName, 'storage.')
        ? `storage.${I.StringSlice(fullName, I.StringLastIndexOf(fullName, '.') + 1)}`
        : I.StringStartsWith(fullName, 'privacy.')
          ? `types.ChromeSetting.${I.StringSlice(fullName, I.StringLastIndexOf(fullName, '.') + 1)}`
          : fullName;
      const signature =
        typeof own(spec, 'signature') === 'string' ? spec.signature : signatureOf(params, takesCallback);
      const parsed = takesCallback ? I.ArraySlice(params) : params;
      if (takesCallback)
        I.ArrayPush(parsed, own(spec, 'callbackRequired') === true
          ? { __proto__: null, name: 'callback', type: 'function', optional: false }
          : CALLBACK);
      const usesMassage = fullName === 'runtime.sendMessage';
      const member = I.StringSlice(fullName, I.StringLastIndexOf(fullName, '.') + 1);
      const fn = {
        [member](...args) {
          ensureValid();
          const setting = own(spec, 'setting');
          if (setting !== undefined && args.length > 0 && !matchesSetting(setting, args[0], 0))
            throw new I.TypeError('Invalid invocation');
          const values = usesMassage
            ? massaged(displayName, signature, args)
            : resolveArguments(displayName, signature, parsed, args);
          if (!async) return local(values, fullName, signature);
          const callback = takesCallback ? values[values.length - 1] : null;
          const callValues = takesCallback ? I.ArraySlice(values, 0, values.length - 1) : values;
          let promise;
          if (local !== undefined) {
            promise = local(callValues, fullName, signature);
          } else {
            let text;
            try {
              text = I.JSONStringify(prepare(callValues, params));
            } catch {
              throw E.invocation(displayName, signature, 'Value is unserializable.');
            }
            promise = client.call(fullName, text, false);
          }
          return settle(promise, callback, takesCallback ? spec : FIRE_AND_FORGET);
        },
      }[member];
      return fn;
    }

    const runtimeExtras = (target) => {
      I.defineProperty(target, 'id', {
        configurable: true,
        enumerable: true,
        get: () => (invalid ? undefined : extensionId),
        set() {},
      });
      lastError.install(target);
    };
    const extensionExtras = (target) => {
      define(target, 'inIncognitoContext', incognito);
      lastError.install(target);
    };

    const keys = I.keys(namespaces);
    const depth = (key) => {
      let count = 0;
      for (let i = 0; i < key.length; i++) if (key[i] === '.') count++;
      return count;
    };
    const ordered = [];
    for (let d = 0; d < 8; d++) {
      for (let i = 0; i < keys.length; i++) if (depth(keys[i]) === d) I.ArrayPush(ordered, keys[i]);
    }

    const presence = new I.Map();
    const isPresent = (key) => I.MapGet(presence, key) === true;
    const clearPresentEvents = () => {
      const names = [];
      I.SetForEach(presentEvents, (name) => I.ArrayPush(names, name));
      for (let i = 0; i < names.length; i++) I.SetDelete(presentEvents, names[i]);
    };

    // Brings every namespace and member in line with the context kind and
    // the granted permissions: unavailable ones are absent, not stubs.
    function refresh() {
      clearPresentEvents();
      for (let n = 0; n < ordered.length; n++) {
        const key = ordered[n];
        const node = namespaces[key];
        const dot = I.StringLastIndexOf(key, '.');
        const parentKey = dot === -1 ? null : I.StringSlice(key, 0, dot);
        const parent = parentKey === null ? chrome : I.MapGet(objects, parentKey);
        const leaf = I.StringSlice(key, dot + 1);
        let target = I.MapGet(objects, key);
        if ((parentKey !== null && !isPresent(parentKey)) || !available(node)) {
          if (target !== undefined && parent !== undefined && parent[leaf] === target) I.deleteProperty(parent, leaf);
          I.MapSet(presence, key, false);
          continue;
        }
        if (target === undefined) {
          target = {};
          const properties = own(node, 'properties') || {};
          const propertyNames = I.keys(properties);
          for (let i = 0; i < propertyNames.length; i++) {
            define(target, propertyNames[i], deepFreeze(I.JSONParse(I.JSONStringify(properties[propertyNames[i]]))));
          }
          if (key === 'runtime') runtimeExtras(target);
          if (key === 'extension') extensionExtras(target);
          I.MapSet(objects, key, target);
        }
        I.MapSet(presence, key, true);
        const functions = own(node, 'functions') || {};
        const functionNames = I.keys(functions);
        for (let i = 0; i < functionNames.length; i++) {
          const name = functionNames[i];
          const fullName = `${key}.${name}`;
          const spec = functions[name];
          const cached = I.MapGet(memberCache, fullName);
          if (!available(spec)) {
            if (cached !== undefined && target[name] === cached) I.deleteProperty(target, name);
            continue;
          }
          const fn = I.MapHas(memberCache, fullName) ? cached : makeFunction(fullName, spec);
          I.MapSet(memberCache, fullName, fn);
          if (fn !== undefined && target[name] !== fn) define(target, name, fn);
        }
        const eventSpecs = own(node, 'events') || {};
        const eventNames = I.keys(eventSpecs);
        for (let i = 0; i < eventNames.length; i++) {
          const name = eventNames[i];
          const fullName = `${key}.${name}`;
          const spec = eventSpecs[name];
          let event = I.MapGet(eventObjects, fullName);
          if (!available(spec)) {
            if (event !== undefined && target[name] === event) I.deleteProperty(target, name);
            continue;
          }
          if (event === undefined) {
            event = hub.event(fullName, spec);
            I.MapSet(eventObjects, fullName, event);
          }
          I.SetAdd(presentEvents, fullName);
          if (target[name] !== event) define(target, name, event);
        }
        if (parent[leaf] !== target) define(parent, leaf, target);
      }
    }
    refresh();

    function invalidate(message) {
      if (invalid) return;
      invalid = true;
      client.invalidate(message || T.ERRORS.invalidated);
      messaging.invalidate();
    }
    hub.handle('zephium.onInvalidated', (args) => invalidate(typeof args[0] === 'string' ? args[0] : null));
    hub.handle('zephium.onPermissionsChanged', (args) => {
      if (!I.isArray(args[0])) return;
      granted = I.ArraySlice(args[0]);
      refresh();
    });

    if (own(options, 'defineGlobals') !== false) {
      define(global, 'chrome', chrome);
      define(global, 'browser', chrome);
    }

    return {
      __proto__: null,
      chrome,
      client,
      hub,
      messaging,
      lastError,
      invalidate,
      refresh,
      get invalidated() {
        return invalid;
      },
    };
  }

  // An extension page or offscreen document: the native bridge is in this
  // frame's default world.
  function page(surface, bootstrap) {
    return create(surface, bootstrap, T.nativeLink(I.global), {}).chrome;
  }

  return I.freeze({ __proto__: null, create, page, resolveArguments, signatureOf, substitute, typeName, KINDS });
})
