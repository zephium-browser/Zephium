import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { FakeHost, bootstrap, createRealm, modulesSource, plain, sources, surface } from './test/harness.mjs';

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
    "case'owner':return Z.workerScope.owner(bootstrap);" +
    "default:throw new TypeError('Unknown binding entry');}})",
  matcher:
    "(function zephiumMatcher(data){'use strict';const Z={__proto__:null};Z.I=(" +
    sources.intrinsics +
    ')(globalThis);Z.content=(' +
    sources.content +
    ')(Z);return Z.content.matches(data);})',
};
const directory = process.env.ZEPHIUM_WEBEXT_BUNDLE_DIR;
const rust = directory
  ? {
      bundle: readFileSync(join(directory, 'bundle.js'), 'utf8'),
      matcher: readFileSync(join(directory, 'matcher.js'), 'utf8'),
    }
  : null;
const bundles = rust || assembled;

test(
  'the tests load the binding exactly as the crate assembles it',
  { skip: rust === null && 'run through cargo test' },
  () => {
    assert.equal(rust.bundle, assembled.bundle);
    assert.equal(rust.matcher, assembled.matcher);
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
