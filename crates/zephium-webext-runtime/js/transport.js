// The v1 bridge envelope (layout in `src/envelope.rs`), a call client that
// applies the host's limits before sending, and the two byte transports: the
// native post function of the web-process module and a MessagePort.
(function transport(Z) {
  'use strict';
  const I = Z.I;

  const VERSION = 1;
  const HEADER_BYTES = 6;
  const MAX_ERROR_MESSAGE_BYTES = 16 * 1024;
  const MAX_API_NAME_BYTES = 128;
  const KIND = { __proto__: null, call: 1, reply: 2, event: 3, ack: 4, portMessage: 5, portDisconnect: 6 };
  const FROM_CONTEXT = 'fromContext';
  const TO_CONTEXT = 'toContext';
  const U32_MAX = 0xffffffff;
  const U64_MAX = (I.BigInt(1) << I.BigInt(64)) - I.BigInt(1);
  const ZERO = I.BigInt(0);

  const MIB = 1024 * 1024;
  const LIMITS = I.freeze({
    __proto__: null,
    runtimeOwnedMessageBytes: 32 * MIB,
    webRendererMessageBytes: 8 * MIB,
    inFlightBytes: 64 * MIB,
    outstandingCalls: 256,
    ratePerSecond: 2000,
    burst: 500,
  });
  const RUNTIME_OWNED = {
    __proto__: null,
    background: true,
    popup: true,
    options: true,
    extensionTab: true,
    offscreen: true,
  };
  const messageLimit = (kind) =>
    RUNTIME_OWNED[kind] === true ? LIMITS.runtimeOwnedMessageBytes : LIMITS.webRendererMessageBytes;

  const ERRORS = I.freeze({
    __proto__: null,
    tooLarge: 'Message length exceeded maximum allowed length.',
    tooManyPending: 'Too many extension API calls are pending.',
    invalidated: 'Extension context invalidated.',
    malformedReply: 'Access to extension API denied.',
  });

  const isNameByte = (code, first) =>
    (code >= 0x41 && code <= 0x5a) ||
    (code >= 0x61 && code <= 0x7a) ||
    (!first && ((code >= 0x30 && code <= 0x39) || code === 0x5f));

  function validNameBytes(bytes, at, length) {
    if (length > MAX_API_NAME_BYTES) return false;
    let segments = 1;
    let first = true;
    for (let i = 0; i < length; i++) {
      const code = bytes[at + i];
      if (code === 0x2e) {
        if (first) return false;
        segments++;
        first = true;
        continue;
      }
      if (!isNameByte(code, first)) return false;
      first = false;
    }
    return !first && segments >= 2;
  }

  function isApiName(name) {
    if (typeof name !== 'string' || name.length > MAX_API_NAME_BYTES) return false;
    const bytes = new I.Uint8Array(name.length);
    for (let i = 0; i < name.length; i++) {
      const code = I.StringCharCodeAt(name, i);
      if (code > 0x7f) return false;
      bytes[i] = code;
    }
    return validNameBytes(bytes, 0, name.length);
  }

  const isBytes = (value) => {
    try {
      return I.TypedArrayTag(value) === 'Uint8Array';
    } catch {
      return false;
    }
  };
  const isBigId = (value) => typeof value === 'bigint' && value > ZERO && value <= U64_MAX;
  const isCallId = (value) => I.isInteger(value) && value > 0 && value <= U32_MAX;

  // Encodes one frame; returns a Uint8Array or the name of the refusal.
  function encode(frame, direction, maxBytes) {
    if (frame === null || typeof frame !== 'object') return 'Malformed';
    const kind = frame.kind;
    const code = KIND[kind];
    if (code === undefined) return 'Malformed';
    const fromContext = direction === FROM_CONTEXT;
    let length = HEADER_BYTES;
    let name;
    let blob;
    let text;
    switch (kind) {
      case 'call':
      case 'event':
        if (kind === 'call' ? !isCallId(frame.id) : !isBigId(frame.seq)) return 'ZeroId';
        if (!isApiName(frame.name)) return 'InvalidName';
        if (!isBytes(frame.args)) return 'Malformed';
        name = frame.name;
        blob = frame.args;
        length += (kind === 'call' ? 4 : 8) + 1 + name.length + 4 + I.TypedArrayLength(blob);
        break;
      case 'reply':
        if (!isCallId(frame.id)) return 'ZeroId';
        if (frame.ok === true) {
          if (!isBytes(frame.value)) return 'Malformed';
          blob = frame.value;
        } else {
          if (typeof frame.error !== 'string') return 'Malformed';
          text = I.encodeUtf8(frame.error);
          if (I.TypedArrayLength(text) > MAX_ERROR_MESSAGE_BYTES) return 'ErrorTooLong';
          blob = text;
        }
        length += 4 + 1 + 4 + I.TypedArrayLength(blob);
        break;
      case 'ack':
        if (!isBigId(frame.seq)) return 'ZeroId';
        length += 8;
        break;
      case 'portMessage':
        if (!isBigId(frame.port)) return 'ZeroId';
        if (!isBytes(frame.payload)) return 'Malformed';
        blob = frame.payload;
        length += 8 + 4 + I.TypedArrayLength(blob);
        break;
      case 'portDisconnect':
        if (!isBigId(frame.port)) return 'ZeroId';
        if (frame.error !== null && frame.error !== undefined) {
          if (fromContext) return 'WrongDirection';
          if (typeof frame.error !== 'string') return 'Malformed';
          text = I.encodeUtf8(frame.error);
          if (I.TypedArrayLength(text) > MAX_ERROR_MESSAGE_BYTES) return 'ErrorTooLong';
          blob = text;
          length += 8 + 1 + 4 + I.TypedArrayLength(blob);
        } else {
          length += 8 + 1;
        }
        break;
    }
    const valid =
      kind === 'call' || kind === 'ack' ? fromContext : kind === 'reply' || kind === 'event' ? !fromContext : true;
    if (!valid) return 'WrongDirection';
    if (length > U32_MAX || length > maxBytes) return 'TooLarge';
    const out = new I.Uint8Array(length);
    const view = new I.DataView(I.TypedArrayBuffer(out));
    I.DataViewSetUint32(view, 0, length, true);
    out[4] = VERSION;
    out[5] = code;
    let at = HEADER_BYTES;
    const u32 = (value) => {
      I.DataViewSetUint32(view, at, value, true);
      at += 4;
    };
    const u64 = (value) => {
      I.DataViewSetBigUint64(view, at, value, true);
      at += 8;
    };
    const bytes = (value) => {
      const count = I.TypedArrayLength(value);
      u32(count);
      I.TypedArraySet(out, value, at);
      at += count;
    };
    const apiName = (value) => {
      out[at++] = value.length;
      for (let i = 0; i < value.length; i++) out[at++] = I.StringCharCodeAt(value, i);
    };
    switch (kind) {
      case 'call':
        u32(frame.id);
        apiName(name);
        bytes(blob);
        break;
      case 'event':
        u64(frame.seq);
        apiName(name);
        bytes(blob);
        break;
      case 'reply':
        u32(frame.id);
        out[at++] = frame.ok === true ? 0 : 1;
        bytes(blob);
        break;
      case 'ack':
        u64(frame.seq);
        break;
      case 'portMessage':
        u64(frame.port);
        bytes(blob);
        break;
      case 'portDisconnect':
        u64(frame.port);
        if (text !== undefined) {
          out[at++] = 1;
          bytes(text);
        } else {
          out[at++] = 0;
        }
        break;
    }
    return out;
  }

  // Decodes exactly one frame; returns `{frame}` or `{error}` with the same
  // refusal names and order as the Rust decoder.
  function decode(bytes, direction, maxBytes) {
    if (!isBytes(bytes)) return { __proto__: null, error: 'Malformed' };
    const total = I.TypedArrayLength(bytes);
    if (total > maxBytes) return { __proto__: null, error: 'TooLarge' };
    const view = new I.DataView(I.TypedArrayBuffer(bytes), I.TypedArrayByteOffset(bytes), total);
    let at = 0;
    const fail = (error) => {
      throw { __proto__: null, decodeError: error };
    };
    const need = (count) => {
      if (count > total - at) fail('Truncated');
    };
    const u8 = () => {
      need(1);
      return bytes[at++];
    };
    const u32 = () => {
      need(4);
      const value = I.DataViewGetUint32(view, at, true);
      at += 4;
      return value;
    };
    const u64 = () => {
      need(8);
      const value = I.DataViewGetBigUint64(view, at, true);
      at += 8;
      return value;
    };
    const flag = () => {
      const value = u8();
      if (value > 1) fail('InvalidFlag');
      return value === 1;
    };
    const blob = () => {
      const length = u32();
      need(length);
      const value = I.TypedArraySubarray(bytes, at, at + length);
      at += length;
      return value;
    };
    const apiName = () => {
      const length = u8();
      need(length);
      if (!validNameBytes(bytes, at, length)) fail('InvalidName');
      let name = '';
      for (let i = 0; i < length; i++) name += I.StringFromCharCode(bytes[at + i]);
      at += length;
      return name;
    };
    const text = () => {
      const raw = blob();
      if (I.TypedArrayLength(raw) > MAX_ERROR_MESSAGE_BYTES) fail('ErrorTooLong');
      try {
        return I.decodeUtf8(raw);
      } catch {
        return fail('InvalidUtf8');
      }
    };
    const callId = () => {
      const value = u32();
      if (value === 0) fail('ZeroId');
      return value;
    };
    const bigId = () => {
      const value = u64();
      if (value === ZERO) fail('ZeroId');
      return value;
    };
    try {
      const declared = u32();
      if (declared !== total) fail('LengthMismatch');
      const version = u8();
      if (version !== VERSION) fail('UnsupportedVersion');
      const kind = u8();
      const fromContext = direction === FROM_CONTEXT;
      let frame;
      if (kind === KIND.call && fromContext) {
        frame = { __proto__: null, kind: 'call', id: callId(), name: apiName(), args: blob() };
      } else if (kind === KIND.reply && !fromContext) {
        const id = callId();
        frame = flag()
          ? { __proto__: null, kind: 'reply', id, ok: false, error: text() }
          : { __proto__: null, kind: 'reply', id, ok: true, value: blob() };
      } else if (kind === KIND.event && !fromContext) {
        frame = { __proto__: null, kind: 'event', seq: bigId(), name: apiName(), args: blob() };
      } else if (kind === KIND.ack && fromContext) {
        frame = { __proto__: null, kind: 'ack', seq: bigId() };
      } else if (kind === KIND.portMessage) {
        frame = { __proto__: null, kind: 'portMessage', port: bigId(), payload: blob() };
      } else if (kind === KIND.portDisconnect) {
        const port = bigId();
        let error = null;
        if (flag()) {
          if (fromContext) fail('WrongDirection');
          error = text();
        }
        frame = { __proto__: null, kind: 'portDisconnect', port, error };
      } else if (kind >= KIND.call && kind <= KIND.ack) {
        fail('WrongDirection');
      } else {
        fail('UnknownKind');
      }
      if (at !== total) fail('TrailingBytes');
      return { __proto__: null, frame };
    } catch (thrown) {
      if (thrown !== null && typeof thrown === 'object' && I.getPrototypeOf(thrown) === null) {
        return { __proto__: null, error: thrown.decodeError };
      }
      throw thrown;
    }
  }

  // A FIFO of frames released at the host's rate, with a margin so frames the
  // IPC delivers back to back still fit the host's bucket.
  function createScheduler(send, options) {
    const rate = (options.ratePerSecond * 9) / 10 / 1000;
    const burst = I.MathFloor((options.burst * 9) / 10);
    const maxQueuedBytes = options.maxQueuedBytes;
    const maxQueued = options.maxQueued;
    const queue = new I.Map();
    let head = 0;
    let tail = 0;
    let queuedBytes = 0;
    let tokens = burst;
    let refilledAt = I.now();
    let timer = null;
    let closed = false;
    const refill = () => {
      const current = I.now();
      const earned = I.MathFloor((current - refilledAt) * rate);
      if (earned > 0) {
        tokens = I.MathMin(burst, tokens + earned);
        refilledAt = tokens === burst ? current : refilledAt + earned / rate;
      }
    };
    const pump = () => {
      timer = null;
      refill();
      while (head < tail && tokens > 0 && !closed) {
        const entry = I.MapGet(queue, head);
        I.MapDelete(queue, head);
        head++;
        queuedBytes -= I.TypedArrayLength(entry.bytes);
        tokens--;
        const sent = send(entry.bytes);
        if (entry.done) entry.done(sent);
      }
      if (head < tail && !closed && timer === null) {
        timer = I.setTimeout(pump, I.MathMax(1, I.MathFloor(1 / rate)));
      }
    };
    return {
      __proto__: null,
      // Queues a frame; `done(sent)` runs once it left. False when full.
      push(bytes, done) {
        if (closed) return false;
        if (I.MapSize(queue) >= maxQueued || queuedBytes + I.TypedArrayLength(bytes) > maxQueuedBytes) return false;
        I.MapSet(queue, tail++, { __proto__: null, bytes, done });
        queuedBytes += I.TypedArrayLength(bytes);
        if (timer === null) pump();
        return true;
      },
      close() {
        closed = true;
        if (timer !== null) I.clearTimeout(timer);
        timer = null;
        I.MapForEach(queue, (entry) => {
          if (entry.done) entry.done(false);
        });
        I.MapClear(queue);
        head = tail = 0;
        queuedBytes = 0;
      },
    };
  }

  // The call client of one context. `link` is `{send(bytes) -> boolean,
  // listen(receiver)}`. Call ids are unique among this client's outstanding
  // calls and stay within [idBase, idLimit].
  function createClient(link, options) {
    const maxBytes = options.maxMessageBytes;
    const idBase = options.idBase || 1;
    const idLimit = options.idLimit || 0x7fffffff;
    const handlers = options.handlers;
    const pending = new I.Map();
    const waiting = new I.Map();
    let waitingHead = 0;
    let waitingTail = 0;
    let nextId = idBase;
    let inFlightBytes = 0;
    let invalidated = null;
    let dropped = 0;
    const scheduler = createScheduler((bytes) => link.send(bytes), {
      ratePerSecond: LIMITS.ratePerSecond,
      burst: LIMITS.burst,
      maxQueuedBytes: LIMITS.inFlightBytes,
      maxQueued: 16384,
    });

    const allocate = () => {
      for (let tries = 0; tries <= idLimit - idBase; tries++) {
        const id = nextId;
        nextId = nextId >= idLimit ? idBase : nextId + 1;
        if (!I.MapHas(pending, id)) return id;
      }
      return 0;
    };

    const settle = (id, ok, value) => {
      const entry = I.MapGet(pending, id);
      if (entry === undefined) return;
      I.MapDelete(pending, id);
      inFlightBytes -= entry.bytes;
      if (ok) entry.resolve(value);
      else entry.reject(value);
      drainWaiting();
    };

    const dispatchCall = (name, argsText, resolve, reject) => {
      const id = allocate();
      if (id === 0) return reject(ERRORS.tooManyPending);
      const bytes = encode(
        { __proto__: null, kind: 'call', id, name, args: I.encodeUtf8(argsText) },
        FROM_CONTEXT,
        maxBytes,
      );
      if (typeof bytes === 'string') return reject(bytes === 'TooLarge' ? ERRORS.tooLarge : ERRORS.malformedReply);
      const size = I.TypedArrayLength(bytes);
      I.MapSet(pending, id, { __proto__: null, resolve, reject, bytes: size });
      inFlightBytes += size;
      const queued = scheduler.push(bytes, (sent) => {
        if (!sent) settle(id, false, invalidated || ERRORS.invalidated);
      });
      if (!queued) settle(id, false, invalidated || ERRORS.tooManyPending);
    };

    const hasRoom = (bytes) =>
      I.MapSize(pending) < LIMITS.outstandingCalls && inFlightBytes + bytes <= LIMITS.inFlightBytes;

    function drainWaiting() {
      while (waitingHead < waitingTail && hasRoom(0)) {
        const entry = I.MapGet(waiting, waitingHead);
        I.MapDelete(waiting, waitingHead);
        waitingHead++;
        dispatchCall(entry.name, entry.argsText, entry.resolve, entry.reject);
      }
    }

    const receive = (bytes) => {
      const decoded = decode(bytes, TO_CONTEXT, maxBytes);
      if (decoded.error !== undefined) {
        dropped++;
        return;
      }
      const frame = decoded.frame;
      if (invalidated !== null) return;
      switch (frame.kind) {
        case 'reply': {
          if (!frame.ok) return settle(frame.id, false, frame.error);
          let value;
          try {
            value = I.JSONParse(I.decodeUtf8(frame.value));
          } catch {
            return settle(frame.id, false, ERRORS.malformedReply);
          }
          return settle(frame.id, true, value);
        }
        case 'event': {
          let text;
          try {
            text = I.decodeUtf8(frame.args);
          } catch {
            dropped++;
            return undefined;
          }
          return handlers.event(frame.seq, frame.name, text);
        }
        case 'portMessage':
          return handlers.portMessage(frame.port, frame.payload);
        case 'portDisconnect':
          return handlers.portDisconnect(frame.port, frame.error);
      }
      return undefined;
    };
    link.listen(receive);

    const sendFrame = (frame) => {
      if (invalidated !== null) return ERRORS.invalidated;
      const bytes = encode(frame, FROM_CONTEXT, maxBytes);
      if (typeof bytes === 'string') return bytes === 'TooLarge' ? ERRORS.tooLarge : ERRORS.malformedReply;
      return scheduler.push(bytes, null) ? null : ERRORS.tooManyPending;
    };

    return {
      __proto__: null,
      maxMessageBytes: maxBytes,
      // Sends an API call; settles with the parsed JSON result or rejects
      // with the error text. Internal calls wait for a free slot instead of
      // failing when the context is at its outstanding-call limit.
      call(name, argsText, internal) {
        return new I.Promise((resolve, reject) => {
          if (invalidated !== null) return reject(invalidated);
          const bytes = argsText.length;
          if (!hasRoom(bytes) || waitingTail > waitingHead) {
            if (!internal) return reject(ERRORS.tooManyPending);
            if (I.MapSize(waiting) >= 4096) return reject(ERRORS.tooManyPending);
            I.MapSet(waiting, waitingTail++, { __proto__: null, name, argsText, resolve, reject });
            return undefined;
          }
          return dispatchCall(name, argsText, resolve, reject);
        });
      },
      ack(seq) {
        return sendFrame({ __proto__: null, kind: 'ack', seq });
      },
      postPort(port, payload) {
        return sendFrame({ __proto__: null, kind: 'portMessage', port, payload });
      },
      closePort(port) {
        return sendFrame({ __proto__: null, kind: 'portDisconnect', port, error: null });
      },
      invalidate(message) {
        if (invalidated !== null) return;
        invalidated = message || ERRORS.invalidated;
        scheduler.close();
        const entries = [];
        I.MapForEach(pending, (entry) => I.ArrayPush(entries, entry));
        I.MapForEach(waiting, (entry) => I.ArrayPush(entries, entry));
        I.MapClear(pending);
        I.MapClear(waiting);
        for (let i = 0; i < entries.length; i++) entries[i].reject(invalidated);
      },
      get invalidated() {
        return invalidated;
      },
      get outstanding() {
        return I.MapSize(pending);
      },
      get dropped() {
        return dropped;
      },
    };
  }

  // The web-process module's functions: captured once, globals removed, and
  // the receiver kept alive because the module holds it only weakly.
  function nativeLink(global, postName, registerName) {
    const post = global[postName || '__zephiumPost'];
    const register = global[registerName || '__zephiumRegisterReceiver'];
    I.deleteProperty(global, postName || '__zephiumPost');
    I.deleteProperty(global, registerName || '__zephiumRegisterReceiver');
    if (typeof post !== 'function' || typeof register !== 'function') return null;
    return {
      __proto__: null,
      send(bytes) {
        try {
          return !!I.apply(post, undefined, [bytes]);
        } catch {
          return false;
        }
      },
      listen(receiver) {
        const accept = function (bytes) {
          if (isBytes(bytes)) receiver(bytes);
        };
        I.defineProperty(global, I.Symbol('zephium.receiver'), { value: accept });
        return !!I.apply(register, undefined, [accept]);
      },
    };
  }

  const arrayBufferLength = (value) => {
    try {
      return I.ArrayBufferByteLength(value);
    } catch {
      return -1;
    }
  };

  // Frames over a MessagePort as copied ArrayBuffers; outgoing frames wait
  // (bounded) until `attach` supplies the port.
  function portLink(maxBytes) {
    let port = null;
    let receiver = null;
    const early = new I.Map();
    let earlyCount = 0;
    let earlyBytes = 0;
    const post = (bytes) => {
      const buffer = I.TypedArrayBuffer(bytes);
      const copy =
        I.TypedArrayByteOffset(bytes) === 0 && I.ArrayBufferByteLength(buffer) === I.TypedArrayLength(bytes)
          ? buffer
          : I.TypedArrayBuffer(I.TypedArraySlice(bytes));
      try {
        I.MessagePortPostMessage(port, copy);
        return true;
      } catch {
        return false;
      }
    };
    const onMessage = (event) => {
      const data = I.MessageEventData(event);
      const length = arrayBufferLength(data);
      if (length < 0 || length > maxBytes || receiver === null) return;
      receiver(new I.Uint8Array(data));
    };
    return {
      __proto__: null,
      send(bytes) {
        if (port !== null) return post(bytes);
        if (earlyCount >= 4096 || earlyBytes + I.TypedArrayLength(bytes) > LIMITS.inFlightBytes) return false;
        I.MapSet(early, earlyCount++, bytes);
        earlyBytes += I.TypedArrayLength(bytes);
        return true;
      },
      listen(fn) {
        receiver = fn;
      },
      attach(value) {
        if (port !== null) return false;
        port = value;
        I.addEventListener(port, 'message', onMessage);
        I.MessagePortStart(port);
        for (let i = 0; i < earlyCount; i++) post(I.MapGet(early, i));
        I.MapClear(early);
        earlyCount = earlyBytes = 0;
        return true;
      },
    };
  }

  return I.freeze({
    __proto__: null,
    VERSION,
    LIMITS,
    ERRORS,
    FROM_CONTEXT,
    TO_CONTEXT,
    messageLimit,
    isApiName,
    encode,
    decode,
    createClient,
    nativeLink,
    portLink,
    arrayBufferLength,
  });
})
