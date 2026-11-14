import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { test } from 'node:test';

import { bootstrap, createRealm, flush, plain } from './test/harness.mjs';

const bundle = readFileSync(join(process.env.ZEPHIUM_WEBEXT_BUNDLE_DIR, 'bundle.js'), 'utf8');
const surface = readFileSync(new URL('./surface.generated.js', import.meta.url), 'utf8');
const encoder = new TextEncoder();

// Control records identify adapter-owned contexts; only v1 envelope bytes
// cross each context's native bridge. API results are never mocked here.
class RustPeer {
  constructor(t) {
    this.next = 0;
    this.waiting = new Map();
    this.contexts = new Map();
    this.errors = [];
    this.process = spawn(process.env.ZEPHIUM_WEBEXT_E2E_HOST, ['--ignored', '--exact', 'stdio_host', '--nocapture'], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    this.stderr = '';
    this.process.stderr.on('data', (chunk) => (this.stderr += chunk));
    this.process.on('error', (error) => this.fail(error));
    this.process.on('exit', (code) => {
      if (code !== 0 && code !== null) this.fail(new Error(`Rust peer exited ${code}: ${this.stderr}`));
    });
    const lines = createInterface({ input: this.process.stdout });
    lines.on('line', (line) => {
      const prefix = 'ZEPHIUM_E2E:';
      const start = line.indexOf(prefix);
      if (start === -1) return;
      const record = JSON.parse(line.slice(start + prefix.length));
      if (record.frame) {
        const context = this.contexts.get(record.context);
        context.receiver(new context.realmBytes(record.frame));
      } else {
        const pending = this.waiting.get(record.id);
        assert.ok(pending, `unknown control reply ${record.id}`);
        this.waiting.delete(record.id);
        pending.resolve(record.result);
      }
    });
    t.after(async () => {
      this.process.stdin.end();
      if (this.process.exitCode === null && this.process.signalCode === null) {
        await new Promise((resolve) => {
          const timer = setTimeout(() => this.process.kill(), 2_000);
          this.process.once('exit', () => {
            clearTimeout(timer);
            resolve();
          });
        });
      }
      lines.close();
      assert.deepEqual(this.errors, []);
    });
  }

  fail(error) {
    this.errors.push(error.message);
    for (const pending of this.waiting.values()) pending.reject(error);
    this.waiting.clear();
  }

  command(op, data = {}) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      this.process.stdin.write(JSON.stringify({ id, op, ...data }) + '\n');
    });
  }

  async idle() {
    for (let round = 0; round < 3 || this.waiting.size > 0; round++) {
      assert.ok(round < 100, 'bridge did not become idle');
      await this.command('stats');
      await flush();
    }
  }

  async context(label, kind = 'background', frameId = 0) {
    await this.command('context', { context: label, kind, frameId });
    const realm = createRealm();
    const context = { ...realm, label, dropAcks: false, frames: [], rawReplies: new Map(), rawId: 0x70000000 };
    context.realmBytes = realm.run('Uint8Array');
    const transport = realm.Z.transport;
    const global = realm.run('globalThis');
    global.__zephiumPost = (bytes) => {
      const frame = transport.decode(bytes, transport.FROM_CONTEXT, transport.messageLimit(kind)).frame;
      context.frames.push(frame);
      if (context.dropAcks && frame.kind === 'ack') return true;
      this.command('frame', { context: label, frame: Array.from(bytes) }).catch((error) => this.fail(error));
      return true;
    };
    global.__zephiumRegisterReceiver = (receiver) => {
      context.receiver = (bytes) => {
        const frame = transport.decode(bytes, transport.TO_CONTEXT, transport.messageLimit(kind)).frame;
        const raw = frame.kind === 'reply' ? context.rawReplies.get(frame.id) : undefined;
        if (raw) {
          context.rawReplies.delete(frame.id);
          if (frame.ok) raw.resolve(JSON.parse(new TextDecoder().decode(frame.value)));
          else raw.reject(new Error(frame.error));
        } else receiver(bytes);
      };
      return true;
    };
    this.contexts.set(label, context);
    realm.run(surface);
    context.chrome = realm.run(bundle)(
      'page',
      global.__zephiumWebExtensionSurface,
      bootstrap({ contextKind: kind, manifest: { manifest_version: 3, name: 'Fixture', version: '1', action: {}, commands: {} }, permissions: ['storage', 'alarms', 'nativeMessaging', 'tabs', 'activeTab', 'scripting', 'webNavigation', 'history', 'bookmarks', 'sessions', 'topSites', 'search', 'notifications', 'idle', 'fontSettings'] }),
    );
    context.rawCall = (name, args) => {
      const id = ++context.rawId;
      return new Promise((resolve, reject) => {
        context.rawReplies.set(id, { resolve, reject });
        const bytes = transport.encode({ kind: 'call', id, name, args: encoder.encode(JSON.stringify(args)) }, transport.FROM_CONTEXT, transport.messageLimit(kind));
        this.command('frame', { context: label, frame: Array.from(bytes) }).catch(reject);
      });
    };
    context.rawAck = (seq) => this.command('frame', { context: label, frame: Array.from(transport.encode({ kind: 'ack', seq }, transport.FROM_CONTEXT, transport.messageLimit(kind))) });
    return context;
  }
}

test('real storage callbacks, promises, changes and lastError scope', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const background = await peer.context('background');
  const content = await peer.context('content', 'contentScript');
  const changes = [];
  content.chrome.storage.onChanged.addListener((change, area) => changes.push([plain(change), area]));
  await peer.idle();
  assert.equal(await background.chrome.storage.local.set({ answer: 42 }), undefined);
  assert.deepEqual(plain(await content.chrome.storage.local.get('answer')), { answer: 42 });
  await new Promise((resolve) => background.chrome.storage.local.set({ callback: true }, resolve));
  const callbackValue = await new Promise((resolve) => content.chrome.storage.local.get(['answer', 'callback'], resolve));
  assert.deepEqual(plain(callbackValue), { answer: 42, callback: true });
  await peer.idle();
  assert.deepEqual(changes, [[{ answer: { newValue: 42 } }, 'local'], [{ callback: { newValue: true } }, 'local']]);
  let error;
  await new Promise((resolve) => background.chrome.storage.managed.set({ refused: true }, () => {
    error = background.chrome.runtime.lastError.message;
    resolve();
  }));
  assert.equal(error, 'This is a read-only store.');
  assert.equal(background.chrome.runtime.lastError, undefined);
  await assert.rejects(background.chrome.storage.managed.set({ refused: true }), { message: error });
  assert.deepEqual(plain(await background.chrome.storage.managed.get(null)), {});
  assert.equal((await peer.command('stats')).bytes, 0);
});

test('real alarm service fires only after the fake clock advances', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const background = await peer.context('background');
  const alarms = [];
  background.chrome.alarms.onAlarm.addListener((alarm) => alarms.push(plain(alarm)));
  await peer.idle();
  await background.chrome.alarms.create('wake', { delayInMinutes: 0.5 });
  assert.equal((await background.chrome.alarms.get('wake')).name, 'wake');
  await peer.command('advance', { millis: 29_999 });
  assert.equal(alarms.length, 0);
  await peer.command('advance', { millis: 1 });
  await peer.idle();
  assert.equal(alarms.length, 1);
  assert.equal(alarms[0].name, 'wake');
  assert.equal(alarms[0].scheduledTime, 1_030_000);
  assert.equal(await background.chrome.alarms.get('wake'), undefined);
});

test('real messaging service handles sync, callback, promise and refusal responses', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const background = await peer.context('background');
  const content = await peer.context('content', 'contentScript');
  background.chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    assert.equal(sender.id, bootstrap().extensionId);
    if (message.mode === 'sync') sendResponse({ style: 'sync', value: message.value });
    if (message.mode === 'callback') {
      setTimeout(() => sendResponse({ style: 'callback' }), 0);
      return true;
    }
    if (message.mode === 'promise') return Promise.resolve({ style: 'promise' });
    if (message.mode === 'reject') return Promise.reject(new Error('fixture rejected'));
    if (message.mode === 'pending') return true;
    return undefined;
  });
  content.chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => sendResponse({ content: message }));
  await peer.idle();
  assert.deepEqual(plain(await content.chrome.runtime.sendMessage({ mode: 'sync', value: 7 })), { style: 'sync', value: 7 });
  assert.deepEqual(plain(await content.chrome.runtime.sendMessage({ mode: 'callback' })), { style: 'callback' });
  assert.deepEqual(plain(await content.chrome.runtime.sendMessage({ mode: 'promise' })), { style: 'promise' });
  assert.deepEqual(plain(await background.chrome.tabs.sendMessage(1, 'reverse')), { content: 'reverse' });
  await assert.rejects(background.chrome.runtime.sendMessage('content is not a runtime target'), {
    message: 'Could not establish connection. Receiving end does not exist.',
  });
  await assert.rejects(content.chrome.runtime.sendMessage({ mode: 'reject' }), { message: 'fixture rejected' });
  assert.equal(await content.chrome.runtime.sendMessage({ mode: 'none' }), undefined);
  let callbackError;
  await new Promise((resolve) => content.chrome.runtime.sendMessage({ mode: 'none' }, (response) => {
    assert.equal(response, undefined);
    callbackError = content.chrome.runtime.lastError.message;
    resolve();
  }));
  assert.equal(callbackError, 'The message port closed before a response was received.');
  assert.equal(content.chrome.runtime.lastError, undefined);
  const pending = content.chrome.runtime.sendMessage({ mode: 'pending' });
  const rejected = assert.rejects(pending, /A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received/);
  await peer.idle();
  await peer.command('cancel', { context: 'background' });
  await rejected;
  await peer.idle();
  assert.equal((await peer.command('stats')).bytes, 0);
});

test('real ports relay opening messages and disconnect after context cancellation', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const background = await peer.context('background');
  const content = await peer.context('content', 'contentScript');
  const received = [];
  background.chrome.runtime.onConnect.addListener((port) => {
    assert.equal(port.name, 'fixture');
    port.onMessage.addListener((message) => port.postMessage({ echo: message }));
  });
  await peer.idle();
  const port = content.chrome.runtime.connect({ name: 'fixture' });
  port.onMessage.addListener((message) => received.push(plain(message)));
  const disconnected = new Promise((resolve) => port.onDisconnect.addListener(resolve));
  port.postMessage({ beforeHandshake: true });
  await peer.idle();
  assert.deepEqual(received, [{ echo: { beforeHandshake: true } }]);
  await peer.command('cancel', { context: 'background' });
  assert.equal(await disconnected, port);
  assert.throws(() => port.postMessage('late'), /Attempting to use a disconnected port object/);
  assert.equal((await peer.command('stats')).ports, 0);
  assert.equal((await peer.command('stats')).bytes, 0);
});

test('background interests survive restart and queued delivery requires its exact acknowledgement', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const first = await peer.context('background');
  const content = await peer.context('content', 'contentScript');
  let firstDeliveries = 0;
  first.chrome.alarms.onAlarm.addListener(() => firstDeliveries++);
  await peer.idle();
  await first.rawCall('zephium.workerLifecycle', ['activated', null]);
  first.dropAcks = true;
  await peer.command('queue');
  await peer.command('replay');
  await peer.command('replay');
  assert.equal(firstDeliveries, 1);
  assert.equal((await content.rawAck(1n)).queued, 1);
  await peer.command('cancel', { context: 'background' });
  await peer.command('restore');
  // Saved listener interest still authorizes waking after live authority dies.
  await peer.command('queue');
  const second = await peer.context('background');
  const replayed = [];
  second.chrome.alarms.onAlarm.addListener((alarm) => replayed.push(alarm.name));
  await peer.idle();
  await second.rawCall('zephium.workerLifecycle', ['activated', null]);
  await peer.command('replay');
  await peer.idle();
  assert.deepEqual(replayed, ['replayed']);
  assert.equal((await peer.command('stats')).queued, 1);
  await peer.command('replay');
  await peer.idle();
  assert.deepEqual(replayed, ['replayed', 'replayed']);
  const stats = await peer.command('stats');
  assert.equal(stats.queued, 0);
  assert.equal(stats.acknowledged, 2);
});

test('generated argument errors and Rust context policy both refuse invalid calls', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const background = await peer.context('background');
  const content = await peer.context('content', 'contentScript');
  const expected = 'Error in invocation of storage.set(object items, optional function callback): No matching signature.';
  assert.throws(() => background.chrome.storage.local.set(), { message: expected });
  await assert.rejects(background.rawCall('storage.local.set', []), { message: expected });
  assert.deepEqual(plain(await background.chrome.runtime.sendNativeMessage('fixture.host', { hello: 'native' })), {
    hello: 'native',
  });
  await assert.rejects(content.rawCall('runtime.connectNative', ['fixture.host']), /not available in this context/);
  await assert.rejects(content.rawCall('zephium.workerLifecycle', ['activated', null]), /not available in this context/);
  assert.deepEqual(plain(await background.chrome.storage.local.get(null)), {});
  assert.equal((await peer.command('stats')).bytes, 0);
});


function callback(chrome, invoke) {
  return new Promise((resolve, reject) => invoke((value) => {
    const error = chrome.runtime.lastError;
    if (error) reject(new Error(error.message));
    else resolve(value);
  }));
}

for (const metadata of [true, false]) test(`browser tab mutations preserve event order with metadata=${metadata}`, { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const { chrome } = await peer.context('background');
  const tabs = plain(await chrome.tabs.query({}));
  assert.deepEqual(tabs.map((tab) => tab.url), ['https://example.com/', 'https://other.test/']);
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.tabs.query({}, done))), tabs);
  if (!metadata) {
    await peer.command('restrictPermissions');
    for (const tab of await chrome.tabs.query({})) {
      for (const field of ['url', 'pendingUrl', 'title', 'favIconUrl']) assert.equal(field in tab, false);
    }
    for (const window of await chrome.windows.getAll({ populate: true })) {
      for (const tab of window.tabs) assert.equal('url' in tab, false);
    }
  }
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.tabs.query({}, done))), plain(await chrome.tabs.query({})));
  const events = [];
  for (const name of ['onCreated', 'onActivated', 'onUpdated', 'onRemoved']) {
    chrome.tabs[name].addListener((...args) => events.push([name, plain(args)]));
  }
  await peer.idle();
  const created = await chrome.tabs.create({ url: 'https://example.com/new', active: true });
  await chrome.tabs.update(created.id, { url: 'https://example.com/next' });
  await callback(chrome, (done) => chrome.tabs.remove(created.id, done));
  await peer.idle();
  assert.deepEqual(events.map(([name]) => name), ['onCreated', 'onActivated', 'onUpdated', ...(metadata ? ['onUpdated'] : []), 'onUpdated', 'onRemoved', 'onActivated']);
  assert.equal(events[0][1][0].id, created.id);
  assert.deepEqual(events.filter(([name]) => name === 'onUpdated').map(([, args]) => args[1].status), metadata ? ['loading', undefined, 'complete'] : ['loading', 'complete']);
  const removed = events.find(([name]) => name === 'onRemoved')[1];
  assert.equal(removed[0], created.id);
  assert.equal(removed[1].isWindowClosing, false);
  if (!metadata) {
    assert.equal('url' in events[0][1][0], false);
    for (const [, args] of events.filter(([name]) => name === 'onUpdated')) {
      assert.equal('url' in args[1], false);
      assert.equal('url' in args[2], false);
    }
  }
  assert.deepEqual(plain(await chrome.tabs.query({})).map((tab) => tab.id), [1, 2]);
  await assert.rejects(chrome.tabs.get(created.id), /No tab with id/);
});

test('tabs messages honor frame and document selectors through the browser port', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const { chrome } = await peer.context('background');
  const main = await peer.context('main', 'contentScript');
  const child = await peer.context('child', 'contentScript', 3);
  const received = [];
  for (const [context, frame] of [[main, 0], [child, 3]]) {
    context.chrome.runtime.onMessage.addListener((message, _sender, respond) => {
      received.push(frame);
      respond({ frame, message });
    });
  }
  await peer.idle();
  assert.deepEqual(plain(await chrome.tabs.sendMessage(1, 'child', { frameId: 3 })), { frame: 3, message: 'child' });
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.tabs.sendMessage(1, 'main', { frameId: 0 }, done))), { frame: 0, message: 'main' });
  assert.deepEqual(received, [3, 0]);
  await assert.rejects(chrome.tabs.sendMessage(1, 'mismatch', { frameId: 0, documentId: 'child-document' }), /Receiving end does not exist/);
  await assert.rejects(chrome.tabs.sendMessage(1, 'missing', { frameId: 99 }), /Receiving end does not exist/);
  assert.deepEqual(received, [3, 0]);
});

test('windows populate tabs and action state grants only the invoked activeTab', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const { chrome } = await peer.context('background');
  const windows = plain(await chrome.windows.getAll({ populate: true }));
  assert.deepEqual(windows[0].tabs.map((tab) => tab.id), [1, 2]);
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.windows.getAll({ populate: true }, done))), windows);
  await chrome.action.setBadgeText({ text: 'global' });
  await callback(chrome, (done) => chrome.action.setBadgeText({ tabId: 1, text: 'one' }, done));
  assert.equal(await chrome.action.getBadgeText({ tabId: 1 }), 'one');
  assert.equal(await callback(chrome, (done) => chrome.action.getBadgeText({ tabId: 2 }, done)), 'global');
  await peer.command('restrictPermissions');
  assert.equal((await chrome.tabs.get(1)).url, undefined);
  await assert.rejects(chrome.scripting.executeScript({ target: { tabId: 1 }, files: ['script.js'] }), /Cannot access/);
  const clicks = [];
  chrome.action.onClicked.addListener((tab) => clicks.push(plain(tab)));
  await peer.idle();
  await peer.command('click', { tabId: 1 });
  assert.equal(clicks[0].url, 'https://example.com/');
  assert.equal((await chrome.tabs.get(1)).url, 'https://example.com/');
  assert.equal((await chrome.tabs.get(2)).url, undefined);
  assert.equal((await chrome.scripting.executeScript({ target: { tabId: 1 }, files: ['script.js'] }))[0].frameId, 0);
  await chrome.tabs.update(1, { url: 'https://other.test/next' });
  assert.equal(await chrome.action.getBadgeText({ tabId: 1 }), 'global');
  assert.equal((await chrome.tabs.get(1)).url, undefined);
});

test('permissions contains works in both modes and request requires a native gesture', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const { chrome } = await peer.context('background');
  assert.equal(await chrome.permissions.contains({ permissions: ['tabs'] }), true);
  assert.equal(await callback(chrome, (done) => chrome.permissions.contains({ permissions: ['downloads'] }, done)), false);
  const error = { message: 'This function must be called during a user gesture' };
  await assert.rejects(chrome.permissions.request({ permissions: ['downloads'] }), error);
  await assert.rejects(callback(chrome, (done) => chrome.permissions.request({ permissions: ['downloads'] }, done)), error);
  assert.equal(chrome.runtime.lastError, undefined);
  assert.equal(await chrome.permissions.contains({ permissions: ['downloads'] }), false);
});

test('scripting returns native frame result shapes and navigation applies URL filters', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const { chrome } = await peer.context('background');
  const injection = { target: { tabId: 1, allFrames: true }, func: (value) => value, args: [null] };
  const expected = [{ frameId: 0, documentId: 'document-1', result: { frame: 0, value: null } }, { frameId: 3, documentId: 'child-document', result: { frame: 3, value: null } }];
  assert.deepEqual(plain(await chrome.scripting.executeScript(injection)), expected);
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.scripting.executeScript(injection, done))), expected);
  await assert.rejects(chrome.scripting.executeScript({ target: { tabId: 2 }, files: ['script.js'] }), /Cannot access/);
  const frames = plain(await chrome.webNavigation.getAllFrames({ tabId: 1 }));
  assert.deepEqual(frames.map((frame) => frame.frameId), [0, 3]);
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.webNavigation.getAllFrames({ tabId: 1 }, done))), frames);
  const matching = [], excluded = [];
  chrome.webNavigation.onCommitted.addListener((event) => matching.push(plain(event)), { url: [{ hostEquals: 'example.com' }] });
  chrome.webNavigation.onCommitted.addListener((event) => excluded.push(plain(event)), { url: [{ hostEquals: 'other.test' }] });
  await peer.idle();
  await peer.command('navigation', { tabId: 1 });
  await peer.command('navigation', { tabId: 2 });
  await peer.idle();
  assert.equal(matching.length, 1);
  assert.equal(matching[0].url, 'https://example.com/');
  assert.equal(matching[0].documentId, 'document-1');
  assert.equal(matching[0].transitionType, 'link');
  assert.deepEqual(excluded, []);
});


test('remaining batch-one namespaces reach Rust in callback and promise modes', { timeout: 15_000 }, async (t) => {
  const peer = new RustPeer(t);
  const { chrome } = await peer.context('background');
  for (const [namespace, method, args, expected] of [
    ['commands', 'getAll', [], []],
    ['history', 'search', [{ text: '' }], []],
    ['bookmarks', 'getChildren', ['0'], []],
    ['sessions', 'getRecentlyClosed', [], []],
    ['topSites', 'get', [], []],
    ['notifications', 'getAll', [], {}],
    ['extension', 'isAllowedIncognitoAccess', [], false],
    ['idle', 'queryState', [15], 'active'],
    ['fontSettings', 'getFontList', [], [{ fontId: 'Fixture Mono', displayName: 'Fixture Mono' }]],
  ]) {
    assert.deepEqual(plain(await chrome[namespace][method](...args)), expected, `${namespace} promise`);
    assert.deepEqual(plain(await callback(chrome, (done) => chrome[namespace][method](...args, done))), expected, `${namespace} callback`);
  }
  const info = plain(await chrome.runtime.getPlatformInfo());
  assert.equal(typeof info.os, 'string');
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.runtime.getPlatformInfo(done))), info);
  const self = plain(await chrome.management.getSelf());
  assert.equal(self.id, bootstrap().extensionId);
  assert.equal(self.name, 'Fixture');
  assert.deepEqual(plain(await callback(chrome, (done) => chrome.management.getSelf(done))), self);
  assert.equal(await chrome.search.query({ text: 'promise', tabId: 1 }), undefined);
  assert.equal(await callback(chrome, (done) => chrome.search.query({ text: 'callback', tabId: 1 }, done)), undefined);
  assert.equal((await chrome.tabs.get(1)).url, 'https://example.com/search?q=callback');
  assert.equal((await chrome.history.search({ text: 'example.com' })).length, 2);
  // DOM identity stays in the realm: this namespace has only a synchronous helper.
  assert.equal(typeof chrome.dom.openOrClosedShadowRoot, 'function');
  assert.throws(() => chrome.dom.openOrClosedShadowRoot({}), /HTMLElement/);
  assert.equal((await peer.command('stats')).bytes, 0);
});
