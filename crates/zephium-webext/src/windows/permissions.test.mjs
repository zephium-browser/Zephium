import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const read = (name) => readFileSync(new URL(name, import.meta.url), 'utf8');
const shim = read('./permissions.js');
const hostShim = read('./permissions-host.js');
const origin = 'chrome-extension://fixture/';
const manifest = {
  optional_permissions: ['privacy'],
  optional_host_permissions: ['https://fixture.test/*'],
  background: {service_worker: 'worker.js'}
};
const request = {permissions: ['privacy']};

function fixture({worker = false, gesture = true} = {}) {
  const pending = [];
  const activation = {isActive: false};
  let listener;
  let onNativeMessage;
  let requested = 0;
  let granted = [];
  let origins = [];
  const settings = new Map();
  const specContains = (requested, permissions) =>
    requested.permissions.every(permission => permissions.includes(permission)) && requested.origins.length === 0;
  const sender = {id: 'fixture', url: origin + (worker ? 'worker.js' : 'options.html')};
  const defaults = {
    id: sender.id,
    getURL: (path) => new URL(path, origin).href,
    getManifest: () => manifest,
  };
  function replyNative(id, result) {
    onNativeMessage({data: {kind: 'permissions-result', id, result}});
  }
  function post(payload) {
    const {kind, id, spec, op, setting, value} = JSON.parse(payload);
    if (kind === 'permissions-toolbar-request') {
      replyNative(id, false);
    } else if (kind === 'permissions-privacy') {
      if (!granted.includes('privacy')) replyNative(id, false);
      else if (op === 'get') replyNative(id, {value: settings.get(setting) ?? true,
        levelOfControl: settings.has(setting) ? 'controlled_by_this_extension' : 'controllable_by_this_extension'});
      else {
        if (op === 'set') settings.set(setting, value);
        else settings.delete(setting);
        replyNative(id, true);
      }
    } else if (kind === 'permissions-remove') {
      granted = granted.filter(permission => !spec.permissions.includes(permission));
      origins = origins.filter(origin => !spec.origins.includes(origin));
      replyNative(id, true);
    } else {
      replyNative(id, {permissions: granted, origins});
    }
  }
  vm.runInNewContext(hostShim, {
    chrome: {
      runtime: {...defaults, onMessage: {addListener: (l) => { listener = l; }}},
      webview: {postMessage: post, addEventListener: (_event, l) => { onNativeMessage = l; }}
    },
    navigator: {userActivation: activation},
    window: {open(url) {
      assert.ok(activation.isActive, 'the native gesture check is required');
      assert.match(url, /^about:blank#zephium-permissions=/);
      const {id, spec} = JSON.parse(decodeURIComponent(url.split('=')[1]));
      activation.isActive = false;
      ++requested;
      pending.push(allowed => {
        if (allowed) {
          granted.push(...spec.permissions);
          origins.push(...spec.origins);
        }
        replyNative(id, allowed);
      });
    }},
    setTimeout, clearTimeout
  });
  const chrome = {
    permissions: {
      request() { assert.fail('the native permission dialog must never run'); },
      contains: async spec => specContains(spec, ['storage']),
      getAll: async () => ({permissions: ['storage'], origins: []}),
      remove: async () => true
    },
    runtime: {...defaults, sendMessage(message) {
      activation.isActive = gesture;
      return new Promise(resolve => {
        const waiting = listener(message, sender, resolve);
        if (!waiting) resolve(undefined);
      });
    }}
  };
  const page = worker ? {} : {document: {}, navigator: {userActivation: {isActive: gesture}}};
  const context = vm.createContext({chrome, ...page});
  vm.runInContext(shim, context);
  return {chrome, context, pending, requested: () => requested,
    dispatch: (message, extensionSender) => listener(message, extensionSender, () => assert.fail('unexpected reply'))};
}

for (const worker of [false, true]) {
  test(`listed optional permission reaches the host and Allow is reflected in the API (${worker ? 'worker' : 'page'})`, async () => {
    const {chrome, pending, requested} = fixture({worker});
    const result = chrome.permissions.request(request);
    assert.equal(requested(), 1);
    assert.equal(await chrome.permissions.contains(request), false);
    pending.shift()(true);
    assert.equal(await result, true);
    assert.equal(await chrome.permissions.contains({permissions: ['storage', 'privacy']}), true);
    assert.deepEqual(Array.from((await chrome.permissions.getAll()).permissions), ['storage', 'privacy']);
    assert.equal(await chrome.permissions.remove(request), true);
    assert.equal(await chrome.permissions.contains(request), false);
  });
}

test('unlisted names and malformed or oversized input create no prompt', async () => {
  const {chrome, requested} = fixture();
  for (const value of [
    {permissions: ['tabs']}, {origins: ['https://other.test/*']},
    {permissions: [null]}, {permissions: Array(17).fill('privacy')}, {permissions: [42]}
  ]) {
    assert.equal(await chrome.permissions.request(value), false);
  }
  assert.equal(requested(), 0);
});

test('listed optional origins can be granted and removed', async () => {
  const {chrome, pending} = fixture();
  const spec = {origins: manifest.optional_host_permissions};
  const result = chrome.permissions.request(spec);
  pending.shift()(true);
  assert.equal(await result, true);
  assert.equal(await chrome.permissions.contains(spec), true);
  assert.deepEqual(Array.from((await chrome.permissions.getAll()).origins), spec.origins);
  assert.equal(await chrome.permissions.remove(spec), true);
  assert.equal(await chrome.permissions.contains(spec), false);
});

test('a page or worker without gesture creates no prompt', async () => {
  for (const worker of [false, true]) {
    const {chrome, requested} = fixture({worker, gesture: false});
    assert.equal(await chrome.permissions.request(request), false);
    assert.equal(requested(), 0);
  }
});

test('Deny settles false and a concurrent request cannot replace the prompt', async () => {
  const {chrome, pending, requested} = fixture();
  const result = new Promise(resolve => {
    assert.equal(chrome.permissions.request(request, resolve), undefined);
  });
  assert.equal(await chrome.permissions.request(request), false);
  assert.equal(requested(), 1);
  pending.shift()(false);
  assert.equal(await result, false);
  assert.equal(await chrome.permissions.contains(request), false);
});

test('replacing the global with a Proxy preserves the installed wrappers', async () => {
  const {context, pending, requested} = fixture({worker: true});
  const result = vm.runInContext('chrome = new Proxy(chrome, {}); chrome.permissions.request({permissions: ["privacy"]})', context);
  assert.equal(requested(), 1);
  pending.shift()(true);
  assert.equal(await result, true);
  assert.equal(await vm.runInContext('chrome.permissions.contains({permissions: ["privacy"]})', context), true);
});

test('the host excludes content scripts and other extensions', () => {
  const {dispatch, requested} = fixture();
  const message = {__zephiumPermissions: 'request', spec: {permissions: ['privacy'], origins: []}};
  assert.equal(dispatch(message, {id: 'fixture', url: 'https://fixture.test/'}), undefined);
  assert.equal(dispatch(message, {id: 'other', url: origin + 'options.html'}), undefined);
  assert.equal(requested(), 0);
});

test('Bitwarden-style autofill calls require the local privacy grant', async () => {
  const {chrome, pending} = fixture();
  const services = Object.values(chrome.privacy.services);
  await assert.rejects(services[0].set({value: false}), /Privacy setting unavailable/);
  const result = chrome.permissions.request(request);
  pending.shift()(true);
  assert.equal(await result, true);
  for (const setting of services) {
    await setting.set({value: false});
    const details = await new Promise(resolve => setting.get({}, resolve));
    assert.equal(details.value, false);
    assert.equal(details.levelOfControl, 'controlled_by_this_extension');
    await setting.clear({});
    assert.equal((await setting.get({})).value, true);
  }
});
