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

  async context(label, kind = 'background') {
    await this.command('context', { context: label, kind });
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
      bootstrap({ contextKind: kind, permissions: ['storage', 'alarms', 'nativeMessaging'] }),
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
  assert.deepEqual(plain(await background.chrome.tabs.sendMessage(5, 'reverse')), { content: 'reverse' });
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
