import assert from 'node:assert/strict';
import { test } from 'node:test';

import { EXTENSION_ID, extensionContext, flush, plain } from './test/harness.mjs';

const SENDER = {
  id: EXTENSION_ID,
  url: 'https://example.com/',
  tab: { id: 4 },
  frameId: 0,
  origin: 'https://example.com',
};

function responder(host) {
  const responses = [];
  host.handlers['zephium.respond'] = ([token, listener, response]) => {
    responses.push([token, listener, response]);
    return null;
  };
  return responses;
}

test('sendMessage carries the message as JSON text and resolves with the response', async () => {
  const { chrome, host } = extensionContext('contentScript');
  host.handlers['runtime.sendMessage'] = ([target, text, options]) => ({
    response: JSON.stringify({ echo: JSON.parse(text), target, options }),
  });
  assert.deepEqual(plain(await chrome.runtime.sendMessage({ a: [1, 'b'] })), {
    echo: { a: [1, 'b'] },
    target: null,
    options: null,
  });
  assert.deepEqual(plain(await chrome.runtime.sendMessage('other-id', 'hi')), {
    echo: 'hi',
    target: 'other-id',
    options: null,
  });
  assert.deepEqual(plain(await chrome.runtime.sendMessage({ m: 1 }, { includeTlsChannelId: true })), {
    echo: { m: 1 },
    target: null,
    options: { includeTlsChannelId: true },
  });
  assert.deepEqual(plain(await chrome.runtime.sendMessage(null, 'x')), { echo: 'x', target: null, options: null });
  assert.deepEqual(plain(await chrome.runtime.sendMessage('id', 'x', {})), { echo: 'x', target: 'id', options: {} });
  assert.deepEqual(host.callsTo('runtime.sendMessage')[0].args, [null, '{"a":[1,"b"]}', null]);
  let callbackResponse;
  assert.equal(
    chrome.runtime.sendMessage('cb', (response) => (callbackResponse = response)),
    undefined,
  );
  await flush();
  assert.deepEqual(plain(callbackResponse), { echo: 'cb', target: null, options: null });
});

test("no response resolves undefined, or calls back with Chrome's port-closed lastError", async () => {
  const { chrome, host, logs } = extensionContext('contentScript');
  host.handlers['runtime.sendMessage'] = () => null;
  assert.equal(await chrome.runtime.sendMessage('x'), undefined);
  let seen;
  chrome.runtime.sendMessage('x', (...args) => (seen = [args.length, chrome.runtime.lastError?.message]));
  chrome.runtime.sendMessage('x', () => {});
  await flush();
  assert.deepEqual(seen, [0, 'The message port closed before a response was received.']);
  assert.deepEqual(logs.error, [
    'Unchecked runtime.lastError: The message port closed before a response was received.',
  ]);
  host.handlers['runtime.sendMessage'] = () => ({ response: null });
  assert.equal(await chrome.runtime.sendMessage('x'), undefined);
  host.handlers['runtime.sendMessage'] = () => {
    throw { error: 'Could not establish connection. Receiving end does not exist.' };
  };
  await assert.rejects(chrome.runtime.sendMessage('x'), {
    message: 'Could not establish connection. Receiving end does not exist.',
  });
});

test('messages serialize as Chrome does: JSON semantics, refusals thrown at the call', async () => {
  const { chrome, host, run } = extensionContext('contentScript');
  host.handlers['runtime.sendMessage'] = ([, text]) => ({ response: text });
  const value = run(
    '({ keep: 1, drop: undefined, fn() {}, [Symbol("s")]: 1, date: new Date(0), nan: NaN, list: [undefined, () => 1], nested: { toJSON: () => "custom" } })',
  );
  assert.deepEqual(plain(await chrome.runtime.sendMessage(value)), {
    keep: 1,
    date: '1970-01-01T00:00:00.000Z',
    nan: null,
    list: [null, null],
    nested: 'custom',
  });
  const Error = run('Error');
  assert.throws(
    () => chrome.runtime.sendMessage(Symbol('s')),
    (error) => error instanceof Error && error.message === 'Could not serialize message.',
  );
  assert.throws(() => chrome.runtime.sendMessage(1n), { message: 'Could not serialize message.' });
  const cyclic = {};
  cyclic.self = cyclic;
  assert.throws(() => chrome.runtime.sendMessage(cyclic), { message: 'Could not serialize message.' });
  assert.throws(() => chrome.runtime.sendMessage(() => 1), /No matching signature/);
  assert.throws(() => chrome.runtime.sendMessage(undefined), run('TypeError'));
  assert.throws(() => chrome.runtime.sendMessage(), /No matching signature/);
  assert.throws(() => chrome.runtime.sendMessage('a', 'b', {}, {}), /No matching signature/);
  assert.throws(() => chrome.runtime.sendMessage('x'.repeat(8 * 1024 * 1024)), {
    message: 'Message exceeded maximum allowed size of 64MiB.',
  });
  assert.equal(host.callsTo('runtime.sendMessage').length, 1);
});

test('tabs.sendMessage takes the tab, the message and options', async () => {
  const { chrome, host } = extensionContext('background');
  host.handlers['tabs.sendMessage'] = (args) => ({ response: JSON.stringify(args) });
  assert.deepEqual(plain(await chrome.tabs.sendMessage(3, { ping: true }, { frameId: 2 })), [
    3,
    '{"ping":true}',
    { frameId: 2 },
  ]);
  assert.throws(() => chrome.tabs.sendMessage('3', {}), /Error at parameter 'tabId'/);
});

test('onMessage: synchronous sendResponse, the first answer wins', () => {
  const { chrome, host } = extensionContext('background');
  const responses = responder(host);
  const seen = [];
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    seen.push(['first', plain(message), sender]);
    sendResponse({ ok: 1 });
  });
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    seen.push(['second', sender === seen[0][2]]);
    sendResponse({ ok: 2 });
  });
  const seq = host.event('runtime.onMessage', ['{"q":"x"}', SENDER, '17', [1, 2]]);
  assert.deepEqual(plain(seen[0][1]), { q: 'x' });
  assert.deepEqual(plain(seen[0][2]), SENDER);
  assert.deepEqual(seen[1], ['second', true]);
  assert.deepEqual(responses, [['17', 1, { response: '{"ok":1}' }], ['17', 2, { response: '{"ok":2}' }]]);
  assert.deepEqual(host.acks, [seq]);
});

test('onMessage: returning true keeps the channel open for a later sendResponse', async () => {
  const { chrome, host } = extensionContext('background');
  const responses = responder(host);
  let later;
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    later = sendResponse;
    return true;
  });
  chrome.runtime.onMessage.addListener(() => undefined);
  host.event('runtime.onMessage', ['1', SENDER, '9', [1, 2]]);
  await flush();
  assert.deepEqual(responses, [['9', 1, { async: 'callback' }], ['9', 2, null]]);
  later('late');
  later('ignored');
  await flush();
  assert.deepEqual(responses, [['9', 1, { async: 'callback' }], ['9', 2, null], ['9', 1, { response: '"late"' }]]);
});

test('onMessage: a returned promise answers with its value or its error', async () => {
  const { chrome, host } = extensionContext('background');
  const responses = responder(host);
  chrome.runtime.onMessage.addListener((message) =>
    message === 'fail' ? Promise.reject(new Error('nope')) : Promise.resolve({ got: message }),
  );
  host.event('runtime.onMessage', ['"a"', SENDER, '1', [1]]);
  host.event('runtime.onMessage', ['"fail"', SENDER, '2', [1]]);
  await flush();
  assert.deepEqual(responses, [
    ['1', 1, { async: 'promise' }],
    ['2', 1, { async: 'promise' }],
    ['1', 1, { response: '{"got":"a"}' }],
    ['2', 1, { error: 'nope' }],
  ]);
});

test('onMessage consumes a returned rejection after synchronous sendResponse already won', async () => {
  const { chrome, host } = extensionContext('background');
  const responses = responder(host);
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    sendResponse('first');
    return Promise.reject(new Error('ignored after response'));
  });
  host.event('runtime.onMessage', ['1', SENDER, '3', [1]]);
  await flush();
  assert.deepEqual(responses, [['3', 1, { response: '"first"' }]]);
});

test('onMessage: no listener answering closes the channel without a response', async () => {
  const { chrome, host } = extensionContext('background');
  const responses = responder(host);
  let late;
  const listener = (message, sender, sendResponse) => {
    late = sendResponse;
    return 'not true';
  };
  chrome.runtime.onMessage.addListener(listener);
  host.event('runtime.onMessage', ['1', SENDER, '3', [1]]);
  late('too late');
  chrome.runtime.onMessage.removeListener(listener);
  host.event('runtime.onMessage', ['1', SENDER, '4', [1]]);
  host.event('runtime.onMessageExternal', ['1', SENDER, '5', [1]]);
  chrome.runtime.onMessage.addListener(listener);
  host.event('runtime.onMessage', ['not json', SENDER, '6', [2]]);
  await flush();
  assert.deepEqual(responses, [
    ['3', 1, null],
    ['4', 1, null],
    ['5', 1, null],
    ['6', 2, null],
  ]);
});

test('sendResponse() with nothing answers undefined; unserializable responses throw in the listener', async () => {
  const { chrome, host, logs } = extensionContext('background');
  const responses = responder(host);
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (message === 'bad') sendResponse(() => 1);
    else sendResponse();
  });
  host.event('runtime.onMessage', ['"ok"', SENDER, '1', [1]]);
  host.event('runtime.onMessage', ['"bad"', SENDER, '2', [1]]);
  await flush();
  assert.deepEqual(responses, [
    ['1', 1, { response: null }],
    ['2', 1, null],
  ]);
  assert.match(logs.error[0], /Error in event handler: Error: Could not serialize message\./);
});

test('runtime.connect gives a usable port at once; messages wait for the host id', async () => {
  const { chrome, host } = extensionContext('contentScript');
  host.handlers['runtime.connect'] = () => host.LATER;
  const port = chrome.runtime.connect({ name: 'channel' });
  assert.equal(port.name, 'channel');
  assert.equal(port.sender, undefined);
  port.postMessage({ n: 1 });
  port.postMessage({ n: 2 });
  assert.deepEqual(host.callsTo('runtime.connect')[0].args, [null, { name: 'channel' }]);
  assert.equal(host.portMessages.length, 0);
  host.reply(host.callsTo('runtime.connect')[0].id, '18446744073709551615');
  await flush();
  port.postMessage({ n: 3 });
  assert.deepEqual(host.portMessages, [
    { port: 18446744073709551615n, message: { n: 1 } },
    { port: 18446744073709551615n, message: { n: 2 } },
    { port: 18446744073709551615n, message: { n: 3 } },
  ]);
  const received = [];
  port.onMessage.addListener((message, from) => received.push([plain(message), from === port]));
  host.portMessage(18446744073709551615n, { reply: true });
  assert.deepEqual(received, [[{ reply: true }, true]]);
  assert.throws(() => chrome.runtime.connect({ name: 5 }), /Error at property 'name'/);
});

test('ports: remote disconnect fires onDisconnect with lastError; local disconnect does not', async () => {
  const { chrome, host, run } = extensionContext('background');
  let next = 1n;
  host.handlers['runtime.connect'] = () => String(next++);
  host.handlers['tabs.connect'] = () => String(next++);
  const first = chrome.runtime.connect();
  const second = chrome.tabs.connect(4, { name: 'tab', frameId: 0 });
  await flush();
  assert.deepEqual(host.callsTo('tabs.connect')[0].args, [4, { name: 'tab', frameId: 0 }]);
  const events = [];
  first.onDisconnect.addListener((port) => events.push(['first', port === first, chrome.runtime.lastError?.message]));
  second.onDisconnect.addListener(() => events.push(['second']));
  host.portDisconnect(
    1n,
    'The page keeping the extension port is moved into back/forward cache, so the message channel is closed.',
  );
  second.disconnect();
  second.disconnect();
  assert.deepEqual(host.disconnects, [2n]);
  assert.deepEqual(events, [
    [
      'first',
      true,
      'The page keeping the extension port is moved into back/forward cache, so the message channel is closed.',
    ],
  ]);
  assert.throws(
    () => first.postMessage(1),
    (error) => error instanceof run('Error') && error.message === 'Attempting to use a disconnected port object',
  );
  assert.throws(() => second.postMessage(1), { message: 'Attempting to use a disconnected port object' });
  host.portMessage(1n, 'after');
  const { postMessage } = first;
  assert.throws(() => postMessage.call({}, 1), run('TypeError'));
});

test('ports: a refused connection reports why through onDisconnect', async () => {
  const { chrome, host, logs } = extensionContext('contentScript');
  host.handlers['runtime.connect'] = () => {
    throw { error: 'Could not establish connection. Receiving end does not exist.' };
  };
  const port = chrome.runtime.connect();
  let seen;
  port.onDisconnect.addListener(() => (seen = chrome.runtime.lastError.message));
  chrome.runtime.connect();
  await flush();
  assert.equal(seen, 'Could not establish connection. Receiving end does not exist.');
  assert.deepEqual(logs.error, [
    'Unchecked runtime.lastError: Could not establish connection. Receiving end does not exist.',
  ]);
});

test('ports: disconnecting before the host answers closes the host port when it arrives', async () => {
  const { chrome, host } = extensionContext('contentScript');
  host.handlers['runtime.connect'] = () => host.LATER;
  const port = chrome.runtime.connect();
  port.postMessage('queued');
  port.disconnect();
  host.reply(host.calls[0].id, '44');
  await flush();
  assert.deepEqual(host.disconnects, [44n]);
  assert.equal(host.portMessages.length, 0);
});

test('onConnect hands out a port with the host-supplied name and sender', () => {
  const { chrome, host } = extensionContext('background');
  const ports = [];
  chrome.runtime.onConnect.addListener((port) => ports.push(port));
  host.event('runtime.onConnect', ['77', 'popup', SENDER]);
  assert.equal(ports.length, 1);
  assert.equal(ports[0].name, 'popup');
  assert.deepEqual(plain(ports[0].sender), SENDER);
  ports[0].postMessage({ hello: 1 });
  assert.deepEqual(host.portMessages, [{ port: 77n, message: { hello: 1 } }]);
});

test('connections nobody accepts are closed, and malformed ids ignored', () => {
  const { host } = extensionContext('background');
  host.event('runtime.onConnect', ['5', '', SENDER]);
  host.event('runtime.onConnectExternal', ['6', '', SENDER]);
  host.event('runtime.onConnect', ['0', '', SENDER]);
  host.event('runtime.onConnect', ['18446744073709551616', '', SENDER]);
  host.event('runtime.onConnect', [5, '', SENDER]);
  assert.deepEqual(host.disconnects, [5n, 6n]);
});

test('context death disconnects every open port and its listeners hear it', async () => {
  const { chrome, host } = extensionContext('contentScript');
  host.handlers['runtime.connect'] = () => '8';
  const port = chrome.runtime.connect({ name: 'live' });
  await flush();
  let fired = 0;
  port.onDisconnect.addListener(() => fired++);
  host.event('zephium.onInvalidated', ['Extension context invalidated.']);
  assert.equal(fired, 1);
  assert.equal(chrome.runtime.id, undefined);
  assert.throws(() => port.postMessage(1), { message: 'Attempting to use a disconnected port object' });
});

test('native ports and messages need nativeMessaging and a privileged context', async () => {
  const { chrome, host } = extensionContext('background', { permissions: ['nativeMessaging'] });
  host.handlers['runtime.connectNative'] = () => '3';
  host.handlers['runtime.sendNativeMessage'] = ([app, text]) => ({
    response: JSON.stringify({ app, got: JSON.parse(text) }),
  });
  const port = chrome.runtime.connectNative('com.example.host');
  assert.equal(port.name, '');
  assert.deepEqual(plain(await chrome.runtime.sendNativeMessage('com.example.host', { a: 1 })), {
    app: 'com.example.host',
    got: { a: 1 },
  });
  const content = extensionContext('contentScript', { permissions: ['nativeMessaging'] });
  assert.equal(content.chrome.runtime.connectNative, undefined);
});
