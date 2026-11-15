import assert from 'node:assert/strict';
import { test } from 'node:test';

import { extensionContext, flush, plain } from './test/harness.mjs';

const reports = (host) =>
  host.calls
    .filter((call) => call.name === 'zephium.addListener' || call.name === 'zephium.removeListener')
    .map((call) => [call.name, ...call.args]);

test('listener interest reports exact callback identities to the host', () => {
  const { chrome, host } = extensionContext();
  const event = chrome.alarms.onAlarm;
  const a = () => {};
  const b = () => {};
  assert.equal(event.hasListeners(), false);
  event.addListener(a);
  event.addListener(a);
  event.addListener(b);
  assert.equal(event.hasListener(a), true);
  assert.equal(event.hasListeners(), true);
  event.removeListener(a);
  event.removeListener(a);
  event.removeListener(b);
  assert.equal(event.hasListener(b), false);
  assert.deepEqual(reports(host), [
    ['zephium.addListener', 'alarms.onAlarm', 1, null, null],
    ['zephium.addListener', 'alarms.onAlarm', 2, null, null],
    ['zephium.removeListener', 'alarms.onAlarm', 1],
    ['zephium.removeListener', 'alarms.onAlarm', 2],
  ]);
});

test('event methods check their receiver and argument', () => {
  const { chrome, run } = extensionContext();
  const { addListener } = chrome.tabs.onRemoved;
  assert.throws(() => addListener(() => {}), run('TypeError'));
  chrome.tabs.onRemoved.addListener('nope');
  assert.equal(chrome.tabs.onRemoved.hasListeners(), false);
  assert.throws(() => chrome.tabs.onRemoved.addRules([]), { message: 'This event does not support rules.' });
});

test('events reach every listener in order, each isolated, then are acknowledged', () => {
  const { chrome, host, logs } = extensionContext();
  const calls = [];
  chrome.tabs.onRemoved.addListener((tabId, info) => calls.push(['first', tabId, plain(info)]));
  chrome.tabs.onRemoved.addListener(() => {
    throw new Error('second fails');
  });
  chrome.tabs.onRemoved.addListener((tabId) => calls.push(['third', tabId]));
  const seq = host.event('tabs.onRemoved', [7, { windowId: 1, isWindowClosing: false }]);
  assert.deepEqual(calls, [
    ['first', 7, { windowId: 1, isWindowClosing: false }],
    ['third', 7],
  ]);
  assert.deepEqual(host.acks, [seq]);
  assert.equal(logs.error.length, 1);
  assert.match(logs.error[0], /^Error in event handler: Error: second fails/);
});

test('redelivered events are acknowledged again but dispatched once', () => {
  const { chrome, host } = extensionContext();
  let count = 0;
  chrome.runtime.onStartup.addListener(() => count++);
  host.event('runtime.onStartup', [], 5n);
  host.event('runtime.onStartup', [], 5n);
  host.event('runtime.onStartup', [], 6n);
  assert.equal(count, 2);
  assert.deepEqual(host.acks, [5n, 5n, 6n]);
});

test('events nobody listens to, or that do not parse, are still acknowledged', () => {
  const { host } = extensionContext();
  host.event('tabs.onCreated', [{ id: 1 }], 1n);
  host.send({ kind: 'event', seq: 2n, name: 'tabs.onCreated', args: new TextEncoder().encode('{not json') });
  host.send({ kind: 'event', seq: 3n, name: 'tabs.onCreated', args: new TextEncoder().encode('{}') });
  assert.deepEqual(host.acks, [1n, 2n, 3n]);
});

test('a listener removed during dispatch still hears that event, not the next', () => {
  const { chrome, host } = extensionContext();
  const heard = [];
  const second = () => heard.push('second');
  chrome.tabs.onActivated.addListener(() => {
    heard.push('first');
    chrome.tabs.onActivated.removeListener(second);
  });
  chrome.tabs.onActivated.addListener(second);
  host.event('tabs.onActivated', [{ tabId: 1 }]);
  host.event('tabs.onActivated', [{ tabId: 2 }]);
  assert.deepEqual(heard, ['first', 'second', 'first']);
});

test('filtered listeners are reported one by one and only the matched ones run', () => {
  const { chrome, host } = extensionContext('background', { permissions: ['webNavigation'] });
  const heard = [];
  const event = chrome.webNavigation.onCommitted;
  event.addListener((details) => heard.push(['example', details.url]), { url: [{ hostSuffix: 'example.com' }] });
  event.addListener((details) => heard.push(['all', details.url]));
  assert.throws(
    () => event.addListener(() => {}, { url: 'example.com' }),
    /filters.url should be an array/,
  );
  assert.deepEqual(reports(host), [
    ['zephium.addListener', 'webNavigation.onCommitted', 1, { url: [{ hostSuffix: 'example.com' }] }, null],
    ['zephium.addListener', 'webNavigation.onCommitted', 2, null, null],
  ]);
  host.event('webNavigation.onCommitted', [{ url: 'https://other.org/' }, [2]]);
  host.event('webNavigation.onCommitted', [{ url: 'https://www.example.com/' }, [1, 2]]);
  host.event('webNavigation.onCommitted', [{ url: 'https://ignored.example/' }]);
  assert.deepEqual(heard, [
    ['all', 'https://other.org/'],
    ['example', 'https://www.example.com/'],
    ['all', 'https://www.example.com/'],
  ]);
  event.removeListener(heard.push);
  assert.equal(host.callsTo('zephium.removeListener').length, 0);
});

test('webRequest filters are required and asyncBlocking answers go back with the sequence', async () => {
  const { chrome, host } = extensionContext('background', { permissions: ['webRequest'] });
  const event = chrome.webRequest.onAuthRequired;
  assert.throws(() => event.addListener(() => {}), /No matching signature/);
  assert.throws(() => event.addListener(() => {}, { urls: '<all_urls>' }), /Error at property 'urls'/);
  let answer;
  event.addListener((details, callback) => (answer = callback), { urls: ['<all_urls>'] }, ['asyncBlocking']);
  event.addListener(() => ({ cancel: true }), { urls: ['https://*/*'] }, ['blocking']);
  event.addListener(() => ({ ignored: true }), { urls: ['https://*/*'] });
  const seq = host.event('webRequest.onAuthRequired', [{ requestId: '1', url: 'https://example.com/' }, [1, 2, 3]]);
  answer({ authCredentials: { username: 'u', password: 'p' } });
  answer({ cancel: true });
  await flush();
  assert.deepEqual(
    host.callsTo('zephium.eventResult').map((call) => call.args),
    [
      [String(seq), 2, { cancel: true }],
      [String(seq), 1, { authCredentials: { username: 'u', password: 'p' } }],
    ],
  );
  assert.deepEqual(host.callsTo('zephium.addListener')[0].args, [
    'webRequest.onAuthRequired',
    1,
    { urls: ['<all_urls>'] },
    ['asyncBlocking'],
  ]);
});

test('held events wait for release and keep their order', () => {
  const { chrome, host, context } = extensionContext();
  const heard = [];
  chrome.runtime.onInstalled.addListener((details) => heard.push(details.reason));
  context.hub.hold();
  host.event('runtime.onInstalled', [{ reason: 'install' }]);
  host.event('runtime.onInstalled', [{ reason: 'update' }]);
  assert.deepEqual(heard, []);
  assert.deepEqual(host.acks, []);
  context.hub.release();
  assert.deepEqual(heard, ['install', 'update']);
  assert.equal(host.acks.length, 2);
});

test('selective release dispatches installed events while preserving other held events', () => {
  const { chrome, host, context } = extensionContext();
  const heard = [];
  chrome.runtime.onInstalled.addListener((details) => heard.push(details.reason));
  chrome.runtime.onStartup.addListener(() => heard.push('startup'));
  context.hub.hold();
  const startup = host.event('runtime.onStartup', []);
  const install = host.event('runtime.onInstalled', [{ reason: 'install' }]);
  context.hub.release('runtime.onInstalled');
  const update = host.event('runtime.onInstalled', [{ reason: 'update' }]);
  assert.deepEqual(heard, ['install', 'update']);
  assert.deepEqual(host.acks, [install, update]);
  context.hub.release();
  assert.deepEqual(heard, ['install', 'update', 'startup']);
  assert.deepEqual(host.acks, [install, update, startup]);
});

test('windowTypes listener filters retain exact host-selected callback identities', () => {
  const { chrome, host } = extensionContext('background');
  const heard = [];
  const event = chrome.windows.onCreated;
  event.addListener((window) => heard.push(['normal', window.id]), { windowTypes: ['normal'] });
  event.addListener((window) => heard.push(['popup', window.id]), { windowTypes: ['popup'] });
  assert.deepEqual(reports(host), [
    ['zephium.addListener', 'windows.onCreated', 1, { windowTypes: ['normal'] }, null],
    ['zephium.addListener', 'windows.onCreated', 2, { windowTypes: ['popup'] }, null],
  ]);
  host.event('windows.onCreated', [{ id: 7, type: 'popup' }, [2]]);
  host.event('windows.onCreated', [{ id: 8, type: 'normal' }, [1]]);
  assert.deepEqual(heard, [['popup', 7], ['normal', 8]]);
});
