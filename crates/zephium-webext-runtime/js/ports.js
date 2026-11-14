// Ports (`runtime.connect`, `tabs.connect`, `runtime.connectNative`) and
// one-time messages (`runtime.sendMessage`, `tabs.sendMessage`). Messages are
// JSON text the host forwards without parsing; `sender` objects come only
// from the host.
(function ports(Z) {
  'use strict';
  const I = Z.I;
  const E = Z.errors;
  const EV = Z.events;
  const T = Z.transport;

  const COULD_NOT_SERIALIZE = 'Could not serialize message.';
  const DISCONNECTED = 'Attempting to use a disconnected port object';
  const PORT_CLOSED = 'The message port closed before a response was received.';
  const NO_RESPONSE = I.freeze({ __proto__: null, callbackError: PORT_CLOSED });
  const MESSAGE_EVENTS = ['runtime.onMessage', 'runtime.onMessageExternal', 'runtime.onUserScriptMessage'];
  const CONNECT_EVENTS = [
    'runtime.onConnect',
    'runtime.onConnectExternal',
    'runtime.onUserScriptConnect',
    'runtime.onConnectNative',
  ];
  const U64_MAX = (I.BigInt(1) << I.BigInt(64)) - I.BigInt(1);

  // Host port ids travel as decimal strings: u64 does not fit a JSON number.
  const parsePortId = (value) => {
    if (typeof value !== 'string' || value.length === 0 || value.length > 20) return null;
    for (let i = 0; i < value.length; i++) {
      const code = I.StringCharCodeAt(value, i);
      if (code < 0x30 || code > 0x39) return null;
    }
    const id = I.BigInt(value);
    return id > I.BigInt(0) && id <= U64_MAX ? id : null;
  };

  // Chrome serializes with JSON.stringify: functions, symbols and undefined
  // members are dropped; cycles and BigInt throw; a top-level value that
  // stringifies to nothing cannot be sent.
  function serialize(message, limit) {
    let text;
    try {
      text = I.JSONStringify(message);
    } catch {
      throw new I.Error(COULD_NOT_SERIALIZE);
    }
    if (typeof text !== 'string') throw new I.Error(COULD_NOT_SERIALIZE);
    const bytes = I.encodeUtf8(text);
    if (I.TypedArrayLength(bytes) > limit) throw new I.Error(T.ERRORS.tooLarge);
    return { __proto__: null, text, bytes };
  }

  const parseMessage = (text) => {
    try {
      return { __proto__: null, ok: true, value: I.JSONParse(text) };
    } catch {
      return { __proto__: null, ok: false };
    }
  };

  const isThenable = (value) => {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) return false;
    try {
      return typeof value.then === 'function';
    } catch {
      return false;
    }
  };

  const portStates = new I.WeakMap();
  const portPrototype = I.create(I.ObjectPrototype);
  const portState = (port) => {
    const state = port !== null && typeof port === 'object' ? I.WeakMapGet(portStates, port) : undefined;
    if (state === undefined)
      throw new I.TypeError('Illegal invocation: Function must be called on an object of type Port');
    return state;
  };
  I.defineProperty(portPrototype, 'postMessage', {
    configurable: true,
    writable: true,
    value: function postMessage(message) {
      const state = portState(this);
      state.owner.post(state, message);
    },
  });
  I.defineProperty(portPrototype, 'disconnect', {
    configurable: true,
    writable: true,
    value: function disconnect() {
      const state = portState(this);
      state.owner.disconnect(state);
    },
  });

  // `context` supplies the client, the event hub, the lastError slot and the
  // per-message byte limit.
  function createMessaging(context) {
    const client = context.client;
    const hub = context.hub;
    const lastError = context.lastError;
    const limit = context.messageLimit;
    const open = new I.Map();
    const QUEUE_LIMIT = 4096;

    const owner = {
      __proto__: null,
      post(state, message) {
        if (!state.open) throw new I.Error(DISCONNECTED);
        const encoded = serialize(message, limit);
        if (state.id === null) {
          if (
            I.MapSize(state.queue) >= QUEUE_LIMIT ||
            state.queuedBytes + I.TypedArrayLength(encoded.bytes) > T.LIMITS.inFlightBytes
          ) {
            throw new I.Error(T.ERRORS.tooManyPending);
          }
          I.MapSet(state.queue, I.MapSize(state.queue), encoded.bytes);
          state.queuedBytes += I.TypedArrayLength(encoded.bytes);
          return;
        }
        const failure = client.postPort(state.id, encoded.bytes);
        if (failure !== null) throw new I.Error(failure);
      },
      disconnect(state) {
        if (!state.open) return;
        state.open = false;
        if (state.id !== null) {
          I.MapDelete(open, state.key);
          client.closePort(state.id);
        }
      },
    };

    function makePort(name, sender) {
      const port = I.create(portPrototype);
      const state = {
        __proto__: null,
        owner,
        port,
        id: null,
        key: null,
        open: true,
        queue: new I.Map(),
        queuedBytes: 0,
        onMessage: EV.local('Port.onMessage'),
        onDisconnect: EV.local('Port.onDisconnect'),
      };
      I.defineProperty(port, 'name', { value: name, enumerable: true, writable: true, configurable: true });
      if (sender !== undefined) {
        I.defineProperty(port, 'sender', { value: sender, enumerable: true, writable: true, configurable: true });
      }
      I.defineProperty(port, 'onDisconnect', {
        value: state.onDisconnect,
        enumerable: true,
        writable: true,
        configurable: true,
      });
      I.defineProperty(port, 'onMessage', {
        value: state.onMessage,
        enumerable: true,
        writable: true,
        configurable: true,
      });
      I.WeakMapSet(portStates, port, state);
      return state;
    }

    const bind = (state, id) => {
      state.id = id;
      state.key = `${id}`;
      I.MapSet(open, state.key, state);
    };

    const closed = (state, error) => {
      if (!state.open) return;
      state.open = false;
      if (state.key !== null) I.MapDelete(open, state.key);
      try {
        lastError.run(error, () => EV.fire(state.onDisconnect, [state.port]));
      } catch (thrown) {
        E.reportListenerError(thrown);
      }
    };

    // Opens a port through `apiName`; the port is usable at once and its
    // messages wait for the host-minted id.
    function connect(apiName, args, name) {
      const state = makePort(name, undefined);
      I.PromiseThen(
        client.call(apiName, I.JSONStringify(args), true),
        (value) => {
          const id = parsePortId(value);
          if (id === null) return closed(state, T.ERRORS.malformedReply);
          if (!state.open) {
            client.closePort(id);
            return undefined;
          }
          bind(state, id);
          const count = I.MapSize(state.queue);
          for (let i = 0; i < count; i++) {
            const failure = client.postPort(id, I.MapGet(state.queue, i));
            if (failure !== null) {
              owner.disconnect(state);
              return closed(state, failure);
            }
          }
          I.MapClear(state.queue);
          state.queuedBytes = 0;
          return undefined;
        },
        (error) => closed(state, E.messageOf(error)),
      );
      return state.port;
    }

    // Sends a one-time message; resolves with the response, or with
    // `NO_RESPONSE` when every receiver closed without answering.
    function send(apiName, leading, message, trailing) {
      const encoded = serialize(message, limit);
      const args = I.ArraySlice(leading);
      I.ArrayPush(args, encoded.text);
      for (let i = 0; i < trailing.length; i++) I.ArrayPush(args, trailing[i]);
      return new I.Promise((resolve, reject) => {
        I.PromiseThen(
          client.call(apiName, I.JSONStringify(args), false),
          (reply) => {
            if (reply === null) return resolve(NO_RESPONSE);
            if (reply === null || typeof reply !== 'object' || !I.hasOwn(reply, 'response')) {
              return reject(T.ERRORS.malformedReply);
            }
            const response = reply.response;
            if (response === null) return resolve(undefined);
            const parsed = typeof response === 'string' ? parseMessage(response) : { ok: false };
            return parsed.ok ? resolve(parsed.value) : reject(T.ERRORS.malformedReply);
          },
          reject,
        );
      });
    }

    const respond = (token, listener, response) => {
      I.PromiseThen(client.call('zephium.respond', I.JSONStringify([token, listener, response]), true), undefined, (error) =>
        I.consoleError(`Unchecked runtime.lastError: ${E.messageOf(error)}`),
      );
    };

    // `[messageJson, sender, messageId, listenerIds]`: Rust selects the exact
    // callbacks. Each reports its outcome so one closed callback cannot close
    // another callback's asynchronous response channel.
    function receiveMessage(event, args) {
      const token = parsePortId(args[2]) !== null ? args[2] : null;
      if (token === null || !I.isArray(args[3])) return;
      const selected = new I.Set();
      for (let i = 0; i < args[3].length; i++) {
        const id = args[3][i];
        if (I.isInteger(id) && id > 0 && id <= 9007199254740991) I.SetAdd(selected, id);
      }
      const parsed = typeof args[0] === 'string' ? parseMessage(args[0]) : { ok: false };
      const sender = args[1] !== null && typeof args[1] === 'object' ? args[1] : {};
      const entries = event === undefined ? [] : hub.messageListeners(event);
      for (let i = 0; i < entries.length; i++) {
        const entry = entries[i];
        const id = entry.record.id;
        if (!I.SetHas(selected, id)) continue;
        I.SetDelete(selected, id);
        if (!parsed.ok) {
          respond(token, id, null);
          continue;
        }
        let answered = false;
        let channelOpen = true;
        const sendResponse = function sendResponse(response) {
          if (answered || !channelOpen) return;
          const encoded = response === undefined ? null : serialize(response, limit);
          answered = true;
          respond(token, id, { response: encoded === null ? null : encoded.text });
        };
        let result;
        try {
          result = I.apply(entry.listener, undefined, [parsed.value, sender, sendResponse]);
        } catch (error) {
          E.reportListenerError(error);
        }
        if (answered) {
          if (isThenable(result)) I.PromiseThen(I.PromiseResolve(result), undefined, () => {});
          continue;
        }
        if (result === true) {
          respond(token, id, { async: 'callback' });
        } else if (isThenable(result)) {
          respond(token, id, { async: 'promise' });
          I.PromiseThen(
            I.PromiseResolve(result),
            (value) => {
              if (answered || !channelOpen) return;
              try {
                sendResponse(value);
              } catch (error) {
                answered = true;
                respond(token, id, { error: E.messageOf(error) });
              }
            },
            (error) => {
              if (answered || !channelOpen) return;
              answered = true;
              respond(token, id, { error: E.messageOf(error) });
            },
          );
        } else {
          channelOpen = false;
          respond(token, id, null);
        }
      }
      I.SetForEach(selected, (id) => respond(token, id, null));
    }

    // `[portId, name, sender]`: a port opened towards this context.
    function receiveConnect(event, args) {
      const id = parsePortId(args[0]);
      if (id === null) return;
      if (EV.count(event) === 0) {
        client.closePort(id);
        return;
      }
      const name = typeof args[1] === 'string' ? args[1] : '';
      const sender = args[2] !== null && typeof args[2] === 'object' ? args[2] : {};
      const state = makePort(name, sender);
      bind(state, id);
      EV.fire(event, [state.port]);
    }

    for (let i = 0; i < MESSAGE_EVENTS.length; i++) {
      const name = MESSAGE_EVENTS[i];
      hub.handle(name, (args) => {
        const event = context.eventFor(name);
        receiveMessage(event, args);
      });
    }
    for (let i = 0; i < CONNECT_EVENTS.length; i++) {
      const name = CONNECT_EVENTS[i];
      hub.handle(name, (args) => {
        const event = context.eventFor(name);
        const id = parsePortId(args[0]);
        if (event === undefined) {
          if (id !== null) client.closePort(id);
          return;
        }
        receiveConnect(event, args);
      });
    }

    return {
      __proto__: null,
      NO_RESPONSE,
      connect,
      send,
      serialize: (message) => serialize(message, limit),
      portMessage(id, payload) {
        const state = I.MapGet(open, `${id}`);
        if (state === undefined || !state.open) return;
        let text;
        try {
          text = I.decodeUtf8(payload);
        } catch {
          return;
        }
        const parsed = parseMessage(text);
        if (!parsed.ok) return;
        EV.fire(state.onMessage, [parsed.value, state.port]);
      },
      portDisconnect(id, error) {
        const state = I.MapGet(open, `${id}`);
        if (state !== undefined) closed(state, error);
      },
      // The context is gone: every port closes and its listeners hear it.
      invalidate() {
        const states = [];
        I.MapForEach(open, (state) => I.ArrayPush(states, state));
        for (let i = 0; i < states.length; i++) closed(states[i], null);
      },
      get openPorts() {
        return I.MapSize(open);
      },
    };
  }

  return I.freeze({ __proto__: null, createMessaging, parsePortId, serialize, NO_RESPONSE, PORT_CLOSED });
})
