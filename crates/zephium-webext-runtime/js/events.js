// `chrome.events.Event` objects and the context's event hub: listener interest
// reported to the host (so it can wake a sleeping background and evaluate
// filters), at-least-once delivery acknowledged per sequence number, and
// listeners isolated from each other's exceptions.
(function events(Z) {
  'use strict';
  const I = Z.I;
  const E = Z.errors;

  const SEEN_LIMIT = 4096;
  const HELD_LIMIT = 4096;
  const states = new I.WeakMap();

  const brand = (event, method) => {
    const state =
      event !== null && (typeof event === 'object' || typeof event === 'function')
        ? I.WeakMapGet(states, event)
        : undefined;
    if (state === undefined)
      throw new I.TypeError(`Illegal invocation: ${method} called on an object that is not an event`);
    return state;
  };

  const isPlainObject = (value) => value !== null && typeof value === 'object' && !I.isArray(value);
  const isStringArray = (value) => {
    if (!I.isArray(value)) return false;
    for (let i = 0; i < value.length; i++) if (typeof value[i] !== 'string') return false;
    return true;
  };

  // Chrome rejects malformed filters when the listener is added.
  function checkFilter(state, filter, extra) {
    const name = state.name;
    if (state.filter === 'url') {
      if (filter === undefined || filter === null) return null;
      if (!isPlainObject(filter) || !I.isArray(filter.url)) {
        throw E.invocation(
          `${name}.addListener`,
          'function callback, optional object filters',
          "Error at parameter 'filters': Error at property 'url': Invalid type: expected array, found " +
            (filter && filter.url === undefined ? 'undefined.' : 'object.'),
        );
      }
      for (let i = 0; i < filter.url.length; i++) {
        if (!isPlainObject(filter.url[i])) {
          throw E.invocation(
            `${name}.addListener`,
            'function callback, optional object filters',
            `Error at parameter 'filters': Error at property 'url': Error at index ${i}: Invalid type: expected events.UrlFilter, found ${typeof filter.url[i]}.`,
          );
        }
      }
      return filter;
    }
    if (state.filter === 'request') {
      const signature = 'function callback, object filter, optional array extraInfoSpec';
      if (!isPlainObject(filter)) throw E.invocation(`${name}.addListener`, signature, 'No matching signature.');
      if (!isStringArray(filter.urls)) {
        throw E.invocation(
          `${name}.addListener`,
          signature,
          "Error at parameter 'filter': Error at property 'urls': Invalid type: expected array, found " +
            (filter.urls === undefined ? 'undefined.' : typeof filter.urls + '.'),
        );
      }
      if (extra !== undefined && extra !== null && !isStringArray(extra)) {
        throw E.invocation(`${name}.addListener`, signature, 'No matching signature.');
      }
      return filter;
    }
    return null;
  }

  const prototype = I.create(I.ObjectPrototype);
  const method = (name, fn) =>
    I.defineProperty(prototype, name, { value: fn, configurable: true, writable: true, enumerable: false });
  method('addListener', function addListener(listener, filter, extra) {
    const state = brand(this, 'addListener');
    if (typeof listener !== 'function') {
      throw E.invocation(`${state.name}.addListener`, 'function callback', 'No matching signature.');
    }
    if (I.MapHas(state.listeners, listener)) return;
    const checked = checkFilter(state, filter, extra);
    const record = {
      __proto__: null,
      id: state.hub ? state.hub.nextListenerId() : 0,
      filter: checked,
      extra: state.filter === 'request' && I.isArray(extra) ? I.ArraySlice(extra) : null,
    };
    I.MapSet(state.listeners, listener, record);
    if (state.hub) state.hub.listenerAdded(state, record);
  });
  method('removeListener', function removeListener(listener) {
    const state = brand(this, 'removeListener');
    const record = I.MapGet(state.listeners, listener);
    if (record === undefined) return;
    I.MapDelete(state.listeners, listener);
    if (state.hub) state.hub.listenerRemoved(state, record);
  });
  method('hasListener', function hasListener(listener) {
    return I.MapHas(brand(this, 'hasListener').listeners, listener);
  });
  method('hasListeners', function hasListeners() {
    return I.MapSize(brand(this, 'hasListeners').listeners) > 0;
  });

  const rulesUnsupported = function () {
    throw new I.Error('This event does not support rules.');
  };

  function makeEvent(name, spec, hub) {
    const event = I.create(prototype);
    const state = {
      __proto__: null,
      name,
      event,
      hub,
      filter: spec && (spec.filter === 'url' || spec.filter === 'request') ? spec.filter : null,
      blocking: !!(spec && spec.blocking === true),
      listeners: new I.Map(),
    };
    if (spec && spec.rules === true && hub) {
      const verbs = ['addRules', 'getRules', 'removeRules'];
      for (let i = 0; i < verbs.length; i++) {
        const verb = verbs[i];
        I.defineProperty(event, verb, {
          configurable: true,
          writable: true,
          value: function (...args) {
            return hub.rules(state, verb, args);
          },
        });
      }
    } else {
      I.defineProperty(event, 'addRules', { configurable: true, writable: true, value: rulesUnsupported });
      I.defineProperty(event, 'getRules', { configurable: true, writable: true, value: rulesUnsupported });
      I.defineProperty(event, 'removeRules', { configurable: true, writable: true, value: rulesUnsupported });
    }
    I.WeakMapSet(states, event, state);
    return event;
  }

  const snapshot = (state) => {
    const entries = [];
    I.MapForEach(state.listeners, (record, listener) => I.ArrayPush(entries, { __proto__: null, listener, record }));
    return entries;
  };

  // Calls every listener present when dispatch began, each in isolation.
  // `visit(result, record)` sees each return value.
  function dispatch(state, args, visit, only) {
    const entries = snapshot(state);
    for (let i = 0; i < entries.length; i++) {
      const entry = entries[i];
      if (only !== undefined && !I.SetHas(only, entry.record.id)) continue;
      let result;
      try {
        result = I.apply(entry.listener, undefined, args);
      } catch (error) {
        E.reportListenerError(error);
        continue;
      }
      if (visit) visit(result, entry.record, entry.listener);
    }
    return entries.length;
  }

  // A listener-only event that is never reported to the host (ports).
  const local = (name) => makeEvent(name, null, null);
  const fire = (event, args) => dispatch(I.WeakMapGet(states, event), args);
  const count = (event) => I.MapSize(I.WeakMapGet(states, event).listeners);

  // The context's hub. `host` supplies `report(verb, argsArray)` for the
  // `zephium.addListener` family, `ack(seq)` and `result(seq, id, value)`.
  function createHub(host) {
    const byName = new I.Map();
    const special = new I.Map();
    const seen = new I.Map();
    let seenOrder = 0;
    const held = new I.Map();
    let heldHead = 0;
    let heldTail = 0;
    let holding = false;
    let nextId = 0;

    const report = (verb, args) => {
      I.PromiseThen(host.call(verb, args), undefined, (error) =>
        I.consoleError(`Unchecked runtime.lastError: ${E.messageOf(error)}`),
      );
    };

    const hub = {
      __proto__: null,
      nextListenerId: () => ++nextId,
      listenerAdded(state, record) {
        if (state.filter !== null) {
          report('zephium.addListener', [state.name, record.id, record.filter, record.extra]);
        } else if (I.MapSize(state.listeners) === 1) {
          report('zephium.addListener', [state.name, null, null, null]);
        }
      },
      listenerRemoved(state, record) {
        if (state.filter !== null) report('zephium.removeListener', [state.name, record.id]);
        else if (I.MapSize(state.listeners) === 0) report('zephium.removeListener', [state.name, null]);
      },
      rules(state, verb, args) {
        let callback;
        if (args.length > 0 && typeof args[args.length - 1] === 'function') {
          callback = args[args.length - 1];
          args = I.ArraySlice(args, 0, args.length - 1);
        }
        const result = host.call(`zephium.${verb}`, [state.name, args]);
        if (callback === undefined) return result;
        I.PromiseThen(
          result,
          (value) => E.invokeCallback(host.lastError, callback, [value], null),
          (error) => E.invokeCallback(host.lastError, callback, [], E.messageOf(error)),
        );
        return undefined;
      },
      // An API event object, created once per name.
      event(name, spec) {
        let event = I.MapGet(byName, name);
        if (event === undefined) {
          event = makeEvent(name, spec, hub);
          I.MapSet(byName, name, event);
        }
        return event;
      },
      // A handler for an event whose arguments are a wire protocol rather
      // than the listener arguments (messages, connections, internal).
      handle(name, handler) {
        I.MapSet(special, name, handler);
      },
      stateOf(event) {
        return I.WeakMapGet(states, event);
      },
      // While held, deliveries queue (bounded) instead of dispatching.
      hold() {
        holding = true;
      },
      release() {
        holding = false;
        while (heldHead < heldTail && !holding) {
          const entry = I.MapGet(held, heldHead);
          I.MapDelete(held, heldHead);
          heldHead++;
          deliverNow(entry.seq, entry.name, entry.text);
        }
      },
      deliver(seq, name, text) {
        const key = `${seq}`;
        if (I.MapHas(seen, key)) {
          host.ack(seq);
          return;
        }
        if (holding) {
          if (I.MapSize(held) >= HELD_LIMIT) return;
          I.MapSet(held, heldTail++, { __proto__: null, seq, name, text });
          return;
        }
        deliverNow(seq, name, text);
      },
      dispatch,
    };

    const remember = (seq) => {
      I.MapSet(seen, `${seq}`, seenOrder++);
      if (I.MapSize(seen) > SEEN_LIMIT) {
        let oldest;
        I.MapForEach(seen, (order, key) => {
          if (oldest === undefined) oldest = key;
        });
        I.MapDelete(seen, oldest);
      }
    };

    function deliverNow(seq, name, text) {
      if (I.MapHas(seen, `${seq}`)) {
        host.ack(seq);
        return;
      }
      remember(seq);
      let args;
      try {
        args = I.JSONParse(text);
      } catch {
        args = undefined;
      }
      if (I.isArray(args)) {
        const handler = I.MapGet(special, name);
        if (handler !== undefined) {
          try {
            handler(args, seq);
          } catch (error) {
            E.reportListenerError(error);
          }
        } else {
          const event = I.MapGet(byName, name);
          if (event !== undefined) deliverApi(I.WeakMapGet(states, event), args, seq);
        }
      }
      host.ack(seq);
    }

    function deliverApi(state, args, seq) {
      let only;
      if (state.filter !== null) {
        const matched = args.length > 0 ? args[args.length - 1] : undefined;
        if (!I.isArray(matched)) return;
        only = new I.Set();
        for (let i = 0; i < matched.length; i++) I.SetAdd(only, matched[i]);
        args = I.ArraySlice(args, 0, args.length - 1);
      }
      if (!state.blocking) {
        dispatch(state, args, undefined, only);
        return;
      }
      const seqText = `${seq}`;
      const entries = snapshot(state);
      for (let i = 0; i < entries.length; i++) {
        const entry = entries[i];
        if (only !== undefined && !I.SetHas(only, entry.record.id)) continue;
        const extra = entry.record.extra;
        const mode =
          extra && I.ArrayIndexOf(extra, 'asyncBlocking') !== -1
            ? 'async'
            : extra && I.ArrayIndexOf(extra, 'blocking') !== -1
              ? 'sync'
              : null;
        let answered = false;
        const answer = (value) => {
          if (answered) return;
          answered = true;
          report('zephium.eventResult', [seqText, entry.record.id, value === undefined ? null : value]);
        };
        const callArgs = I.ArraySlice(args);
        if (mode === 'async') I.ArrayPush(callArgs, answer);
        let result;
        try {
          result = I.apply(entry.listener, undefined, callArgs);
        } catch (error) {
          E.reportListenerError(error);
          if (mode !== null) answer(null);
          continue;
        }
        if (mode === 'sync') {
          if (
            result !== null &&
            (typeof result === 'object' || typeof result === 'function') &&
            typeof result.then === 'function'
          ) {
            I.PromiseThen(I.PromiseResolve(result), answer, (error) => {
              E.reportListenerError(error);
              answer(null);
            });
          } else {
            answer(result);
          }
        }
      }
    }

    return hub;
  }

  return I.freeze({ __proto__: null, createHub, local, fire, count, makeEvent, prototype });
})
