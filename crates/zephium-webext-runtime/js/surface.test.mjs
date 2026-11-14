import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { fixtureSurface, generatedSurface, extensionContext, flush } from './test/harness.mjs';

const members = JSON.parse(readFileSync(new URL('./test/surface.members.json', import.meta.url)));

test('the small fixture is an exact member selection from generated metadata', () => {
  const namespaces = {};
  for (const [namespace, selected] of Object.entries(members)) {
    const node = { ...generatedSurface.namespaces[namespace] };
    for (const field of ['functions', 'events', 'properties']) {
      const trimmed = {};
      for (const name of selected[field] || []) {
        assert.ok(Object.hasOwn(node[field], name), `${namespace}.${name}`);
        trimmed[name] = node[field][name];
      }
      if (Object.keys(trimmed).length) node[field] = trimmed;
      else delete node[field];
    }
    namespaces[namespace] = node;
  }
  assert.deepEqual(fixtureSurface, { version: 1, namespaces });
});

test('generated surface has the binding format and excludes host-only metadata', () => {
  assert.equal(generatedSurface.version, 1);
  const namespaces = generatedSurface.namespaces;
  assert.equal(Array.isArray(namespaces), false);
  assert.ok(Object.keys(namespaces).length >= 32);
  assert.equal(namespaces.zephium, undefined);
  assert.equal(namespaces.runtime.functions.markListenerRegistrationComplete, undefined);
  for (const namespace of Object.values(namespaces)) {
    for (const key of Object.keys(namespace)) assert.ok(['contexts', 'permissions', 'manifest', 'functions', 'events', 'properties'].includes(key), key);
    for (const fn of Object.values(namespace.functions || {})) {
      for (const key of Object.keys(fn)) assert.ok(['contexts', 'permissions', 'manifest', 'parameters', 'returns', 'callback', 'callbackRequired', 'promise', 'result', 'optionalResult', 'signature', 'setting'].includes(key), key);
      assert.equal(Boolean(fn.parameters?.some((parameter) => parameter.name === 'callback')), false);
    }
  }
  assert.equal(namespaces['storage.local'].properties.QUOTA_BYTES, 10485760);
  assert.equal(namespaces.scripting.properties.ExecutionWorld.MAIN, 'MAIN');
  assert.deepEqual(namespaces.scripting.functions.executeScript.parameters[0].serializeFunctions, ['func', 'function']);
  assert.equal(namespaces.webNavigation.events.onCommitted.filter, 'url');
  assert.deepEqual(namespaces.webRequest.events.onAuthRequired, { blocking: true, filter: 'request' });
});

test('serialization failures and object-backed errors use Chromium invocation text', () => {
  const { chrome } = extensionContext();
  const cycle = {};
  cycle.self = cycle;
  assert.throws(() => chrome.storage.local.set(cycle), {
    message: 'Error in invocation of storage.set(object items, optional function callback): Value is unserializable.',
  });
  assert.throws(() => chrome.scripting.executeScript(), {
    message: 'Error in invocation of scripting.executeScript(scripting.ScriptInjection injection, optional function callback): No matching signature.',
  });
});

test('optional host results resolve to undefined in callback and promise modes', async () => {
  const { chrome, host } = extensionContext();
  host.handlers['alarms.get'] = () => null;
  assert.equal(await chrome.alarms.get('absent'), undefined);
  let called = false;
  chrome.alarms.get('absent', (...args) => {
    called = true;
    assert.equal(args.length, 0);
    assert.equal(chrome.runtime.lastError, undefined);
  });
  await flush();
  assert.equal(called, true);
  host.handlers['storage.local.get'] = () => ({ value: null });
  assert.equal((await chrome.storage.local.get('value')).value, null);
});

test('privacy setting setters perform Chromium typed prevalidation before generic argument parsing', async () => {
  const { chrome, host, run } = extensionContext('background', { permissions: ['privacy'] });
  const password = chrome.privacy.services.passwordSavingEnabled;
  const TypeError = run('TypeError');
  for (const value of [false, null, undefined, {}, { value: 'yes' }, { value: null }, { value: true, unknown: 1 }]) {
    assert.throws(() => password.set(value), (error) => error instanceof TypeError && error.message === 'Invalid invocation');
  }
  assert.throws(() => password.set(), { message: 'Error in invocation of types.ChromeSetting.set(object details, optional function callback): No matching signature.' });
  assert.throws(() => password.get(false), { message: "Error in invocation of types.ChromeSetting.get(object details, optional function callback): Error at parameter 'details': Invalid type: expected object, found boolean." });
  const policy = chrome.privacy.network.webRTCIPHandlingPolicy;
  assert.throws(() => policy.set({ value: 'invalid' }), { message: 'Invalid invocation' });
  assert.throws(() => policy.set({ value: 'default', scope: 'invalid' }), { message: 'Invalid invocation' });
  const autofill = chrome.privacy.services.autofillSettings;
  assert.throws(() => autofill.set({ value: [{ urlPattern: '*', blockedTypes: ['invalid'] }] }), { message: 'Invalid invocation' });
  host.handlers['privacy.services.passwordSavingEnabled.set'] = () => null;
  host.handlers['privacy.network.webRTCIPHandlingPolicy.set'] = () => null;
  host.handlers['privacy.services.autofillSettings.set'] = () => null;
  assert.equal(await password.set({ value: true, scope: 'regular' }), undefined);
  assert.equal(await policy.set({ value: 'default' }), undefined);
  assert.equal(await autofill.set({ value: [{ urlPattern: '*', blockedTypes: ['payments'] }] }), undefined);
  assert.equal(host.callsTo('privacy.services.passwordSavingEnabled.set').length, 1);
});

test('mixed synchronous return and callback methods retain their modes and require custom hooks', () => {
  const spec = generatedSurface.namespaces.contextMenus.functions.create;
  assert.equal(spec.returns, 'sync');
  assert.equal(spec.callback, true);
  assert.equal(spec.promise, false);
  assert.equal(spec.signature, 'contextMenus.CreateProperties createProperties, optional function callback');
  const { chrome } = extensionContext('background', { permissions: ['contextMenus'] });
  assert.equal(typeof chrome.contextMenus.onClicked.addListener, 'function');
  assert.equal(chrome.contextMenus.create, undefined);
});


test('manifest-gated APIs require admitted features independently of API permissions', () => {
  for (const feature of ['action', 'commands']) {
    const absent = extensionContext('background', { permissions: [feature] });
    assert.equal(absent.chrome[feature], undefined);
    const empty = extensionContext('background', { manifest: { manifest_version: 3, [feature]: null } });
    assert.equal(empty.chrome[feature], undefined);
    const admitted = extensionContext('background', { permissions: [], manifest: { manifest_version: 3, [feature]: {} } });
    assert.equal(typeof admitted.chrome[feature], 'object');
    const content = extensionContext('contentScript', { manifest: { manifest_version: 3, [feature]: {} } });
    assert.equal(content.chrome[feature], undefined);
  }
});
