import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { createRealm, FakeHost, flush } from './test/harness.mjs';

const fixtures = JSON.parse(readFileSync(new URL('./test/envelopes.json', import.meta.url), 'utf8'));
const fromHex = (hex) => Uint8Array.from(Buffer.from(hex, 'hex'));
const toHex = (bytes) => Buffer.from(bytes).toString('hex');
const text = (bytes) => new TextDecoder().decode(bytes);

function frameOf(fixture) {
  const frame = { ...fixture };
  for (const key of ['seq', 'port']) if (key in frame) frame[key] = BigInt(frame[key]);
  for (const key of ['args', 'value', 'payload'])
    if (typeof frame[key] === 'string') frame[key] = new TextEncoder().encode(frame[key]);
  if (frame.payloadHex) {
    frame.payload = fromHex(frame.payloadHex);
    delete frame.payloadHex;
  }
  return frame;
}

test('frames encode to the bytes the Rust encoder produces and decode back', () => {
  const { Z } = createRealm();
  const T = Z.transport;
  for (const fixture of fixtures.accepted) {
    const frame = frameOf(fixture.frame);
    const bytes = T.encode(frame, fixture.direction, fixtures.max);
    assert.equal(typeof bytes, 'object', fixture.name);
    assert.equal(toHex(bytes), fixture.hex, fixture.name);
    const decoded = T.decode(fromHex(fixture.hex), fixture.direction, fixtures.max);
    assert.equal(decoded.error, undefined, fixture.name);
    for (const [key, value] of Object.entries(frame)) {
      const actual = decoded.frame[key];
      if (value instanceof Uint8Array) assert.equal(toHex(actual), toHex(value), `${fixture.name}.${key}`);
      else assert.equal(actual, value, `${fixture.name}.${key}`);
    }
  }
});

test('the decoder refuses what the Rust decoder refuses, for the same reason', () => {
  const { Z } = createRealm();
  for (const fixture of fixtures.rejected) {
    const decoded = Z.transport.decode(fromHex(fixture.hex), fixture.direction, fixture.max ?? fixtures.max);
    assert.equal(decoded.error, fixture.error, fixture.name);
  }
});

test('the encoder refuses frames the Rust encoder refuses', () => {
  const { Z } = createRealm();
  const T = Z.transport;
  const args = new TextEncoder().encode('[]');
  assert.equal(T.encode({ kind: 'reply', id: 1, ok: true, value: args }, T.FROM_CONTEXT, 1 << 20), 'WrongDirection');
  assert.equal(T.encode({ kind: 'ack', seq: 1n }, T.TO_CONTEXT, 1 << 20), 'WrongDirection');
  assert.equal(T.encode({ kind: 'portDisconnect', port: 1n, error: 'x' }, T.FROM_CONTEXT, 1 << 20), 'WrongDirection');
  assert.equal(
    T.encode({ kind: 'reply', id: 1, ok: false, error: 'x'.repeat(16 * 1024 + 1) }, T.TO_CONTEXT, 1 << 20),
    'ErrorTooLong',
  );
  assert.equal(T.encode({ kind: 'call', id: 0, name: 'tabs.query', args }, T.FROM_CONTEXT, 1 << 20), 'ZeroId');
  assert.equal(T.encode({ kind: 'call', id: 2 ** 32, name: 'tabs.query', args }, T.FROM_CONTEXT, 1 << 20), 'ZeroId');
  assert.equal(T.encode({ kind: 'event', seq: 2n ** 64n, name: 'a.b', args }, T.TO_CONTEXT, 1 << 20), 'ZeroId');
  for (const name of ['tabs', 'tabs.', '.query', 'tabs..query', 'tabs.1q', 'tabs.qüery', `a.${'b'.repeat(127)}`]) {
    assert.equal(T.encode({ kind: 'call', id: 1, name, args }, T.FROM_CONTEXT, 1 << 20), 'InvalidName', name);
  }
  assert.equal(T.encode({ kind: 'call', id: 1, name: 'tabs.query', args }, T.FROM_CONTEXT, 26), 'TooLarge');
  assert.equal(T.isApiName('privacy.services.passwordSavingEnabled.set'), true);
});

test('call ids are unique among outstanding calls and stay in their range', async () => {
  const { Z } = createRealm();
  const host = new FakeHost(Z);
  host.handlers['test.call'] = () => host.LATER;
  const client = Z.transport.createClient(host.link, {
    maxMessageBytes: 1 << 20,
    idBase: 5,
    idLimit: 7,
    handlers: { event() {}, portMessage() {}, portDisconnect() {} },
  });
  const first = client.call('test.call', '[]');
  client.call('test.call', '[]');
  client.call('test.call', '[]');
  assert.deepEqual(
    host.calls.map((call) => call.id),
    [5, 6, 7],
  );
  host.reply(6, 'six');
  host.reply(5, 'five');
  assert.equal(await first, 'five');
  client.call('test.call', '[]');
  client.call('test.call', '[]');
  assert.deepEqual(
    host.calls.map((call) => call.id),
    [5, 6, 7, 5, 6],
  );
  await assert.rejects(
    client.call('test.call', '[]'),
    (error) => error === 'Too many extension API calls are pending.',
  );
});

test("oversize calls are refused before sending with Chrome's text", async () => {
  const { Z } = createRealm();
  const host = new FakeHost(Z, 'contentScript');
  const client = Z.transport.createClient(host.link, {
    maxMessageBytes: Z.transport.messageLimit('contentScript'),
    handlers: { event() {}, portMessage() {}, portDisconnect() {} },
  });
  const big = JSON.stringify(['x'.repeat(8 * 1024 * 1024)]);
  await assert.rejects(
    client.call('storage.local.set', big),
    (error) => error === 'Message length exceeded maximum allowed length.',
  );
  assert.equal(host.frames.length, 0);
  assert.equal(Z.transport.messageLimit('background'), 32 * 1024 * 1024);
  assert.equal(Z.transport.messageLimit('extensionFrame'), 8 * 1024 * 1024);
});

test('the outstanding-call limit refuses API calls but queues internal ones', async () => {
  const { Z } = createRealm();
  const host = new FakeHost(Z);
  host.handlers['test.slow'] = () => host.LATER;
  host.handlers['zephium.respond'] = () => 'done';
  const client = Z.transport.createClient(host.link, {
    maxMessageBytes: 1 << 20,
    handlers: { event() {}, portMessage() {}, portDisconnect() {} },
  });
  for (let i = 0; i < 256; i++) client.call('test.slow', '[]');
  await assert.rejects(
    client.call('test.slow', '[]'),
    (error) => error === 'Too many extension API calls are pending.',
  );
  const internal = client.call('zephium.respond', '["1",null]', true);
  await flush();
  assert.equal(host.callsTo('zephium.respond').length, 0);
  assert.equal(client.outstanding, 256);
  host.reply(host.calls[0].id, null);
  await flush();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(host.callsTo('zephium.respond').length, 1);
  assert.equal(await internal, 'done');
});

test('frames beyond the rate burst wait and leave in order', async () => {
  const { Z } = createRealm();
  const host = new FakeHost(Z);
  host.handlers['test.fast'] = () => null;
  const client = Z.transport.createClient(host.link, {
    maxMessageBytes: 1 << 20,
    handlers: { event() {}, portMessage() {}, portDisconnect() {} },
  });
  const done = [];
  for (let i = 0; i < 600; i++) done.push(client.ack(BigInt(i + 1)));
  assert.ok(done.every((failure) => failure === null));
  assert.ok(host.acks.length < 500, `burst sent ${host.acks.length}`);
  assert.ok(host.acks.length >= 400);
  while (host.acks.length < 600) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(
    host.acks,
    Array.from({ length: 600 }, (_, i) => BigInt(i + 1)),
  );
});

test('malformed or misdirected host frames are dropped', async () => {
  const { Z } = createRealm();
  const host = new FakeHost(Z);
  let events = 0;
  const client = Z.transport.createClient(host.link, {
    maxMessageBytes: 1 << 20,
    handlers: { event: () => events++, portMessage() {}, portDisconnect() {} },
  });
  host.deliver(fromHex(fixtures.accepted[0].hex));
  host.deliver(new Uint8Array([1, 2, 3]));
  host.deliver(fromHex('0c00000001030100000000000000'));
  host.event('runtime.onStartup', []);
  assert.equal(events, 1);
  assert.equal(client.dropped, 3);
  host.reply(99, 'nobody asked');
});

test('the native link is captured once, its globals removed and its receiver kept', () => {
  const { Z, run } = createRealm();
  const sent = [];
  let receiver = null;
  run('globalThis.__zephiumPost = null; globalThis.__zephiumRegisterReceiver = null;');
  const global = run('globalThis');
  global.__zephiumPost = (bytes) => (sent.push(bytes), true);
  global.__zephiumRegisterReceiver = (fn) => ((receiver = fn), true);
  const link = Z.transport.nativeLink(global);
  assert.equal(run('typeof __zephiumPost'), 'undefined');
  assert.equal(run('typeof __zephiumRegisterReceiver'), 'undefined');
  const received = [];
  assert.equal(
    link.listen((bytes) => received.push(bytes)),
    true,
  );
  receiver('not bytes');
  receiver(new Uint16Array(2));
  receiver(new Uint8Array([1]));
  assert.equal(received.length, 1);
  assert.equal(Object.getOwnPropertySymbols(global).length, 1);
  assert.equal(link.send(new Uint8Array([9])), true);
  assert.equal(sent.length, 1);
  assert.equal(Z.transport.nativeLink({}), null);
});

test('a refused native post fails the call instead of hanging', async () => {
  const { Z } = createRealm();
  const client = Z.transport.createClient(
    { send: () => false, listen() {} },
    { maxMessageBytes: 1 << 20, handlers: { event() {}, portMessage() {}, portDisconnect() {} } },
  );
  await assert.rejects(client.call('tabs.query', '[{}]'), (error) => error === 'Extension context invalidated.');
});

test('the port link buffers until attached and drops oversize or foreign data', async () => {
  const { Z } = createRealm();
  const link = Z.transport.portLink(16);
  const received = [];
  link.listen((bytes) => received.push(text(bytes)));
  assert.equal(link.send(new TextEncoder().encode('early')), true);
  const { port1, port2 } = new MessageChannel();
  const seen = [];
  port2.onmessage = (event) => seen.push(text(new Uint8Array(event.data)));
  link.attach(port1);
  link.send(new TextEncoder().encode('later'));
  port2.postMessage(new TextEncoder().encode('ok').buffer);
  port2.postMessage(new TextEncoder().encode('x'.repeat(17)).buffer);
  port2.postMessage('a string');
  port2.postMessage(new Uint8Array([1, 2]));
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(seen, ['early', 'later']);
  assert.deepEqual(received, ['ok']);
  port1.close();
});
