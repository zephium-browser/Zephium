import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('./worker-compat.js', import.meta.url), 'utf8');
function event() {
  const listeners = new Set();
  return {listeners, addListener: fn => listeners.add(fn), removeListener: fn => listeners.delete(fn),
    emit: (...args) => [...listeners].map(fn => fn(...args))};
}
function fixture() {
  const calls = [], timers = new Set();
  const chrome = {
    runtime: {id: 'a'.repeat(32), onMessage: event(), getURL: path => `chrome-extension://${'a'.repeat(32)}/${path.replace(/^\/+/, '')}`},
    tabs: {onCreated: event(), create: (properties, callback) => calls.push({properties, callback}),
      get: async id => ({id, url: 'https://example.com/'})},
    action: {onClicked: event(), isEnabled: async () => true, getPopup: async () => ''}
  };
  const context = vm.createContext({chrome, URL, setTimeout: fn => { timers.add(fn); return fn; }, clearTimeout: fn => timers.delete(fn)});
  vm.runInContext(source, context);
  return {chrome, calls, timers, context};
}

test('create and the action relay keep working after the worker replaces its chrome global', async () => {
  // Proton Pass swaps `chrome` for a locked Proxy once its worker starts.
  const {chrome, calls, context} = fixture();
  const {runtime} = chrome;
  context.chrome = new Proxy({}, {get: () => undefined});
  const result = chrome.tabs.create({url: 'https://example.com/'});
  calls[0].callback({id: 7});
  assert.equal((await result).id, 7);
  const host = {id: runtime.id, url: runtime.getURL('zephium-windows-host/host.html')};
  const seen = [];
  chrome.action.onClicked.addListener(tab => seen.push(tab.id));
  const reply = await new Promise(resolve => runtime.onMessage.emit({__zephiumActionClick: true, tabId: 5}, host, resolve));
  assert.equal(reply.dispatched, true);
  assert.deepEqual(seen, [5]);
});

test('native Tab result wins and the temporary listener is removed', async () => {
  const {chrome, calls, timers} = fixture();
  const result = chrome.tabs.create({url: 'https://example.com/'});
  calls[0].callback({id: 7});
  assert.equal((await result).id, 7);
  assert.equal(chrome.tabs.onCreated.listeners.size, 0);
  assert.equal(timers.size, 0);
});

test('concurrent same-URL creates settle FIFO in promise and callback forms', async () => {
  const {chrome, calls} = fixture();
  const first = chrome.tabs.create({url: 'https://example.com/'});
  let callbackTab;
  assert.equal(chrome.tabs.create({url: 'https://example.com/'}, tab => callbackTab = tab), undefined);
  chrome.tabs.onCreated.emit({id: 11});
  calls[0].callback(undefined);
  calls[1].callback(undefined);
  chrome.tabs.onCreated.emit({id: 12});
  assert.equal((await first).id, 11);
  await Promise.resolve();
  assert.equal(callbackTab.id, 12);
  assert.equal(chrome.tabs.onCreated.listeners.size, 0);
});

test('a user-created tab is ignored and different URLs settle out of order', async () => {
  const {chrome, calls, timers} = fixture();
  const first = chrome.tabs.create({url: 'https://EXAMPLE.com:443'});
  let callbackTab;
  chrome.tabs.create({url: 'https://example.org/login'}, tab => callbackTab = tab);
  calls.forEach(call => call.callback());
  chrome.tabs.onCreated.emit({id: 90, url: 'https://unrelated.example/'});
  await Promise.resolve();
  assert.equal(callbackTab, undefined);
  assert.equal(timers.size, 2);
  chrome.tabs.onCreated.emit({id: 12, pendingUrl: 'https://example.org/login', url: 'about:blank'});
  chrome.tabs.onCreated.emit({id: 11, url: 'https://example.com/'});
  assert.equal((await first).id, 11);
  assert.equal(callbackTab.id, 12);
  assert.equal(timers.size, 0);
  assert.equal(chrome.tabs.onCreated.listeners.size, 0);
});

test('relative extension URLs match native absolute URLs before FIFO', async () => {
  const {chrome, calls} = fixture();
  const first = chrome.tabs.create({url: 'first.html'});
  const second = chrome.tabs.create({url: 'second.html'});
  calls.forEach(call => call.callback());
  chrome.tabs.onCreated.emit({id: 22, url: chrome.runtime.getURL('second.html')});
  chrome.tabs.onCreated.emit({id: 21, url: chrome.runtime.getURL('first.html')});
  assert.equal((await first).id, 21);
  assert.equal((await second).id, 22);
});

test('native errors reject promises and expose callback lastError only during callback', async () => {
  const {chrome, calls} = fixture();
  const first = chrome.tabs.create({url: 'invalid'});
  chrome.runtime.lastError = {message: 'Native rejection'};
  calls[0].callback();
  delete chrome.runtime.lastError;
  await assert.rejects(first, /Native rejection/);
  let message;
  chrome.tabs.create({url: 'invalid'}, () => message = chrome.runtime.lastError?.message);
  chrome.runtime.lastError = {message: 'Callback rejection'};
  calls[1].callback();
  delete chrome.runtime.lastError;
  await Promise.resolve();
  assert.equal(message, 'Callback rejection');
  assert.equal(chrome.runtime.lastError, undefined);
  assert.equal(chrome.tabs.onCreated.listeners.size, 0);
});

test('a denied create with no native event times out without leaking listeners', async () => {
  const {chrome, calls, timers} = fixture();
  const result = chrome.tabs.create({url: 'https://example.com/'});
  calls[0].callback();
  for (const timer of [...timers]) timer();
  await assert.rejects(result, /timed out/);
  assert.equal(timers.size, 0);
  assert.equal(chrome.tabs.onCreated.listeners.size, 0);
});

test('action relay checks host identity and honors listener removal and popup changes', async () => {
  const {chrome} = fixture();
  const seen = [];
  const removed = () => assert.fail('removed listener called');
  chrome.action.onClicked.addListener(removed);
  chrome.action.onClicked.removeListener(removed);
  chrome.action.onClicked.addListener(tab => seen.push(tab.id));
  const host = {id: chrome.runtime.id, url: chrome.runtime.getURL('zephium-windows-host/host.html')};
  const message = {__zephiumActionClick: true, tabId: 23};
  chrome.runtime.onMessage.emit(message, {...host, url: 'https://example.com/'}, () => assert.fail('web sender accepted'));
  const dispatch = () => new Promise(resolve => chrome.runtime.onMessage.emit(message, host, resolve));
  assert.equal((await dispatch()).dispatched, true);
  assert.deepEqual(seen, [23]);
  chrome.action.getPopup = async () => chrome.runtime.getURL('popup.html');
  assert.match((await dispatch()).error, /Action changed/);
  assert.deepEqual(seen, [23]);
});
