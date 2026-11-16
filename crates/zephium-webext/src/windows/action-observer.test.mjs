import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('./action-observer.js', import.meta.url), 'utf8');
test('observer reports per-tab icons only after a native per-tab setIcon succeeds', async () => {
  let listener, fail = false;
  const chrome = {
    action: Object.fromEntries(['setTitle', 'setBadgeText', 'setIcon', 'setPopup', 'enable', 'disable'].map(method => [method, async () => { if (fail) throw Error('native failure'); }])),
    runtime: {id: 'own', sendMessage: async () => {}, onMessage: {addListener: fn => { listener = fn; }}}
  };
  vm.runInNewContext(source, {chrome});
  const snapshot = (tabId, sender = 'own') => {
    let result;
    listener({__zephiumActionSnapshot: true, tabId}, {id: sender}, value => { result = value; });
    return result;
  };
  assert.equal(snapshot(1).perTabIcons, false);
  await chrome.action.setIcon({path: 'global.png'});
  assert.equal(snapshot(1).perTabIcons, false);
  assert.equal(snapshot(1).icon.path, 'global.png');
  fail = true;
  await assert.rejects(chrome.action.setIcon({tabId: 1, path: 'failed.png'}));
  assert.equal(snapshot(1).perTabIcons, false);
  fail = false;
  await chrome.action.setIcon({tabId: 1, path: 'tab.png'});
  assert.equal(snapshot(1).perTabIcons, true);
  assert.equal(snapshot(1).icon.path, 'tab.png');
  assert.equal(snapshot(2).icon.path, 'global.png');
  assert.equal(snapshot(1, 'foreign'), undefined);
});

test('observer never throws into the extension after the worker replaces its chrome global', async () => {
  // Proton Pass swaps `chrome` for a locked Proxy once its worker starts.
  const sent = [];
  const chrome = {
    action: {setTitle: async () => {}, setBadgeText: () => undefined, setIcon: async () => {},
      setPopup: async () => {}, enable: async () => {}, disable: async () => {}},
    runtime: {id: 'own', sendMessage: async message => { sent.push(message); }, onMessage: {addListener: () => {}}}
  };
  const context = vm.createContext({chrome});
  vm.runInContext(source, context);
  context.chrome = new Proxy({}, {get: () => undefined});
  // A callback-style call returns no promise, so the report runs synchronously.
  assert.doesNotThrow(() => chrome.action.setBadgeText({text: '1'}));
  await chrome.action.setTitle({title: 'x'});
  await Promise.resolve();
  assert.equal(sent.length, 2);
  assert.equal(sent.every(message => message.__zephiumActionChanged === true), true);
});
