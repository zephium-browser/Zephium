// Loads the binding into a fresh realm the way `src/js.rs` assembles it, and
// plays the host over a fake link.
import { existsSync, readFileSync } from 'node:fs';
import vm from 'node:vm';

const read = (name) => {
  const url = new URL(`../${name}`, import.meta.url);
  return existsSync(url) ? readFileSync(url, 'utf8') : null;
};
// In the order of `modules!()` in `src/js.rs`.
const MODULES = [
  ['I', 'intrinsics.js', 'globalThis'],
  ['errors', 'errors.js', 'Z'],
  ['transport', 'transport.js', 'Z'],
  ['events', 'events.js', 'Z'],
  ['ports', 'ports.js', 'Z'],
  ['api', 'api.js', 'Z'],
  ['content', 'content.js', 'Z'],
  ['workerScope', 'worker_scope.js', 'Z'],
];
export const sources = Object.fromEntries(MODULES.map(([key, file]) => [key === 'I' ? 'intrinsics' : key, read(file)]));
export const fixtureSurface = JSON.parse(read('test/surface.fixture.json'));
const generatedRealm = vm.createContext({});
vm.runInContext(read('surface.generated.js'), generatedRealm);
export const generatedSurface = JSON.parse(JSON.stringify(generatedRealm.__zephiumWebExtensionSurface));
// Every suite uses the shipping metadata by default. Fixture mode checks the
// same behavior against the generator's small, reviewable member selection.
export const surface = process.env.ZEPHIUM_WEBEXT_TEST_SURFACE === 'fixture' ? fixtureSurface : generatedSurface;

export const EXTENSION_ID = 'abcdefghijklmnopabcdefghijklmnop';

export const modulesSource = () =>
  'const Z={__proto__:null};' +
  MODULES.map(([key, file, argument]) => {
    const source = sources[key === 'I' ? 'intrinsics' : key];
    return source === null ? '' : `Z.${key}=(${source})(${argument});`;
  }).join('');

export function createRealm(extra = {}) {
  const logs = { error: [], warn: [] };
  const fakeConsole = {
    error: (...args) => logs.error.push(args.map(String).join(' ')),
    warn: (...args) => logs.warn.push(args.map(String).join(' ')),
    log: () => {},
  };
  const sandbox = {
    console: fakeConsole,
    setTimeout,
    clearTimeout,
    queueMicrotask,
    TextEncoder,
    TextDecoder,
    URL,
    Event,
    EventTarget,
    MessageChannel,
    MessagePort,
    MessageEvent,
    DOMException,
    performance,
    ...extra,
  };
  const context = vm.createContext(sandbox);
  const Z = vm.runInContext(`(() => { 'use strict'; ${modulesSource()} return Z; })()`, context);
  return { context, Z, logs, run: (code) => vm.runInContext(code, context) };
}

export const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
export const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

// The host side of one context: decodes what the context sends, answers
// calls through `handlers[name](args, call)` (return a value, throw
// `{error}` to fail, or return `host.LATER` to answer by hand).
export class FakeHost {
  constructor(Z, kind = 'background') {
    this.Z = Z;
    this.T = Z.transport;
    this.max = this.T.messageLimit(kind);
    this.frames = [];
    this.calls = [];
    this.acks = [];
    this.portMessages = [];
    this.disconnects = [];
    this.handlers = {};
    this.LATER = Symbol('later');
    this.accept = true;
    this.nextSeq = 1n;
    this.nextPort = 100n;
    this.link = {
      send: (bytes) => this.receive(bytes),
      listen: (receiver) => {
        this.deliver = receiver;
      },
    };
    this.handlers['zephium.addListener'] = () => null;
    this.handlers['zephium.removeListener'] = () => null;
    this.handlers['zephium.respond'] = () => null;
    this.handlers['zephium.eventResult'] = () => null;
    this.handlers['zephium.workerLifecycle'] = () => null;
  }

  receive(bytes) {
    if (!this.accept) return false;
    const decoded = this.T.decode(bytes, this.T.FROM_CONTEXT, this.max);
    if (decoded.error) throw new Error(`context sent a bad frame: ${decoded.error}`);
    const frame = decoded.frame;
    this.frames.push(frame);
    const text = (value) => new TextDecoder().decode(value);
    switch (frame.kind) {
      case 'call': {
        const call = { id: frame.id, name: frame.name, args: JSON.parse(text(frame.args)), bytes: bytes.byteLength };
        this.calls.push(call);
        const handler = this.handlers[frame.name];
        if (!handler) {
          this.fail(call.id, 'Access to extension API denied.');
          break;
        }
        let result;
        try {
          result = handler(call.args, call);
        } catch (thrown) {
          this.fail(call.id, thrown && thrown.error ? thrown.error : String(thrown));
          break;
        }
        if (result !== this.LATER) this.reply(call.id, result === undefined ? null : result);
        break;
      }
      case 'ack':
        this.acks.push(frame.seq);
        break;
      case 'portMessage':
        this.portMessages.push({ port: frame.port, message: JSON.parse(text(frame.payload)) });
        break;
      case 'portDisconnect':
        this.disconnects.push(frame.port);
        break;
    }
    return true;
  }

  send(frame) {
    const bytes = this.T.encode(frame, this.T.TO_CONTEXT, this.max);
    if (typeof bytes === 'string') throw new Error(`host could not encode: ${bytes}`);
    this.deliver(bytes);
  }

  reply(id, value) {
    this.send({ kind: 'reply', id, ok: true, value: new TextEncoder().encode(JSON.stringify(value)) });
  }

  fail(id, error) {
    this.send({ kind: 'reply', id, ok: false, error });
  }

  event(name, args, seq = this.nextSeq++) {
    this.send({ kind: 'event', seq, name, args: new TextEncoder().encode(JSON.stringify(args)) });
    return seq;
  }

  portMessage(port, message) {
    this.send({ kind: 'portMessage', port, payload: new TextEncoder().encode(JSON.stringify(message)) });
  }

  portDisconnect(port, error = null) {
    this.send({ kind: 'portDisconnect', port, error });
  }

  callsTo(name) {
    return this.calls.filter((call) => call.name === name);
  }
}

export const bootstrap = (overrides = {}) => ({
  extensionId: EXTENSION_ID,
  scheme: 'chrome-extension',
  contextKind: 'background',
  incognito: false,
  uiLanguage: 'en-US',
  manifest: { manifest_version: 3, name: 'Fixture', version: '1.0', permissions: ['storage', 'alarms', 'scripting'] },
  messages: {
    greeting: 'Hello $1, you have $2 messages',
    dollars: 'Cost: $$5 and $$$$ and $x end$',
    markup: 'a <b> c',
  },
  permissions: ['storage', 'alarms', 'scripting', 'tabs'],
  ...overrides,
});

// A realm with `chrome` built for `kind`, answered by a fake host.
export function extensionContext(kind = 'background', overrides = {}, options = {}) {
  const realm = createRealm(options.globals);
  const host = new FakeHost(realm.Z, kind);
  const context = realm.Z.api.create(surface, bootstrap({ contextKind: kind, ...overrides }), host.link, {
    ...(options.create || {}),
  });
  return { ...realm, host, context, chrome: context.chrome };
}
