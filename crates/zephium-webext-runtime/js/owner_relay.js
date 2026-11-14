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
    const ownerLink = {
      __proto__: null,
      send: (bytes) => link.send(bytes),
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
      link.send(bytes);
    });
    I.MessagePortStart(port);
    I.addEventListener(worker, 'error', (event) =>
      reportError({ message: event.message, filename: event.filename, lineno: event.lineno, colno: event.colno }),
    );
    I.addEventListener(worker, 'messageerror', () => reportError({ message: 'messageerror' }));
    worker.postMessage(PORT_MARKER, [channel.port2]);
    return { __proto__: null, worker, client, reportError };
  }


  return I.freeze({ __proto__: null, owner });
})
