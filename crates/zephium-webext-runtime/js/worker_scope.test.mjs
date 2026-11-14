import assert from 'node:assert/strict';
import { test } from 'node:test';

import { EXTENSION_ID, FakeHost, bootstrap, createRealm, flush, plain, surface } from './test/harness.mjs';

const SCRIPT_URL = `chrome-extension://${EXTENSION_ID}/background.js`;

// A dedicated worker's global as WebKit shapes it: WorkerGlobalScope and
// DedicatedWorkerGlobalScope prototypes, no window or document.
function workerRealm(workerConfig = {}, options = {}) {
  const realm = createRealm();
  const self = realm.run(`(() => {
    class WorkerGlobalScope extends EventTarget {}
    class DedicatedWorkerGlobalScope extends WorkerGlobalScope {
      postMessage() {}
      close() {}
      get name() { return ''; }
    }
    class WorkerLocation {}
    class WorkerNavigator {}
    const self = new DedicatedWorkerGlobalScope();
    Object.assign(self, { WorkerGlobalScope, DedicatedWorkerGlobalScope, WorkerLocation, navigator: new WorkerNavigator(), imported: [] });
    self.importScripts = (...urls) => self.imported.push(...urls);
    self.self = self;
    return self;
  })()`);
  const host = new FakeHost(realm.Z, 'background');
  const context = realm.Z.workerScope.worker(
    surface,
    bootstrap({
      contextKind: 'background',
      worker: { type: 'classic', scriptURL: SCRIPT_URL, install: true, ...workerConfig },
    }),
    { global: self, link: options.withoutLink ? undefined : host.link },
  );
  // Evaluates `source` as worker code: globals resolve on the fake `self`.
  const inWorker = (source) => realm.run(`(function (self) { with (self) { return (${source}); } })`)(self);
  return { ...realm, self, host, context, chrome: self.chrome, inWorker };
}

const lifecycle = (host) => host.callsTo('zephium.workerLifecycle').map((call) => call.args);

test('the worker passes the service-worker identity checks real extensions use', () => {
  const { inWorker, self } = workerRealm();
  assert.equal(
    inWorker('typeof ServiceWorkerGlobalScope !== "undefined" && self instanceof ServiceWorkerGlobalScope'),
    true,
  );
  assert.equal(inWorker('Object.prototype.toString.call(self)'), '[object ServiceWorkerGlobalScope]');
  assert.equal(inWorker('typeof DedicatedWorkerGlobalScope'), 'undefined');
  assert.equal(inWorker('typeof self.postMessage'), 'undefined');
  assert.equal(inWorker('typeof self.close'), 'undefined');
  assert.equal(inWorker('"name" in self'), false);
  assert.equal(inWorker('self instanceof WorkerGlobalScope'), true);
  assert.equal(inWorker('typeof importScripts === "function"'), true);
  assert.equal(inWorker('self.registration == null'), true, 'Bitwarden takes its unsupported Web Push path');
  assert.equal(inWorker('typeof self.skipWaiting'), 'function');
  assert.equal(inWorker('self.clients instanceof Clients'), true);
  assert.equal(inWorker('typeof chrome.runtime.id === "string" && browser === chrome'), true);
  assert.equal(inWorker('({}) instanceof ServiceWorkerGlobalScope'), false);
  assert.throws(() => inWorker('new ServiceWorkerGlobalScope()'), /Illegal constructor/);
  assert.equal(self.location.href, SCRIPT_URL);
  assert.equal(`${self.location}`, SCRIPT_URL);
  assert.equal(self.location.pathname, '/background.js');
  assert.equal(inWorker('location instanceof WorkerLocation'), true);
});

test('install then activate run once the script finished, with waitUntil and handler attributes', async () => {
  const { inWorker, host, self } = workerRealm();
  const order = [];
  self.order = order;
  inWorker(`(() => {
    self.oninstall = (event) => {
      importScripts('chunk-1.js', 'chunk-2.js');
      self.order.push('oninstall:' + (event instanceof ExtendableEvent));
      event.waitUntil(new Promise((resolve) => setTimeout(() => { self.order.push('install settled'); resolve(); }, 5)));
      self.skipWaiting();
    };
    self.addEventListener('install', () => self.order.push('install listener'));
    self.addEventListener('activate', (event) => { self.order.push('activate'); event.waitUntil(clients.claim()); });
    self.addEventListener('fetch', () => self.order.push('fetch'));
    self.addEventListener('message', () => self.order.push('message'));
    self.order.push('script done');
  })()`);
  assert.deepEqual(order, ['script done']);
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(order, ['script done', 'oninstall:true', 'install listener', 'install settled', 'activate']);
  assert.deepEqual(plain(self.imported), ['chunk-1.js', 'chunk-2.js']);
  assert.deepEqual(lifecycle(host), [['activated', null]]);
  assert.equal(typeof self.onfetch, 'object');
});

test('events wait for activation; replies to calls do not', async () => {
  const { chrome, host, inWorker } = workerRealm();
  const heard = [];
  chrome.runtime.onInstalled.addListener((details) => heard.push(details.reason));
  host.handlers['runtime.getPlatformInfo'] = () => ({ os: 'linux' });
  inWorker('self.addEventListener("install", (e) => e.waitUntil(new Promise((r) => setTimeout(r, 10))))');
  host.event('runtime.onInstalled', [{ reason: 'install' }]);
  assert.deepEqual(plain(await chrome.runtime.getPlatformInfo()), { os: 'linux' });
  assert.deepEqual(heard, []);
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.deepEqual(heard, ['install']);
});

test('waitUntil outside dispatch throws InvalidStateError', async () => {
  const { inWorker, self } = workerRealm();
  inWorker('self.addEventListener("install", (e) => { self.saved = e; })');
  await flush();
  await flush();
  assert.throws(
    () => self.saved.waitUntil(Promise.resolve()),
    (error) => error.name === 'InvalidStateError',
  );
});

test('a later start of an installed worker fires no install event', async () => {
  const { inWorker, host, self } = workerRealm({ install: false });
  self.fired = [];
  inWorker('self.addEventListener("install", () => self.fired.push("install"))');
  await flush();
  assert.deepEqual(plain(self.fired), []);
  assert.deepEqual(lifecycle(host), [['activated', null]]);
});

test('a failing install or a startup error is reported to the host', async () => {
  const failing = workerRealm();
  failing.inWorker('self.addEventListener("install", (e) => e.waitUntil(Promise.reject(new Error("no"))))');
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(lifecycle(failing.host), [['failed', 'install']]);

  const broken = workerRealm();
  broken.self.dispatchEvent(Object.assign(new Event('error'), { message: 'Uncaught SyntaxError' }));
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(lifecycle(broken.host), [['failed', 'Uncaught SyntaxError']]);
});

test('activation failure retains queued events and reports failure', async () => {
  const { chrome, host, inWorker } = workerRealm();
  let called = 0;
  chrome.runtime.onInstalled.addListener(() => called++);
  host.event('runtime.onInstalled', [{ reason: 'install' }]);
  inWorker('self.addEventListener("activate", (event) => event.waitUntil(Promise.reject(new Error("no"))))');
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(lifecycle(host), [['failed', 'activate']]);
  assert.equal(called, 0);
  assert.deepEqual(host.acks, []);
});

test('event release waits for host acceptance of worker activation', async () => {
  const { chrome, host } = workerRealm({ install: false });
  host.handlers['zephium.workerLifecycle'] = () => host.LATER;
  let called = 0;
  chrome.runtime.onStartup.addListener(() => called++);
  host.event('runtime.onStartup', []);
  await flush();
  assert.equal(called, 0);
  const call = host.callsTo('zephium.workerLifecycle')[0];
  host.reply(call.id, null);
  await flush();
  assert.equal(called, 1);
});

test('clients.openWindow without tabs authority rejects with InvalidAccessError', async () => {
  const { chrome, inWorker } = workerRealm();
  delete chrome.tabs.create;
  await assert.rejects(inWorker('clients.openWindow("https://example.com/")'), {
    name: 'InvalidAccessError',
    message: 'Not allowed to open a window.',
  });
});

test("module workers refuse importScripts with Chrome's message", () => {
  const { inWorker } = workerRealm({ type: 'module' });
  assert.throws(() => inWorker('importScripts("x.js")'), {
    name: 'TypeError',
    message: "Failed to execute 'importScripts' on 'WorkerGlobalScope': Module scripts don't support importScripts().",
  });
});

test("navigator.storage is shimmed with the host's figures; clients are inert", async () => {
  const { inWorker } = workerRealm({ storage: { quota: 1073741824, usage: 4096, persisted: true } });
  assert.deepEqual(plain(await inWorker('navigator.storage.estimate()')), {
    quota: 1073741824,
    usage: 4096,
    usageDetails: {},
  });
  assert.equal(await inWorker('navigator.storage.persist()'), true);
  assert.equal(await inWorker('navigator.storage.persisted()'), true);
  assert.deepEqual(plain(await inWorker('clients.matchAll({ includeUncontrolled: true })')), []);
  assert.equal(await inWorker('clients.get("x")'), undefined);
  assert.equal(await inWorker('self.skipWaiting()'), undefined);
});

test('the worker takes its port from the owner and talks through it', async () => {
  const { self, Z, chrome } = workerRealm({}, { withoutLink: true });
  const { port1, port2 } = new MessageChannel();
  let extensionSaw = false;
  self.addEventListener('message', () => (extensionSaw = true));
  const pending = chrome.tabs.query({});
  self.dispatchEvent(new MessageEvent('message', { data: Z.workerScope.PORT_MARKER, ports: [port2] }));
  assert.equal(extensionSaw, false);
  const host = new FakeHost(Z, 'background');
  port1.onmessage = (event) => host.receive(new Uint8Array(event.data));
  host.deliver = (bytes) => port1.postMessage(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length));
  host.handlers['tabs.query'] = () => [{ id: 1 }];
  assert.deepEqual(plain(await pending), [{ id: 1 }]);
  self.dispatchEvent(new MessageEvent('message', { data: 'from elsewhere' }));
  assert.equal(extensionSaw, true);
  port1.close();
});

test('the owner relays frames both ways, keeps its own replies and reports worker errors', async () => {
  const realm = createRealm();
  const { Z } = realm;
  const host = new FakeHost(Z, 'background');
  host.handlers['zephium.workerError'] = () => null;
  const worker = new EventTarget();
  const posted = [];
  worker.postMessage = (data, transfer) => posted.push({ data, transfer });
  const channel = new MessageChannel();
  const started = [];
  const owner = Z.workerScope.owner(
    { workerURL: `${SCRIPT_URL}?zephium-worker`, type: 'module' },
    {
      link: host.link,
      createWorker: (url, type) => (started.push([url, type]), worker),
      createChannel: () => channel,
    },
  );
  assert.ok(owner);
  assert.deepEqual(started, [[`${SCRIPT_URL}?zephium-worker`, 'module']]);
  assert.equal(posted[0].data, Z.workerScope.PORT_MARKER);
  const workerPort = posted[0].transfer[0];
  const toWorker = [];
  workerPort.onmessage = (event) => toWorker.push(new Uint8Array(event.data));

  const T = Z.transport;
  const call = T.encode(
    { kind: 'call', id: 4, name: 'tabs.query', args: new TextEncoder().encode('[{}]') },
    T.FROM_CONTEXT,
    1 << 20,
  );
  workerPort.postMessage(call.buffer);
  const forged = T.encode(
    { kind: 'call', id: 0x80000001, name: 'tabs.query', args: new TextEncoder().encode('[]') },
    T.FROM_CONTEXT,
    1 << 20,
  );
  workerPort.postMessage(forged.buffer);
  workerPort.postMessage('not bytes');
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(
    host.calls.map((c) => [c.id, c.name]),
    [[4, 'tabs.query']],
  );
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(toWorker.length, 1, 'the refusal of call 4 reached the worker');
  assert.equal(T.decode(toWorker[0], T.TO_CONTEXT, 1 << 20).frame.id, 4);

  worker.dispatchEvent(
    Object.assign(new Event('error'), {
      message: 'Uncaught ReferenceError: x is not defined',
      filename: SCRIPT_URL,
      lineno: 3,
      colno: 9,
    }),
  );
  await flush();
  const report = host.callsTo('zephium.workerError');
  assert.equal(report.length, 1);
  assert.ok(report[0].id >= 0x80000000);
  assert.deepEqual(report[0].args, [
    { message: 'Uncaught ReferenceError: x is not defined', filename: SCRIPT_URL, lineno: 3, colno: 9 },
  ]);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(toWorker.length, 1, "the owner's own reply stays with the owner");
  channel.port1.close();
  workerPort.close();
});

test('worker diagnostics obey host UTF8 byte limits, including startup failures', async () => {
  const startup = workerRealm();
  startup.self.dispatchEvent(Object.assign(new Event('error'), { message: '🌍'.repeat(2000) }));
  await flush();
  assert.deepEqual(lifecycle(startup.host), [['failed', '🌍'.repeat(256)]]);

  const realm = createRealm();
  const host = new FakeHost(realm.Z, 'background');
  host.handlers['zephium.workerError'] = ([detail]) => {
    assert.ok(Buffer.byteLength(detail.message) <= 1024);
    assert.ok(Buffer.byteLength(detail.filename) <= 2048);
    assert.equal(detail.lineno, 0);
    assert.equal(detail.colno, 0);
    return null;
  };
  const channel = new MessageChannel();
  const worker = new EventTarget();
  worker.postMessage = () => {};
  const owner = realm.Z.workerScope.owner({ workerURL: SCRIPT_URL }, {
    link: host.link,
    createWorker: () => worker,
    createChannel: () => channel,
  });
  owner.reportError({ message: '漢'.repeat(2048), filename: '🌍'.repeat(2048), lineno: -1, colno: 0x100000000 });
  const [detail] = host.callsTo('zephium.workerError')[0].args;
  assert.equal(detail.message, '漢'.repeat(341));
  assert.equal(detail.filename, '🌍'.repeat(512));
  owner.reportError({ message: String.fromCharCode(0xd800) });
  assert.equal(host.callsTo('zephium.workerError')[1].args[0].message, '�');
  channel.port1.close();
  channel.port2.close();
});
