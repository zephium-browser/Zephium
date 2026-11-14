// Captures every built-in the binding uses, before any extension or page code
// runs. Later code reaches built-ins only through this record, so replacing a
// global or a prototype method afterwards cannot redirect the binding.
(function intrinsics(global) {
  'use strict';
  const R = Reflect;
  const apply = R.apply;
  const uncurry = (fn) =>
    function (self, ...args) {
      return apply(fn, self, args);
    };
  const getter = (proto, name) => {
    const descriptor = R.getOwnPropertyDescriptor(proto, name);
    return descriptor && descriptor.get ? uncurry(descriptor.get) : undefined;
  };
  const method = (proto, name) => (proto && typeof proto[name] === 'function' ? uncurry(proto[name]) : undefined);
  const bound = (target, name) => {
    const fn = target && target[name];
    return typeof fn === 'function'
      ? function (...args) {
          return apply(fn, target, args);
        }
      : undefined;
  };

  const ObjectCtor = Object;
  const ArrayCtor = Array;
  const StringProto = String.prototype;
  const MapProto = Map.prototype;
  const SetProto = Set.prototype;
  const WeakMapProto = WeakMap.prototype;
  const PromiseCtor = Promise;
  const TypedArrayProto = R.getPrototypeOf(Uint8Array.prototype);
  const DataViewProto = DataView.prototype;
  const encoder = typeof TextEncoder === 'function' ? new TextEncoder() : undefined;
  const decoder = typeof TextDecoder === 'function' ? new TextDecoder('utf-8', { fatal: true }) : undefined;
  const URLCtor = typeof URL === 'function' ? URL : undefined;
  const URLProto = URLCtor && URLCtor.prototype;
  const consoleObject = global.console;
  const EventTargetProto = typeof EventTarget === 'function' ? EventTarget.prototype : undefined;
  const EventProto = typeof Event === 'function' ? Event.prototype : undefined;
  const MessageEventProto = typeof MessageEvent === 'function' ? MessageEvent.prototype : undefined;
  const MessagePortProto = typeof MessagePort === 'function' ? MessagePort.prototype : undefined;
  const ElementProto = typeof Element === 'function' ? Element.prototype : undefined;
  const DocumentProto = typeof Document === 'function' ? Document.prototype : undefined;
  const hasOwnProperty = uncurry(ObjectCtor.prototype.hasOwnProperty);
  const encode = encoder && uncurry(encoder.encode);
  const decode = decoder && uncurry(decoder.decode);
  const setTimeoutFn = global.setTimeout;
  const clearTimeoutFn = global.clearTimeout;
  const queueMicrotaskFn = global.queueMicrotask;
  const performanceObject = global.performance;
  const now =
    performanceObject && typeof performanceObject.now === 'function' ? bound(performanceObject, 'now') : Date.now;
  const PromiseThen = uncurry(PromiseCtor.prototype.then);
  const PromiseResolve = bound(PromiseCtor, 'resolve');
  const PromiseReject = bound(PromiseCtor, 'reject');

  const record = {
    __proto__: null,
    global,
    apply,
    construct: R.construct,
    uncurry,
    ownKeys: R.ownKeys,
    getOwnPropertyDescriptor: R.getOwnPropertyDescriptor,
    getPrototypeOf: R.getPrototypeOf,
    setPrototypeOf: R.setPrototypeOf,
    defineProperty: ObjectCtor.defineProperty,
    deleteProperty: R.deleteProperty,
    create: ObjectCtor.create,
    freeze: ObjectCtor.freeze,
    isFrozen: ObjectCtor.isFrozen,
    keys: ObjectCtor.keys,
    hasOwn: ObjectCtor.hasOwn || ((object, key) => hasOwnProperty(object, key)),
    ObjectPrototype: ObjectCtor.prototype,
    isArray: ArrayCtor.isArray,
    ArrayPush: uncurry(ArrayCtor.prototype.push),
    ArraySlice: uncurry(ArrayCtor.prototype.slice),
    ArraySplice: uncurry(ArrayCtor.prototype.splice),
    ArrayIndexOf: uncurry(ArrayCtor.prototype.indexOf),
    ArrayJoin: uncurry(ArrayCtor.prototype.join),
    JSONParse: JSON.parse,
    JSONStringify: JSON.stringify,
    String,
    StringFromCharCode: String.fromCharCode,
    StringSlice: uncurry(StringProto.slice),
    StringIndexOf: uncurry(StringProto.indexOf),
    StringLastIndexOf: uncurry(StringProto.lastIndexOf),
    StringToLowerCase: uncurry(StringProto.toLowerCase),
    StringToUpperCase: uncurry(StringProto.toUpperCase),
    StringCharCodeAt: uncurry(StringProto.charCodeAt),
    StringStartsWith: uncurry(StringProto.startsWith),
    StringEndsWith: uncurry(StringProto.endsWith),
    FunctionToString: uncurry(Function.prototype.toString),
    isInteger: Number.isInteger,
    isFinite: Number.isFinite,
    MathMin: Math.min,
    MathMax: Math.max,
    MathFloor: Math.floor,
    Symbol,
    SymbolFor: Symbol.for,
    hasInstance: Symbol.hasInstance,
    toStringTag: Symbol.toStringTag,
    Map,
    MapGet: uncurry(MapProto.get),
    MapSet: uncurry(MapProto.set),
    MapHas: uncurry(MapProto.has),
    MapDelete: uncurry(MapProto.delete),
    MapForEach: uncurry(MapProto.forEach),
    MapSize: getter(MapProto, 'size'),
    MapClear: uncurry(MapProto.clear),
    Set,
    SetAdd: uncurry(SetProto.add),
    SetHas: uncurry(SetProto.has),
    SetDelete: uncurry(SetProto.delete),
    SetForEach: uncurry(SetProto.forEach),
    SetSize: getter(SetProto, 'size'),
    WeakMap,
    WeakMapGet: uncurry(WeakMapProto.get),
    WeakMapSet: uncurry(WeakMapProto.set),
    WeakMapHas: uncurry(WeakMapProto.has),
    Promise: PromiseCtor,
    PromiseThen,
    PromiseResolve,
    PromiseReject,
    Error,
    TypeError,
    RangeError,
    DOMException: typeof DOMException === 'function' ? DOMException : undefined,
    Uint8Array,
    TypedArrayTag: getter(TypedArrayProto, Symbol.toStringTag),
    TypedArrayLength: getter(TypedArrayProto, 'length'),
    TypedArrayBuffer: getter(TypedArrayProto, 'buffer'),
    TypedArrayByteOffset: getter(TypedArrayProto, 'byteOffset'),
    TypedArraySet: uncurry(TypedArrayProto.set),
    TypedArraySubarray: uncurry(TypedArrayProto.subarray),
    TypedArraySlice: uncurry(TypedArrayProto.slice),
    ArrayBuffer,
    isView: ArrayBuffer.isView,
    ArrayBufferByteLength: getter(ArrayBuffer.prototype, 'byteLength'),
    DataView,
    DataViewGetUint32: uncurry(DataViewProto.getUint32),
    DataViewSetUint32: uncurry(DataViewProto.setUint32),
    DataViewGetBigUint64: uncurry(DataViewProto.getBigUint64),
    DataViewSetBigUint64: uncurry(DataViewProto.setBigUint64),
    BigInt,
    encodeUtf8: encode ? (text) => encode(encoder, text) : undefined,
    decodeUtf8: decode ? (bytes) => decode(decoder, bytes) : undefined,
    URL: URLCtor,
    URLHref: URLProto && getter(URLProto, 'href'),
    URLProtocol: URLProto && getter(URLProto, 'protocol'),
    URLHost: URLProto && getter(URLProto, 'host'),
    URLHostname: URLProto && getter(URLProto, 'hostname'),
    URLPort: URLProto && getter(URLProto, 'port'),
    URLPathname: URLProto && getter(URLProto, 'pathname'),
    URLSearch: URLProto && getter(URLProto, 'search'),
    URLHash: URLProto && getter(URLProto, 'hash'),
    URLOrigin: URLProto && getter(URLProto, 'origin'),
    URLSetHostname: URLProto && uncurry(R.getOwnPropertyDescriptor(URLProto, 'hostname').set),
    consoleError: bound(consoleObject, 'error') || (() => {}),
    consoleWarn: bound(consoleObject, 'warn') || (() => {}),
    setTimeout: (fn, ms) => apply(setTimeoutFn, global, [fn, ms]),
    clearTimeout: (id) => apply(clearTimeoutFn, global, [id]),
    queueMicrotask:
      typeof queueMicrotaskFn === 'function'
        ? (fn) => apply(queueMicrotaskFn, global, [fn])
        : (fn) => PromiseThen(PromiseResolve(), fn),
    now,
    addEventListener: method(EventTargetProto, 'addEventListener'),
    removeEventListener: method(EventTargetProto, 'removeEventListener'),
    dispatchEvent: method(EventTargetProto, 'dispatchEvent'),
    stopImmediatePropagation: method(EventProto, 'stopImmediatePropagation'),
    preventDefault: method(EventProto, 'preventDefault'),
    EventType: EventProto && getter(EventProto, 'type'),
    MessageEventData: MessageEventProto && getter(MessageEventProto, 'data'),
    MessageEventPorts: MessageEventProto && getter(MessageEventProto, 'ports'),
    MessagePortPostMessage: method(MessagePortProto, 'postMessage'),
    MessagePortStart: method(MessagePortProto, 'start'),
    MessagePortClose: method(MessagePortProto, 'close'),
    ElementShadowRoot: ElementProto && getter(ElementProto, 'shadowRoot'),
    isElement: ElementProto
      ? (value) => {
          try {
            getter(ElementProto, 'tagName')(value);
            return true;
          } catch {
            return false;
          }
        }
      : () => false,
    DocumentReadyState: DocumentProto && getter(DocumentProto, 'readyState'),
    Element: typeof Element === 'function' ? Element : undefined,
    MessageChannel: typeof MessageChannel === 'function' ? MessageChannel : undefined,
    Worker: typeof Worker === 'function' ? Worker : undefined,
    Event: typeof Event === 'function' ? Event : undefined,
    EventTarget: typeof EventTarget === 'function' ? EventTarget : undefined,
  };
  return ObjectCtor.freeze(record);
})
