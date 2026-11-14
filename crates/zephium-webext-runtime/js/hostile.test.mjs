import assert from 'node:assert/strict';
import { test } from 'node:test';

import { FakeHost, bootstrap, createRealm, plain, surface } from './test/harness.mjs';

// Replaces the built-ins the binding uses, as a page (in the page world) or
// careless extension code (in its own world) could after bootstrap.
const SABOTAGE = `(() => {
  const OriginalError = Error;
  const evil = function () { throw new OriginalError('hijacked built-in'); };
  const define = Object.defineProperty;
  const each = (list, fn) => { for (let i = 0; i < list.length; i++) fn(list[i]); };
  const typed = Object.getPrototypeOf(Uint8Array.prototype);
  each(['response', 'error', 'kind', 'idBase', 'idLimit', 'global', 'defineGlobals', 'contexts', 'permissions',
        'returns', 'filter', 'includeGlobs', 'excludeMatches', 'allFrames', 'matchAboutBlank', 'fileAccess'],
       (name) => define(Object.prototype, name, { get: evil, configurable: true }));
  define(typed, 'length', { get: evil });
  typed.set = evil;
  typed.subarray = evil;
  JSON.parse = evil;
  JSON.stringify = evil;
  Promise.prototype.then = evil;
  Promise.resolve = evil;
  globalThis.Promise = evil;
  each(['get', 'set', 'has', 'delete', 'forEach', 'clear'], (name) => (Map.prototype[name] = evil));
  each(['add', 'has', 'delete', 'forEach'], (name) => (Set.prototype[name] = evil));
  each(['get', 'set', 'has'], (name) => (WeakMap.prototype[name] = evil));
  each(['slice', 'indexOf', 'lastIndexOf', 'startsWith', 'endsWith', 'toLowerCase', 'charCodeAt'], (name) => (String.prototype[name] = evil));
  each(['getUint32', 'setUint32', 'getBigUint64', 'setBigUint64'], (name) => (DataView.prototype[name] = evil));
  each(['defineProperty', 'keys', 'freeze', 'create', 'hasOwn', 'getPrototypeOf', 'getOwnPropertyDescriptor'], (name) => (Object[name] = evil));
  String.fromCharCode = evil;
  Number.isInteger = evil;
  Math.min = Math.max = Math.floor = evil;
  Symbol.for = evil;
  Reflect.apply = evil;
  Reflect.deleteProperty = evil;
  globalThis.Uint8Array = evil;
  globalThis.DataView = evil;
  globalThis.TextEncoder = evil;
  globalThis.TextDecoder = evil;
  globalThis.URL = evil;
  globalThis.Error = evil;
  globalThis.TypeError = evil;
  globalThis.setTimeout = evil;
  globalThis.queueMicrotask = evil;
  console.error = evil;
  Function.prototype.call = evil;
  Function.prototype.apply = evil;
  Function.prototype.bind = evil;
  Array.isArray = evil;
  each(['push', 'slice', 'splice', 'indexOf', 'join', 'map', 'forEach'], (name) => (Array.prototype[name] = evil));
  Array.prototype[Symbol.iterator] = evil;
})()`;

test('replacing built-ins after bootstrap does not break calls, events, messages or ports', async () => {
  const realm = createRealm();
  const host = new FakeHost(realm.Z, 'background');
  host.handlers['tabs.query'] = () => [{ id: 1 }];
  host.handlers['tabs.get'] = () => {
    throw { error: 'No tab with id: 2.' };
  };
  host.handlers['runtime.sendMessage'] = ([, text]) => ({ response: text });
  host.handlers['runtime.connect'] = () => '31';
  const { chrome } = realm.Z.api.create(surface, bootstrap({ contextKind: 'background' }), host.link, {});
  const OriginalTypeError = realm.run('TypeError');
  const then = realm.run('Promise.prototype.then');
  realm.run(SABOTAGE);

  const results = [];
  const done = new Promise((resolve) => {
    let pending = 4;
    const finish = () => --pending === 0 && resolve();
    chrome.tabs.query({}, (tabs) => {
      results.push(['query', plain(tabs)]);
      finish();
    });
    chrome.tabs.get(2, () => {
      results.push(['get', chrome.runtime.lastError.message]);
      finish();
    });
    chrome.runtime.sendMessage({ hi: 1 }, (response) => {
      results.push(['send', plain(response)]);
      finish();
    });
    const promise = chrome.tabs.query({});
    Reflect.apply(then, promise, [
      (tabs) => {
        results.push(['promise', plain(tabs)]);
        finish();
      },
    ]);
  });
  await done;
  assert.deepEqual(results.sort(), [
    ['get', 'No tab with id: 2.'],
    ['promise', [{ id: 1 }]],
    ['query', [{ id: 1 }]],
    ['send', { hi: 1 }],
  ]);
  assert.throws(
    () => chrome.tabs.get('x'),
    (error) => error instanceof OriginalTypeError && /Invalid type: expected integer/.test(error.message),
  );

  const heard = [];
  chrome.tabs.onRemoved.addListener((id) => heard.push(id));
  chrome.tabs.onRemoved.addListener(() => {
    throw new Error('listener failure');
  });
  const seq = host.event('tabs.onRemoved', [5, {}]);
  assert.deepEqual(heard, [5]);
  assert.ok(host.acks.includes(seq));
  assert.match(realm.logs.error.at(-1), /Error in event handler/);

  host.handlers['zephium.respond'] = (args) => (heard.push(args), null);
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => sendResponse({ echo: message }));
  host.event('runtime.onMessage', ['"ping"', { id: 'x' }, '3']);

  const port = chrome.runtime.connect({ name: 'p' });
  const portMessages = [];
  port.onMessage.addListener((message) => portMessages.push(plain(message)));
  port.postMessage({ queued: true });
  await new Promise((resolve) => setTimeout(resolve, 10));
  host.portMessage(31n, { back: 1 });
  port.disconnect();
  assert.deepEqual(portMessages, [{ back: 1 }]);
  assert.deepEqual(host.portMessages, [{ port: 31n, message: { queued: true } }]);
  assert.deepEqual(host.disconnects, [31n]);
  assert.deepEqual(heard[1], ['3', { response: '{"echo":"ping"}' }]);
  assert.equal(chrome.i18n.getMessage('greeting', ['A', 'B']), 'Hello A, you have B messages');
  assert.equal(chrome.runtime.getURL('x.html'), 'chrome-extension://abcdefghijklmnopabcdefghijklmnop/x.html');
});

test('the content guard and a lazily created API survive a sabotaged world', async () => {
  const realm = createRealm();
  const host = new FakeHost(realm.Z, 'contentScript');
  host.handlers['storage.local.get'] = () => ({ a: 1 });
  const frame = { href: 'https://example.com/page', origin: 'https://example.com', top: false, creator: () => null };
  realm.Z.content.boot(surface, bootstrap({ contextKind: 'contentScript' }), { link: host.link, frame });
  const guard = realm.run("globalThis[Symbol.for('zephium.content.v1')]");
  realm.run(SABOTAGE);
  assert.equal(
    guard.run(realm.run("({ matches: ['https://example.com/*'] })")),
    false,
    'top-frame-only script in a subframe',
  );
  assert.equal(guard.run(realm.run("({ matches: ['https://other.org/*'], allFrames: true })")), false);
  assert.equal(guard.run(realm.run("({ matches: ['https://example.com/*'], allFrames: true })")), true);
  const chrome = realm.run('chrome');
  assert.equal(chrome.tabs, undefined);
  const value = await new Promise((resolve) =>
    chrome.storage.local.get('a', (items) => resolve(items ?? chrome.runtime.lastError)),
  );
  assert.deepEqual(plain(value), { a: 1 });
  assert.ok(host.calls.every((call) => call.id > 0 && call.id < 0x80000000));
});
