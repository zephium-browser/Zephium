// Hidden background owner: only the worker/native message relay.
(function ownerRelay(Z) {
  'use strict';
  const I = Z.I;
  const T = Z.transport;
  const PORT_MARKER = 'zephium.worker.port.v1';
  const OWNER_ID_BASE = 0x80000000;
  const OWNER_ID_LIMIT = 0xffffffff;
  const MAX_ERROR_REPORTS = 32;

  const own = (object, key) =>
    object !== null && typeof object === 'object' && I.hasOwn(object, key) ? object[key] : undefined;
  const peekCallId = (bytes) =>
    I.TypedArrayLength(bytes) >= 10 ? (bytes[6] | (bytes[7] << 8) | (bytes[8] << 16) | (bytes[9] << 24)) >>> 0 : 0;
  const isOwnerReply = (bytes) =>
    I.TypedArrayLength(bytes) >= 10 && bytes[5] === 2 && peekCallId(bytes) >= OWNER_ID_BASE;
  const isOwnerCall = (bytes) =>
    I.TypedArrayLength(bytes) >= 10 && bytes[5] === 1 && peekCallId(bytes) >= OWNER_ID_BASE;

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

  // Runs in the hidden owner page: starts the worker, relays frames between
  // it and the native bridge, and reports its errors.
  function owner(config, options) {
    options = options || {};
    const global = options.global || I.global;
    const link = options.link !== undefined ? options.link : T.nativeLink(global);
    if (!link) return null;
    const max = T.messageLimit('background');
    let ownerReceiver = null;
    let port = null;
    const queue = new I.Map();
    let head = 0, tail = 0, queuedBytes = 0, timer = null, pumping = false, closed = false;
    const refused = (bytes) => {
      const decoded = T.decode(bytes, T.FROM_CONTEXT, max);
      if (decoded.error !== undefined) return;
      const frame = decoded.frame;
      let reply;
      if (frame.kind === 'call') reply = T.encode({kind:'reply', id:frame.id, ok:false, error:T.ERRORS.invalidated}, T.TO_CONTEXT, max);
      else if (frame.kind === 'portMessage') reply = T.encode({kind:'portDisconnect', port:frame.port, error:T.ERRORS.invalidated}, T.TO_CONTEXT, max);
      if (!reply || typeof reply === 'string') return;
      if (isOwnerCall(bytes)) {
        if (ownerReceiver) ownerReceiver(reply);
      } else if (port) {
        try { I.MessagePortPostMessage(port, I.TypedArrayBuffer(reply)); } catch {}
      }
    };
    const pump = () => {
      if (pumping || closed) return;
      pumping = true;
      while (head < tail) {
        const entry = I.MapGet(queue, head);
        let sent = false;
        try { sent = link.send(entry.bytes); } catch {}
        if (!sent && I.now() < entry.deadline) {
          if (timer === null) timer = I.setTimeout(() => { timer = null; pump(); }, 10);
          break;
        }
        I.MapDelete(queue, head++);
        queuedBytes -= I.TypedArrayLength(entry.bytes);
        if (!sent) refused(entry.bytes);
      }
      pumping = false;
    };
    const enqueue = (bytes) => {
      const size = I.TypedArrayLength(bytes);
      if (closed || I.MapSize(queue) >= T.LIMITS.outstandingCalls || queuedBytes + size > T.LIMITS.inFlightBytes) return false;
      I.MapSet(queue, tail++, {bytes: I.TypedArraySlice(bytes), deadline: I.now() + 2000});
      queuedBytes += size;
      pump();
      return true;
    };
    const ownerLink = {
      __proto__: null,
      send: enqueue,
      listen(fn) {
        ownerReceiver = fn;
      },
    };
    const client = T.createClient(ownerLink, {
      maxMessageBytes: max,
      idBase: OWNER_ID_BASE,
      idLimit: OWNER_ID_LIMIT,
      handlers: { __proto__: null, event() {}, portMessage() {}, portDisconnect() {} },
    });
    link.listen((bytes) => {
      if (isOwnerReply(bytes)) {
        if (ownerReceiver !== null) ownerReceiver(bytes);
        return;
      }
      if (port === null) return;
      const copy = I.TypedArrayBuffer(I.TypedArraySlice(bytes));
      try {
        I.MessagePortPostMessage(port, copy);
      } catch {
        // A closed channel means the worker is gone; the host notices.
      }
    });
    let reports = 0;
    const reportError = (fields) => {
      if (reports >= MAX_ERROR_REPORTS) return;
      reports++;
      const detail = {
        message: diagnosticText(fields.message, 1024),
        filename: diagnosticText(fields.filename, 2048),
        lineno: I.isInteger(fields.lineno) && fields.lineno >= 0 && fields.lineno <= 0xffffffff ? fields.lineno : 0,
        colno: I.isInteger(fields.colno) && fields.colno >= 0 && fields.colno <= 0xffffffff ? fields.colno : 0,
      };
      I.PromiseThen(client.call('zephium.workerError', I.JSONStringify([detail]), true), undefined, () => {});
    };

    const createWorker = options.createWorker || ((url, type) => new I.Worker(url, { type }));
    const createChannel = options.createChannel || (() => new I.MessageChannel());
    const type = own(config, 'type') === 'module' ? 'module' : 'classic';
    let worker;
    try {
      worker = createWorker(I.String(own(config, 'workerURL')), type);
    } catch (error) {
      reportError({ message: error && error.message });
      return null;
    }
    const channel = createChannel();
    port = channel.port1;
    I.addEventListener(port, 'message', (event) => {
      const data = I.MessageEventData(event);
      const length = T.arrayBufferLength(data);
      if (length < 6 || length > max) return;
      const bytes = new I.Uint8Array(data);
      if (isOwnerCall(bytes)) return;
      if (!enqueue(bytes)) refused(bytes);
    });
    I.MessagePortStart(port);
    I.addEventListener(worker, 'error', (event) =>
      reportError({ message: event.message, filename: event.filename, lineno: event.lineno, colno: event.colno }),
    );
    I.addEventListener(worker, 'messageerror', () => reportError({ message: 'messageerror' }));
    worker.postMessage(PORT_MARKER, [channel.port2]);
    const close = () => {
      closed = true;
      if (timer !== null) I.clearTimeout(timer);
      timer = null;
      I.MapForEach(queue, entry => refused(entry.bytes));
      I.MapClear(queue);
      queuedBytes = 0;
      client.invalidate();
    };
    return { __proto__: null, worker, client, reportError, close };
  }


  return I.freeze({ __proto__: null, owner });
})
