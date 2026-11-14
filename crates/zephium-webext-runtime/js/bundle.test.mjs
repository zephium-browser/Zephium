import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { FakeHost, bootstrap, createRealm, generatedSurface, modulesSource, ownerModulesSource, plain, sources, surface, workerModulesSource } from './test/harness.mjs';

// What `src/js.rs` concatenates; the Rust test hands over its own copies.
const assembled = {
  bundle:
    "(function zephiumWebExtension(entry,surface,bootstrap){'use strict';" +
    modulesSource() +
    'switch(entry){' +
    "case'page':return Z.api.page(surface,bootstrap);" +
    "case'content':return Z.content.boot(surface,bootstrap);" +
    "case'native-content':if(!bootstrap||!['contentScript','userScript'].includes(bootstrap.contextKind))throw new TypeError('Invalid native content context');return Z.api.page(surface,bootstrap);" +
    "case'worker':return Z.workerScope.worker(surface,bootstrap);" +
    "case'owner':return Z.ownerRelay.owner(bootstrap);" +
    "default:throw new TypeError('Unknown binding entry');}})",
  matcher:
    "(function zephiumMatcher(data){'use strict';const Z={__proto__:null};Z.I=(" +
    sources.intrinsics +
    ')(globalThis);Z.content=(' +
    sources.content +
    ')(Z);return Z.content.matches(data);})',
  owner:
    "(function zephiumWorkerOwner(bootstrap){'use strict';" +
    ownerModulesSource() +
    'return Z.ownerRelay.owner(bootstrap);})',
  worker:
    "(function zephiumWorker(surface,bootstrap){'use strict';" +
    workerModulesSource() +
    'return Z.workerScope.worker(surface,bootstrap);})',
};
const directory = process.env.ZEPHIUM_WEBEXT_BUNDLE_DIR;
const rust = directory
  ? {
      bundle: readFileSync(join(directory, 'bundle.js'), 'utf8'),
      matcher: readFileSync(join(directory, 'matcher.js'), 'utf8'),
      owner: readFileSync(join(directory, 'owner.js'), 'utf8'),
      worker: readFileSync(join(directory, 'worker.js'), 'utf8'),
    }
  : null;
const bundles = rust || assembled;

test('manifest projections preserve the API through optional grants and revocations', { skip: directory === undefined && 'run through cargo test' }, () => {
  const projected = JSON.parse(readFileSync(join(directory, 'projected-surfaces.json'), 'utf8'));
  const shape = (chrome) => Object.fromEntries(Object.keys(generatedSurface.namespaces).sort().map((name) => {
    let value = chrome;
    for (const segment of name.split('.')) value = value && value[segment];
    return [name, value === undefined ? null : Object.keys(value).sort()];
  }));
  for (const { manifest, surface: trimmed } of projected) {
    const full = createRealm();
    const small = createRealm();
    const fullHost = new FakeHost(full.Z);
    const smallHost = new FakeHost(small.Z);
    const initial = manifest.permissions || [];
    const boot = bootstrap({ contextKind: 'background', manifest, permissions: initial });
    const fullApi = full.Z.api.create(generatedSurface, boot, fullHost.link);
    const smallApi = small.Z.api.create(trimmed, boot, smallHost.link);
    const granted = [...initial, ...(manifest.optional_permissions || [])];
    for (const permissions of [initial, granted, initial]) {
      fullHost.event('zephium.onPermissionsChanged', [permissions]);
      smallHost.event('zephium.onPermissionsChanged', [permissions]);
      assert.deepEqual(shape(smallApi.chrome), shape(fullApi.chrome));
    }
    assert.ok(JSON.stringify(trimmed).length < JSON.stringify(generatedSurface).length);
  }
});

test(
  'the tests load the binding exactly as the crate assembles it',
  { skip: rust === null && 'run through cargo test' },
  () => {
    assert.equal(rust.bundle, assembled.bundle);
    assert.equal(rust.matcher, assembled.matcher);
    assert.equal(rust.owner, assembled.owner);
    assert.equal(rust.worker, assembled.worker);
  },
);

function nativeRealm(globals = {}) {
  const realm = createRealm(globals);
  const host = new FakeHost(realm.Z, 'popup');
  const global = realm.run('globalThis');
  global.__zephiumPost = (bytes) => host.receive(bytes);
  global.__zephiumRegisterReceiver = (receiver) => {
    host.deliver = receiver;
    return true;
  };
  const entry = realm.run(bundles.bundle);
  return { ...realm, host, entry, global };
}

test('the page entry builds chrome over the native bridge and removes its globals', async () => {
  const { entry, host, run } = nativeRealm();
  host.handlers['tabs.query'] = () => [{ id: 3 }];
  const chrome = entry('page', surface, bootstrap({ contextKind: 'popup' }));
  assert.equal(run('typeof __zephiumPost + typeof __zephiumRegisterReceiver'), 'undefinedundefined');
  assert.equal(run('chrome === browser'), true);
  assert.equal(run('chrome'), chrome);
  assert.deepEqual(plain(await chrome.tabs.query({})), [{ id: 3 }]);
  assert.throws(() => entry('nonsense'), /Unknown binding entry/);
});

test('the owner entry has only relay dependencies and leaves the API absent', () => {
  const { run, global } = nativeRealm();
  // A missing native link is a clean refusal before worker creation.
  delete global.__zephiumPost;
  delete global.__zephiumRegisterReceiver;
  assert.equal(run(bundles.owner)({ workerURL: 'about:blank' }), null);
  assert.equal(run('typeof chrome'), 'undefined');
  assert.equal(run('typeof browser'), 'undefined');
  assert.equal(bundles.owner.includes('function api('), false);
  assert.equal(bundles.owner.includes('function workerScope('), false);
  assert.equal(bundles.worker.includes('function ownerRelay('), false);
  assert.equal(bundles.worker.includes('function content('), false);
});

test('the standalone owner relays calls and keeps diagnostic replies without API modules', async () => {
  const started = [];
  class Worker extends EventTarget {
    constructor(url, options) {
      super();
      started.push([url, options.type]);
    }
    postMessage(marker, ports) {
      this.marker = marker;
      this.port = ports[0];
    }
  }
  const { run, host, Z } = nativeRealm({ Worker });
  host.handlers['zephium.workerError'] = () => null;
  host.handlers['tabs.query'] = () => [];
  const owner = run(bundles.owner)({ workerURL: 'chrome-extension://fixture/sw.js', type: 'module' });
  const port = owner.worker.port;
  try {
    assert.deepEqual(started, [['chrome-extension://fixture/sw.js', 'module']]);
    assert.equal(owner.worker.marker, 'zephium.worker.port.v1');
    assert.equal(run('typeof chrome'), 'undefined');
    const replies = [];
    let received;
    const response = new Promise((resolve) => (received = resolve));
    port.onmessage = (event) => {
      const frame = Z.transport.decode(new Uint8Array(event.data), Z.transport.TO_CONTEXT, 1 << 20).frame;
      replies.push(frame.id);
      received(frame);
    };
    owner.reportError({ message: 'worker failed' });
    const report = host.callsTo('zephium.workerError')[0];
    assert.ok(report.id >= 0x80000000);
    const call = Z.transport.encode({
      kind: 'call', id: 1, name: 'tabs.query', args: new TextEncoder().encode('[{}]'),
    }, Z.transport.FROM_CONTEXT, 1 << 20);
    port.postMessage(call.buffer);
    const reply = await response;
    assert.equal(reply.id, 1);
    assert.equal(reply.ok, true);
    assert.deepEqual(replies, [1], 'owner diagnostic replies stay outside the worker');
  } finally {
    port.close();
  }
});

test('the content entry boots a guard and the matcher entry decides alone', () => {
  const { entry, run, global } = nativeRealm({
    location: { href: 'https://example.com/x' },
    origin: 'https://example.com',
  });
  global.top = global;
  entry('content', surface, bootstrap({ contextKind: 'contentScript' }));
  assert.equal(run('typeof chrome'), 'undefined');
  assert.equal(run("globalThis[Symbol.for('zephium.content.v1')].run({ matches: ['https://example.com/*'] })"), true);
  assert.equal(run('typeof chrome.runtime.sendMessage'), 'function');
  const matcher = run(bundles.matcher);
  assert.equal(matcher({ matches: ['https://example.com/*'] }), true);
  assert.equal(matcher({ matches: ['https://example.org/*'] }), false);
});

test('native content creates the API before separately evaluated strict files', () => {
  const { entry, run } = nativeRealm();
  entry('native-content', surface, bootstrap({ contextKind: 'contentScript' }));
  assert.equal(run('typeof chrome.runtime.sendMessage'), 'function');
  assert.equal(run('typeof __zephiumPost'), 'undefined');
  run('"use strict"; let sharedNativeLexical = 41;');
  assert.equal(run('"use strict"; sharedNativeLexical + 1'), 42);
  assert.throws(() => run('"use strict"; undeclaredNativeValue = 1'), /not defined/);
  for (const contextKind of ['popup', 'background', 'externalPage']) {
    assert.throws(() => nativeRealm().entry('native-content', surface, bootstrap({ contextKind })), /Invalid native content context/);
  }
});
