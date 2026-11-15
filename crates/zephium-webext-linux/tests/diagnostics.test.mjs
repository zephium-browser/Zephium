import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
import { createRealm, FakeHost, bootstrap, surface, generatedSurface, flush } from '../../zephium-webext-runtime/js/test/harness.mjs';

const script = fs.readFileSync(new URL('../src/diagnostics.js', import.meta.url), 'utf8');
function fixture(worker = false, globals = {}) {
  const records = [], listeners = new Map();
  const realm = vm.createContext({
    ...globals,
    console: { error() {}, warn() {} }, TextEncoder,
    addEventListener(name, callback) { listeners.set(name, callback); },
  });
  const wrap = vm.runInContext(script, realm);
  const client = {call(name, payload) {
    if (name === 'zephium.diagnostic') { records.push(JSON.parse(payload)[0]); return Promise.resolve(null); }
    return Promise.reject('This API is not supported.');
  }};
  const api = { runtime: {}, storage: {local: {}}, constants: Object.freeze({mode: 'a'}) };
  let lastErrorReads = 0;
  Object.defineProperty(api.runtime, 'lastError', {get() { lastErrorReads++; return undefined; }});
  realm.chrome = realm.browser = api;
  const context = {client, chrome: api};
  const spec = {namespaces: {runtime:{},storage:{},'storage.local':{}}};
  const result = worker ? wrap(() => context, true)(spec, {}) : wrap(() => context, false)('page', spec, {});
  return {realm, records, listeners, context, result, api, reads: () => lastErrorReads};
}

test('undefined reads preserve identity, feature detection, getters and frozen constants', () => {
  const f = fixture();
  assert.equal(f.realm.chrome, f.realm.browser);
  assert.equal(f.result, f.realm.chrome);
  assert.equal(f.realm.chrome.storage, f.realm.chrome.storage);
  assert.equal(f.reads(), 0);
  assert.equal(f.realm.chrome.missing, undefined);
  assert.equal(f.realm.chrome.storage.local.missing, undefined);
  assert.equal('missing' in f.realm.chrome, false);
  assert.equal(f.realm.chrome.constants, f.api.constants);
  assert.equal(f.realm.chrome.runtime.lastError, undefined);
  assert.equal(f.reads(), 1);
  assert.equal(f.realm.chrome.__esModule, undefined);
  assert(!f.records.some(e => e.member === 'chrome.__esModule' || e.member === 'chrome.runtime.lastError'));
  assert(f.records.some(e => e.member === 'chrome.storage.local.missing'));
});

test('worker context stays private and refused arguments record types without values', async () => {
  const f = fixture(true);
  assert.equal(f.result, f.context);
  await assert.rejects(f.context.client.call('identity.getAuthToken', '[{"token":"secret","url":"https://example.com/private"}]'), error => error === 'This API is not supported.');
  const refusal = f.records.find(e => e.kind === 'api-refusal');
  assert.equal(refusal.namespace, 'identity');
  assert.equal(refusal.method, 'getAuthToken');
  assert(!JSON.stringify(refusal).includes('secret'));
  assert(!JSON.stringify(refusal).includes('example.com'));
});

test('errors, rejection, console, CSP and resources retain bounded redacted details', () => {
  const f = fixture();
  f.listeners.get('error')({message: 'failed https://user:pass@host/private?q=x', filename:'chrome-extension://id/file.js?secret', lineno: 12, colno: 3});
  f.listeners.get('error')({target: {src: 'https://host/private?q=secret'}});
  f.listeners.get('unhandledrejection')({reason:'reject ' + 'x'.repeat(1000)});
  f.listeners.get('securitypolicyviolation')({effectiveDirective:'script-src', blockedURI:'https://host/private', sourceFile:'chrome-extension://id/a.js', lineNumber:9});
  f.realm.console.warn('warning');
  f.realm.console.error('error');
  for (const kind of ['uncaught','resource-error','unhandled-rejection','csp','console-warn','console-error']) assert(f.records.some(e => e.kind === kind), kind);
  assert(!JSON.stringify(f.records).includes('secret'));
  assert(!JSON.stringify(f.records).includes('private'));
  assert(f.records.every(e => !e.message || e.message.length <= 4096));
});

test('capture saturation emits an explicit truncation record and stops', () => {
  const f = fixture();
  for (let i = 0; i < 1000; i++) void f.realm.chrome.missing;
  assert.equal(f.records.length, 512);
  assert.equal(f.records.at(-1).kind, 'context-truncated');
});

test('SVG resource errors preserve the href and background API traces contain shapes', async () => {
  const timers = [];
  const f = fixture(true, {setTimeout(callback) { timers.push(callback); }});
  f.listeners.get('error')({target: {localName:'image', href:{baseVal:'chrome-extension://id/image.png'}}});
  assert.equal(f.records.at(-1).resource, 'chrome-extension://id/image.png');
  await assert.rejects(f.context.client.call('storage.local.set', '[{"secret":"do-not-record"}]', false));
  timers[2]();
  const batch = f.records.find(e => e.kind === 'api-trace');
  assert.equal(batch.complete, true);
  assert.equal(batch.records[0].name, 'storage.local.set');
  assert.equal(batch.records[1].phase, 'rejected');
  assert(!JSON.stringify(batch).includes('do-not-record'));
});

test('global replacement snapshots do not read or restore package-owned proxies', () => {
  const timers = [];
  const f = fixture(false, {setTimeout(callback) { timers.push(callback); }});
  timers[0]();
  assert.deepEqual(f.records.at(-1).globalsReplaced, {chrome:false, browser:false});
  let reads = 0;
  const replacement = new Proxy({}, {get() { reads++; return undefined; }});
  f.realm.chrome = f.realm.browser = replacement;
  timers[1]();
  assert.deepEqual(f.records.at(-1).globalsReplaced, {chrome:true, browser:true});
  assert.equal(f.realm.chrome, replacement);
  assert.equal(reads, 0);
});


test('real binding event brands and listener registration survive instrumentation', async () => {
  const realm = createRealm({addEventListener() {}});
  const host = new FakeHost(realm.Z, 'popup');
  host.handlers['zephium.diagnostic'] = () => null;
  const wrap = realm.run(script);
  const api = wrap((entry, spec, boot) => realm.Z.api.create(spec, boot, host.link, {}), false)('page', surface, bootstrap({contextKind:'popup'}));
  const callback = () => {};
  api.runtime.onMessage.addListener(callback);
  assert.equal(api.runtime.onMessage.hasListener(callback), true);
  api.runtime.onMessage.removeListener(callback);
  assert.equal(api.runtime.onMessage.hasListener(callback), false);
  assert.equal(host.callsTo('zephium.addListener').length, 1);
});

test('UTF-8 and JSON escaping cannot silently exceed the report bound', () => {
  const f = fixture();
  f.listeners.get('unhandledrejection')({reason:'界'.repeat(4096)});
  assert.equal(f.records.at(-1).kind, 'record-too-large');
});

test('JavaScriptCore stacks without a message retain both message and frames', () => {
  const f = fixture();
  const error = vm.runInContext("Object.assign(new Error('failed probe'), {stack: 'at@worker.js:1:2'})", f.realm);
  f.listeners.get('unhandledrejection')({reason:error});
  assert.match(f.records.at(-1).message, /failed probe/);
  assert.match(f.records.at(-1).stack, /worker.js:1:2/);
});


test('the native refusal probe reaches a registered unsupported API through the real binding', async () => {
  const realm = createRealm({addEventListener() {}});
  const host = new FakeHost(realm.Z, 'popup');
  const records = [];
  host.handlers['zephium.diagnostic'] = ([record]) => { records.push(record); return null; };
  host.handlers['runtime.getPackageDirectoryEntry'] = () => { throw 'This function is not supported.'; };
  const wrap = realm.run(script);
  const api = wrap((entry, spec, boot) => realm.Z.api.create(spec, boot, host.link, {}), false)('page', generatedSurface, bootstrap({contextKind:'popup'}));
  let error;
  api.runtime.getPackageDirectoryEntry(() => { error = api.runtime.lastError.message; });
  await flush();
  assert.equal(error, 'This function is not supported.');
  assert(records.some(e => e.kind === 'api-refusal' && e.method === 'getPackageDirectoryEntry'));
  assert(!records.some(e => e.member === 'chrome.runtime.lastError'));
});


test('fetch failures retain resource and call site without changing settlement', async () => {
  const rejected = fixture(false, {fetch:()=>Promise.reject('Load failed')});
  await assert.rejects(rejected.realm.fetch('chrome-extension://id/build.info?private'), e=>e==='Load failed');
  const failure = rejected.records.find(e=>e.kind==='fetch-failed');
  assert.equal(failure.resource, 'chrome-extension://id/build.info');
  assert.match(failure.stack, /fetch call/);
  const response = {status:503};
  const http = fixture(false, {fetch:()=>Promise.resolve(response)});
  assert.equal(await http.realm.fetch('https://user:secret@host/path'), response);
  assert(http.records.some(e=>e.kind==='fetch-http-error' && e.status===503));
  assert(!JSON.stringify(http.records).includes('secret'));
});
