import assert from 'node:assert/strict';
import { test } from 'node:test';

import { EXTENSION_ID, FakeHost, bootstrap, createRealm, extensionContext, flush, plain, surface, generatedSurface } from './test/harness.mjs';

const keys = (object) => Object.keys(object).sort();

test('offscreen readiness follows synchronous load-listener registration', async () => {
  let loaded;
  class Document { get readyState() { return 'loading'; } }
  class EventTarget { addEventListener(type, listener) { assert.equal(type, 'load'); loaded = listener; } }
  const {chrome, host} = extensionContext('offscreen', {offscreenReadyHandshake: true}, {
    globals: {Document, document: new Document(), EventTarget},
  });
  host.handlers['zephium.offscreenReady'] = () => null;
  assert.deepEqual(host.calls, []);
  loaded();
  chrome.runtime.onMessage.addListener(() => {});
  assert.deepEqual(host.calls.map(call => call.name), ['zephium.addListener']);
  loaded();
  await flush();
  assert.deepEqual(host.calls.map(call => call.name), ['zephium.addListener', 'zephium.offscreenReady']);
  assert.deepEqual(host.calls.at(-1).args, []);
});

test('offscreen readiness cannot overtake registrations waiting on internal backpressure', async () => {
  let loaded;
  class Document { get readyState() { return 'loading'; } }
  class EventTarget { addEventListener(_type, listener) { loaded = listener; } }
  const {host, context} = extensionContext('offscreen', {offscreenReadyHandshake: true}, {
    globals: {Document, document: new Document(), EventTarget},
  });
  host.handlers['zephium.addListener'] = () => host.LATER;
  host.handlers['zephium.offscreenReady'] = () => null;
  const pending = Array.from({length: 129}, () => context.client.call('zephium.addListener', '[]', true));
  loaded();
  await flush();
  assert.equal(host.calls.length, 128);
  host.reply(host.calls[0].id, null);
  await flush();
  assert.equal(host.calls.at(-1).name, 'zephium.addListener');
  for (const call of host.calls.slice(1)) host.reply(call.id, null);
  await Promise.all(pending);
  await flush();
  assert.equal(host.calls.at(-1).name, 'zephium.offscreenReady');
});

test('empty loaded offscreen documents signal once, but retired or other contexts do not', async () => {
  class Document { get readyState() { return 'complete'; } }
  const globals = {Document, document: new Document()};
  const live = extensionContext('offscreen', {offscreenReadyHandshake: true}, {globals});
  const retired = extensionContext('offscreen', {offscreenReadyHandshake: true}, {globals});
  retired.context.invalidate();
  const other = extensionContext('popup', {offscreenReadyHandshake: true}, {globals});
  const disabled = extensionContext('offscreen', {}, {globals});
  await flush();
  assert.deepEqual(live.host.calls.map(call => call.name), ['zephium.offscreenReady']);
  for (const fixture of [retired, other, disabled]) assert.deepEqual(fixture.host.calls, []);
});

test('browser is chrome, both globals, with only the namespaces the context may use', () => {
  const background = extensionContext('background');
  assert.equal(background.run('globalThis.browser === globalThis.chrome'), true);
  assert.equal(background.run('globalThis.chrome'), background.chrome);
  for (const namespace of [
    'alarms',
    'dom',
    'extension',
    'i18n',
    'permissions',
    'runtime',
    'scripting',
    'storage',
    'tabs',
  ]) assert.equal(typeof background.chrome[namespace], 'object', namespace);
  assert.equal(background.chrome.webNavigation, undefined, 'webNavigation needs its permission');
  assert.equal(background.chrome.runtime.connectNative, undefined, 'connectNative needs nativeMessaging');
  assert.equal(typeof background.chrome.storage.session.setAccessLevel, 'function');

  const content = extensionContext('contentScript');
  assert.deepEqual(keys(content.chrome), ['dom', 'extension', 'i18n', 'runtime', 'storage']);
  assert.equal(content.chrome.runtime.getPlatformInfo, undefined);
  assert.equal(content.chrome.runtime.onInstalled, undefined);
  assert.equal(typeof content.chrome.runtime.sendMessage, 'function');
  assert.equal(typeof content.chrome.storage.session.setAccessLevel, 'function');
  assert.equal(content.chrome.extension.getViews, undefined);

  const external = extensionContext('externalPage');
  assert.deepEqual(keys(external.chrome), ['runtime']);
  assert.deepEqual(
    keys(external.chrome.runtime),
    ['connect', 'id', 'sendMessage', ...Object.keys(surface.namespaces.runtime.properties)].sort(),
  );
});

test('granted and revoked permissions add and remove namespaces in a live context', async () => {
  const { chrome, host } = extensionContext('background', { permissions: ['storage'] });
  assert.equal(chrome.alarms, undefined);
  host.event('zephium.onPermissionsChanged', [['storage', 'alarms', 'webNavigation', 'nativeMessaging']]);
  assert.equal(typeof chrome.alarms.create, 'function');
  assert.equal(typeof chrome.webNavigation.onCommitted.addListener, 'function');
  assert.equal(typeof chrome.runtime.connectNative, 'function');
  const alarms = chrome.alarms;
  host.event('zephium.onPermissionsChanged', [['storage']]);
  assert.equal(chrome.alarms, undefined);
  assert.equal(chrome.runtime.connectNative, undefined);
  host.event('zephium.onPermissionsChanged', [['storage', 'alarms']]);
  assert.equal(chrome.alarms, alarms, 'the same namespace object returns');
});

test('DNR alternatives retain feedback conjunction and unpacked-only APIs', () => {
  for (const permission of ['declarativeNetRequest', 'declarativeNetRequestWithHostAccess']) {
    const packed = extensionContext('background', { permissions: [permission], unpacked: false });
    assert.equal(typeof packed.chrome.declarativeNetRequest.updateDynamicRules, 'function');
    assert.equal(packed.chrome.declarativeNetRequest.testMatchOutcome, undefined);
    assert.equal(packed.chrome.declarativeNetRequest.onRuleMatchedDebug, undefined);
    const unpacked = extensionContext('background', { permissions: [permission], unpacked: true });
    assert.equal(typeof unpacked.chrome.declarativeNetRequest.testMatchOutcome, 'function');
    assert.equal(unpacked.chrome.declarativeNetRequest.onRuleMatchedDebug, undefined);
    unpacked.host.event('zephium.onPermissionsChanged', [[permission, 'declarativeNetRequestFeedback']]);
    assert.equal(typeof unpacked.chrome.declarativeNetRequest.onRuleMatchedDebug.addListener, 'function');
    unpacked.host.event('zephium.onPermissionsChanged', [[permission]]);
    assert.equal(unpacked.chrome.declarativeNetRequest.onRuleMatchedDebug, undefined);
    const packedFeedback = extensionContext('background', { permissions: [permission, 'declarativeNetRequestFeedback'], unpacked: false });
    assert.equal(packedFeedback.chrome.declarativeNetRequest.onRuleMatchedDebug, undefined);
  }
  const feedbackOnly = extensionContext('background', { permissions: ['declarativeNetRequestFeedback'], unpacked: true });
  assert.equal(feedbackOnly.chrome.declarativeNetRequest, undefined);
});

test("a promise is returned without a callback and rejects with Chrome's Error", async () => {
  const { chrome, host, run } = extensionContext();
  host.handlers['tabs.get'] = ([tabId]) => {
    if (tabId === 9) throw { error: 'No tab with id: 9.' };
    return { id: tabId };
  };
  const promise = chrome.tabs.get(4);
  assert.equal(Object.getPrototypeOf(promise), run('Promise.prototype'));
  assert.deepEqual(plain(await promise), { id: 4 });
  await assert.rejects(chrome.tabs.get(9), (error) => {
    assert.equal(error instanceof run('Error'), true);
    assert.equal(error.message, 'No tab with id: 9.');
    return true;
  });
  assert.equal(chrome.runtime.lastError, undefined);
  assert.deepEqual(
    host.callsTo('tabs.get').map((call) => call.args),
    [[4], [9]],
  );
});

test('runtime.lastError clears by the next task and warns when unchecked', async () => {
  const { chrome, host, logs } = extensionContext();
  host.handlers['tabs.get'] = () => {
    throw { error: 'No tab with id: 1.' };
  };
  const seen = [];
  assert.equal(
    chrome.tabs.get(1, (...args) => {
      seen.push({
        args: args.length,
        message: chrome.runtime.lastError.message,
        same: chrome.extension.lastError === chrome.runtime.lastError,
      });
    }),
    undefined,
  );
  chrome.tabs.get(1, () => {});
  await flush();
  assert.deepEqual(seen, [{ args: 0, message: 'No tab with id: 1.', same: true }]);
  assert.equal(chrome.runtime.lastError, undefined);
  assert.deepEqual(logs.error, ['Unchecked runtime.lastError: No tab with id: 1.']);
});

test('reading lastError in callback microtasks suppresses the unchecked warning', async () => {
  const { chrome, host, logs } = extensionContext();
  host.handlers['tabs.get'] = () => {
    throw { error: 'No tab with id: 1.' };
  };
  const seen = [];
  await new Promise((resolve) => {
    chrome.tabs.get(1, () => {
      queueMicrotask(() => {
        seen.push(chrome.runtime.lastError.message);
      });
      Promise.resolve().then(() => seen.push(chrome.extension.lastError.message));
      setTimeout(() => {
        seen.push(chrome.runtime.lastError);
        resolve();
      }, 0);
    });
  });
  assert.deepEqual(seen, ['No tab with id: 1.', 'No tab with id: 1.', undefined]);
  assert.deepEqual(logs.error, []);
});

test('lastError clears before tasks queued ahead of the callback', async () => {
  const { Z, logs } = extensionContext();
  const errors = Z.errors.createLastError();
  let seen;
  const nextTask = new Promise((resolve) => setTimeout(() => {
    seen = errors.read();
    resolve();
  }, 0));
  errors.run('queued failure', () => {});
  await nextTask;
  assert.equal(seen, undefined);
  assert.deepEqual(logs.error, ['Unchecked runtime.lastError: queued failure']);
});

test('task cleanup preserves a newer error and reports each unread failure once', () => {
  const tasks = [];
  const { Z, logs } = createRealm({ queueMicrotask: (fn) => tasks.push(fn) });
  const errors = Z.errors.createLastError();
  errors.run('first', () => {});
  errors.run('second', () => {});
  tasks.shift()();
  assert.equal(errors.read().message, 'second');
  tasks.shift()();
  assert.equal(errors.read(), undefined);
  assert.deepEqual(logs.error, ['Unchecked runtime.lastError: first']);
});

test('nested lastError scopes restore the outer callback after success and failure', async () => {
  const { Z, logs } = extensionContext();
  const errors = Z.errors.createLastError();
  errors.run('outer', () => {
    const outer = errors.read();
    errors.run(null, () => assert.equal(errors.read(), undefined));
    assert.equal(errors.read(), outer);
    assert.throws(() => errors.run('inner', () => {
      assert.equal(errors.read().message, 'inner');
      throw new Error('callback failed');
    }), /callback failed/);
    assert.equal(errors.read(), outer);
  });
  assert.equal(errors.read().message, 'outer');
  await flush();
  assert.equal(errors.read(), undefined);
  assert.deepEqual(logs.error, []);
});

test('nested callbacks restore the outer lastError and successes see none', async () => {
  const { chrome, host } = extensionContext();
  host.handlers['tabs.get'] = ([id]) => {
    if (id === 1) throw { error: 'outer' };
    return { id };
  };
  const order = [];
  await new Promise((resolve) => {
    chrome.tabs.get(1, () => {
      order.push(chrome.runtime.lastError.message);
      chrome.tabs.get(2, (tab) => {
        order.push(chrome.runtime.lastError, tab.id);
        resolve();
      });
      order.push(chrome.runtime.lastError.message);
    });
  });
  assert.deepEqual(order, ['outer', 'outer', undefined, 2]);
});

test('a throwing callback is reported and does not disturb the binding', async () => {
  const { chrome, host, logs } = extensionContext();
  host.handlers['tabs.query'] = () => [];
  chrome.tabs.query({}, () => {
    throw new Error('boom');
  });
  await flush();
  assert.equal(logs.error.length, 1);
  assert.match(logs.error[0], /^Error handling response: Error: boom/);
  assert.deepEqual(plain(await chrome.tabs.query({})), []);
});

test('void methods call back with no arguments and resolve to undefined', async () => {
  const { chrome, host } = extensionContext();
  host.handlers['storage.local.set'] = () => null;
  host.handlers['alarms.create'] = () => null;
  let args;
  chrome.storage.local.set({ a: 1 }, (...received) => (args = received));
  await flush();
  assert.deepEqual(args, []);
  assert.equal(await chrome.alarms.create({ periodInMinutes: 1 }), undefined);
  assert.deepEqual(host.callsTo('alarms.create')[0].args, [null, { periodInMinutes: 1 }]);
});

test('arguments resolve like Chrome: skipped optionals, callbacks first, and invocation errors', async () => {
  const { chrome, host, run } = extensionContext();
  host.handlers['storage.local.get'] = ([keys]) => ({ keys });
  host.handlers['tabs.update'] = (args) => args;
  let viaCallback;
  chrome.storage.local.get((items) => (viaCallback = items));
  await flush();
  assert.deepEqual(plain(viaCallback), { keys: null });
  await chrome.storage.local.get(null);
  await chrome.storage.local.get(['a', 'b']);
  await chrome.storage.local.get({ a: 1 });
  assert.deepEqual(
    host.callsTo('storage.local.get').map((call) => call.args),
    [[null], [null], [['a', 'b']], [{ a: 1 }]],
  );
  assert.deepEqual(plain(await chrome.tabs.update({ url: 'https://example.com/' })), [
    null,
    { url: 'https://example.com/' },
  ]);
  assert.deepEqual(plain(await chrome.tabs.update(3, { muted: true })), [3, { muted: true }]);

  const TypeError = run('TypeError');
  const failure = (fn, message) =>
    assert.throws(fn, (error) => error instanceof TypeError && error.message === message);
  failure(
    () => chrome.tabs.get('3'),
    "Error in invocation of tabs.get(integer tabId, optional function callback): Error at parameter 'tabId': Invalid type: expected integer, found string.",
  );
  failure(
    () => chrome.tabs.get(),
    'Error in invocation of tabs.get(integer tabId, optional function callback): No matching signature.',
  );
  failure(
    () => chrome.tabs.get(1.5),
    "Error in invocation of tabs.get(integer tabId, optional function callback): Error at parameter 'tabId': Invalid type: expected integer, found number.",
  );
  failure(
    () => chrome.tabs.get(1, 2),
    'Error in invocation of tabs.get(integer tabId, optional function callback): No matching signature.',
  );
  failure(
    () => chrome.storage.local.get(1),
    'Error in invocation of storage.get(optional [string|array|object] keys, optional function callback): No matching signature.',
  );
  failure(() => chrome.runtime.getURL(), 'Error in invocation of runtime.getURL(string path): No matching signature.');
  failure(
    () => chrome.tabs.remove({}),
    "Error in invocation of tabs.remove([integer|array] tabIds, optional function callback): Error at parameter 'tabIds': Value did not match any choice.",
  );
  assert.equal(host.callsTo('tabs.get').length, 0, 'invalid calls never reach the host');
});

test('methods without a callback parameter are fire-and-forget', async () => {
  const { chrome, host, logs, run } = extensionContext();
  host.handlers['runtime.reload'] = () => {
    throw { error: 'reload refused' };
  };
  assert.equal(chrome.runtime.reload(), undefined);
  assert.throws(() => chrome.runtime.reload(() => {}), run('TypeError'));
  await flush();
  assert.deepEqual(logs.error, ['Unchecked runtime.lastError: reload refused']);
});

test('function-valued injection fields travel as source text', async () => {
  const { chrome, host } = extensionContext();
  host.handlers['scripting.executeScript'] = () => [{ frameId: 0, result: 2 }];
  const results = await chrome.scripting.executeScript({ target: { tabId: 1 }, func: (a) => a + 1, args: [1] });
  assert.deepEqual(plain(results), [{ frameId: 0, result: 2 }]);
  assert.deepEqual(host.callsTo('scripting.executeScript')[0].args, [
    { target: { tabId: 1 }, func: '(a) => a + 1', args: [1] },
  ]);
  assert.equal(chrome.scripting.ExecutionWorld.MAIN, 'MAIN');
  assert.equal(Object.isFrozen(chrome.scripting.ExecutionWorld), true);
});

test('synchronous members come from the bootstrap', () => {
  const { chrome } = extensionContext('popup', { uiLanguage: 'ar-EG', incognito: true });
  const base = `chrome-extension://${EXTENSION_ID}/`;
  assert.equal(chrome.runtime.id, EXTENSION_ID);
  assert.equal(chrome.runtime.getURL(''), base);
  assert.equal(chrome.runtime.getURL('/popup.html'), `${base}popup.html`);
  assert.equal(chrome.runtime.getURL('a/../b c.html?x=1#y'), `${base}b%20c.html?x=1#y`);
  assert.equal(chrome.runtime.getURL('a\\b'), `${base}a/b`);
  const manifest = chrome.runtime.getManifest();
  manifest.name = 'changed';
  assert.equal(chrome.runtime.getManifest().name, 'Fixture');
  assert.equal(chrome.extension.inIncognitoContext, true);
  assert.equal(chrome.i18n.getUILanguage(), 'ar-EG');
  assert.equal(chrome.i18n.getMessage('@@bidi_dir'), 'rtl');
  assert.equal(chrome.i18n.getMessage('@@ui_locale'), 'ar_EG');
  assert.equal(chrome.i18n.getMessage('@@extension_id'), EXTENSION_ID);
  assert.deepEqual(plain(chrome.extension.getViews()), []);
});

test("i18n.getMessage follows Chrome's substitution rules", () => {
  const { chrome } = extensionContext();
  assert.equal(chrome.i18n.getMessage('GREETING', ['Ann', 3]), 'Hello Ann, you have 3 messages');
  assert.equal(chrome.i18n.getMessage('greeting', 'Bob'), 'Hello Bob, you have  messages');
  assert.equal(chrome.i18n.getMessage('greeting'), 'Hello , you have  messages');
  assert.equal(chrome.i18n.getMessage('dollars'), 'Cost: $5 and $$$ and  end');
  assert.equal(chrome.i18n.getMessage('markup', undefined, { escapeLt: true }), 'a &lt;b> c');
  assert.equal(chrome.i18n.getMessage('missing'), '');
  assert.equal(chrome.i18n.getMessage('greeting', Array(10).fill('x')), undefined);
});

test('i18n consumes the resolved Rust catalog and expands named placeholders once', () => {
  const { chrome } = extensionContext('background', {
    messages: {
      welcome: {
        message: 'Hello, $PERSON$! Price: $$ $AMOUNT$.',
        placeholders: { person: { content: '$1' }, amount: { content: '$2' } },
      },
      nested: { message: '[$WRAPPED$]', placeholders: { wrapped: { content: 'prefix:$1:suffix' } } },
      dollars: { message: 'One $$, two $$$$, substitution $1.' },
      markup: { message: '<p>$BODY$</p>', placeholders: { body: { content: '<b>$1</b>' } } },
      malformed: { placeholders: { unused: { content: '$1' } } },
    },
  });
  assert.equal(chrome.i18n.getMessage('welcome', ['Ada', '12']), 'Hello, Ada! Price: $ 12.');
  assert.equal(chrome.i18n.getMessage('WELCOME', 'Ada'), 'Hello, Ada! Price: $ .');
  assert.equal(chrome.i18n.getMessage('welcome'), 'Hello, ! Price: $ .');
  assert.equal(chrome.i18n.getMessage('nested', 'value'), '[prefix:value:suffix]');
  assert.equal(chrome.i18n.getMessage('dollars', '$literal'), 'One $, two $$$, substitution $literal.');
  assert.equal(chrome.i18n.getMessage('nested', '$WRAPPED$'), '[prefix:$WRAPPED$:suffix]');
  assert.equal(
    chrome.i18n.getMessage('markup', '<i>$1</i>', { escapeLt: true }),
    '&lt;p>&lt;b><i>$1</i>&lt;/b>&lt;/p>',
  );
  assert.equal(chrome.i18n.getMessage('malformed'), '');
});

test('an invalidated context refuses calls, settles pending ones and forgets its id', async () => {
  const { chrome, host, run } = extensionContext('contentScript');
  host.handlers['storage.local.get'] = () => host.LATER;
  const pending = chrome.storage.local.get('a');
  host.event('zephium.onInvalidated', []);
  await assert.rejects(pending, (error) => error.message === 'Extension context invalidated.');
  assert.throws(
    () => chrome.storage.local.get('a'),
    (error) => error instanceof run('Error') && error.message === 'Extension context invalidated.',
  );
  assert.throws(() => chrome.runtime.sendMessage('x'), /Extension context invalidated/);
  assert.equal(chrome.runtime.id, undefined);
  assert.equal(chrome.runtime.getURL === undefined, false);
});

test('a context without a bridge gets the API but every call reports invalidation', async () => {
  const { run, Z } = extensionContext();
  const context = Z.api.create(
    (await import('./test/harness.mjs')).surface,
    (await import('./test/harness.mjs')).bootstrap({ contextKind: 'options' }),
    null,
    { defineGlobals: false },
  );
  await assert.rejects(context.chrome.tabs.query({}), (error) => error.message === 'Extension context invalidated.');
  assert.equal(run('typeof chrome'), 'object');
});


test('menu creation returns an id while native errors settle only through lastError', async () => {
  const {chrome, host} = extensionContext('background', {permissions:['contextMenus']});
  host.handlers['contextMenus.create'] = () => { throw 'This function is not supported.'; };
  let seen;
  const id = chrome.contextMenus.create({id:'item', title:'Item'}, () => { seen = chrome.runtime.lastError.message; });
  assert.equal(id, 'item');
  assert.equal(seen, undefined);
  await flush();
  assert.equal(seen, 'This function is not supported.');
  await flush();
  assert.equal(chrome.runtime.lastError, undefined);
  assert.throws(() => chrome.contextMenus.create('invalid'), /invocation|signature|parameter/i);
});

test('adapter compatibility identity is confined to extension realms', () => {
  for (const kind of ['background', 'popup', 'options', 'extensionTab', 'offscreen', 'extensionFrame', 'contentScript', 'userScript', 'externalPage']) {
    const {run} = extensionContext(kind, {userAgentSuffix:' Chrome/157.0.0.0 ZephiumWebExtensions/1'}, {globals:{navigator:{userAgent:'AppleWebKit/605 Safari/605',appVersion:'original-version',vendor:'original-vendor',platform:'original-platform',userAgentData:undefined}, identitySentinel:42}});
    const ua = run('navigator.userAgent');
    assert(ua.includes('AppleWebKit/605'));
    assert.equal(run('navigator.appVersion'), 'original-version');
    assert.equal(run('navigator.vendor'), 'original-vendor');
    assert.equal(run('navigator.platform'), 'original-platform');
    assert.equal(run('navigator.userAgentData'), undefined);
    assert.equal(run('identitySentinel'), 42);
    assert.equal(ua.includes(' Chrome/'), kind !== 'externalPage');
  }
  const ordinary = extensionContext('popup', {}, {globals:{navigator:{userAgent:'original'}}});
  assert.equal(ordinary.run('navigator.userAgent'), 'original');
});

test('menu inline callbacks are refused instead of disappearing during serialization', async () => {
  const {chrome, host} = extensionContext('background', {permissions:['contextMenus']});
  let calls = 0;
  host.handlers['contextMenus.create'] = () => { calls++; };
  host.handlers['contextMenus.update'] = () => { calls++; };
  let error;
  chrome.contextMenus.create({id:'inline', title:'Item', onclick() {}}, () => { error = chrome.runtime.lastError.message; });
  await flush();
  assert.equal(error, 'This function is not supported.');
  await assert.rejects(chrome.contextMenus.update('inline', {onclick() {}}), /not supported/);
  assert.equal(calls, 0);
});

test('fresh disabled user scripts namespace is absent', () => {
  const { chrome, host } = extensionContext('background', { permissions: ['userScripts'], userScriptsEnabled: false });
  assert.equal(chrome.userScripts, undefined);
  assert.equal(Object.hasOwn(chrome, 'userScripts'), false);
  assert.equal(host.calls.length, 0);
});

test('held user scripts throw after revocation before validation or transport', () => {
  const { Z } = createRealm();
  const host = new FakeHost(Z);
  const data = bootstrap({permissions: ['userScripts'], userScriptsEnabled: true});
  const context = Z.api.create(surface, data, host.link);
  const held = context.chrome.userScripts;
  assert.equal(typeof held.getScripts, 'function');
  data.userScriptsEnabled = false;
  context.refresh();
  assert.equal(context.chrome.userScripts, undefined);
  let callback = false;
  for (const method of ['getScripts', 'register', 'configureWorld', 'execute', 'getWorldConfigurations', 'resetWorldConfiguration', 'unregister', 'update']) {
    assert.throws(() => held[method](Symbol(), () => { callback = true; }),
      { message: `'userScripts.${method}' is not available in this context.` });
  }
  assert.equal(callback, false);
  assert.equal(host.calls.length, 0);
});


test('action icons copy prototype ImageData getters and typed pixels before transport', async () => {
  const { chrome, host } = extensionContext('background', { manifest: { manifest_version: 3, action: {} } });
  host.handlers['action.setIcon'] = () => null;
  const pixels = new Uint8ClampedArray([7, 12, 200, 255]);
  const image = Object.create({get width(){return 1},get height(){return 1},get data(){return pixels}});
  const pending = chrome.action.setIcon({imageData:image});
  assert.deepEqual(host.calls.at(-1).args, [{imageData:{1:{width:1,height:1,data:[7,12,200,255]}}}]);
  await pending;
  assert.throws(()=>chrome.action.setIcon({imageData:{width:1,height:1,data:[]}}), /ImageData data length does not match dimensions/);
  assert.throws(()=>chrome.action.setIcon({imageData:{width:257,height:1,get data(){throw Error('must not allocate')}}}), /Icon invalid/);
  assert.throws(()=>chrome.action.setIcon({imageData:{}}), /must contain an ImageData/);
});


test('identity redirect URL is local, authenticated and does not normalize caller paths', () => {
  const { chrome, host } = extensionContext('background', {permissions:['identity']});
  const prefix = `https://${EXTENSION_ID}.chromiumapp.org`;
  assert.equal(chrome.identity.getRedirectURL(), prefix + '/');
  assert.equal(chrome.identity.getRedirectURL(''), prefix + '/');
  assert.equal(chrome.identity.getRedirectURL('../a?b#c'), prefix + '/../a?b#c');
  assert.equal(chrome.identity.getRedirectURL('//other.test'), prefix + '//other.test');
  assert.throws(() => chrome.identity.getRedirectURL(7), /signature|parameter|invocation/);
  assert.equal(host.calls.length, 0);
});


test('unavailable synchronous host APIs validate then refuse without fake cancellation', () => {
  const {chrome,host}=extensionContext('background',{permissions:['desktopCapture'],unavailableSyncApis:true});
  let called=false;
  assert.equal(typeof chrome.desktopCapture.chooseDesktopMedia,'function');
  assert.throws(()=>chrome.desktopCapture.chooseDesktopMedia(['screen'],()=>{called=true}), /This function is not supported/);
  assert.throws(()=>chrome.desktopCapture.chooseDesktopMedia(7), /signature|parameter|invocation/);
  assert.equal(called,false);
  assert.equal(host.calls.length,0);
});


test('unavailable declarative constructors are defined, gated and never inspect arguments', () => {
  const names = ['PageStateMatcher', 'ShowAction', 'ShowPageAction', 'SetIcon', 'RequestContentScript'];
  const context = extensionContext('background', {permissions:['declarativeContent'], unavailableSyncApis:true});
  const hostile = new Proxy({}, {ownKeys(){throw Error('argument inspected')}});
  for (const name of names) {
    const constructor = context.chrome.declarativeContent[name];
    assert.equal(typeof constructor, 'function');
    assert.throws(() => new constructor(hostile), {message:'This function is not supported.'});
    assert.throws(() => constructor(hostile), {message:'This function is not supported.'});
  }
  assert.equal(context.host.calls.length, 0);
  const held = context.chrome.declarativeContent.PageStateMatcher;
  context.host.event('zephium.onInvalidated', []);
  assert.throws(() => new held(), {message:'Extension context invalidated.'});
  for (const kind of ['contentScript', 'offscreen', 'externalPage']) {
    assert.equal(extensionContext(kind, {permissions:['declarativeContent'], unavailableSyncApis:true}).chrome.declarativeContent, undefined);
  }
  assert.equal(extensionContext('background', {permissions:[], unavailableSyncApis:true}).chrome.declarativeContent, undefined);
});


test('cookies get preserves null while an empty set result is absent', async () => {
  const realm = createRealm();
  const host = new FakeHost(realm.Z, 'background');
  const {chrome} = realm.Z.api.create(generatedSurface, bootstrap({contextKind:'background',permissions:['cookies']}), host.link, {});
  host.handlers['cookies.get'] = () => null;
  host.handlers['cookies.set'] = () => null;
  assert.equal(await chrome.cookies.get({url:'https://example.com/',name:'missing'}), null);
  const args = await new Promise(resolve => chrome.cookies.get({url:'https://example.com/',name:'missing'}, (...args)=>resolve(args)));
  assert.deepEqual(plain(args), [null]);
  assert.equal(await chrome.cookies.set({url:'https://example.com/',name:'gone',expirationDate:1}), undefined);
  assert.deepEqual(plain(await new Promise(resolve => chrome.cookies.set({url:'https://example.com/',name:'gone',expirationDate:1}, (...args)=>resolve(args)))), []);
});

test('dynamic resource locators match canonical paths without changing extension identity', () => {
  const alias = 'b'.repeat(32);
  const {chrome} = extensionContext('popup', {dynamicResources:{alias,patterns:['/public/*','/café.txt']}});
  for (const path of ['public/a.js', '/public/a.js?x=1#fragment','folder/../public/a.js','public/a/../b.js','café.txt','caf%c3%a9.txt','public/%64ata.txt','public','//remote.test/public/a.js']) {
    assert.equal(new URL(chrome.runtime.getURL(path)).hostname, alias);
  }
  for (const path of ['private.txt','public/../private.txt','https://remote.test/public/a.js','public/%2Fdata.txt','public/%00data.txt','public/\u202ex.txt','public/%E2%80%AEx.txt','public/%25E2%2580%25AEx.txt']) {
    assert.equal(new URL(chrome.runtime.getURL(path)).hostname, EXTENSION_ID);
  }
  assert.equal(chrome.runtime.id, EXTENSION_ID);
  assert.equal(chrome.runtime.getURL('///icons/../popup.html'), `chrome-extension://${EXTENSION_ID}///popup.html`);
});

test('browser user-script gate event removes held methods before argument validation', () => {
  const {chrome,host} = extensionContext('background',{permissions:['userScripts'],userScriptsEnabled:true});
  const held=chrome.userScripts;
  host.event('zephium.onUserScriptsChanged',[false]);
  assert.equal(chrome.userScripts,undefined);
  assert.throws(()=>held.getScripts(Symbol()),{message:"'userScripts.getScripts' is not available in this context."});
});

test('user-world lastError exists only during its error callback', async () => {
  const {Z}=createRealm();
  const state=Z.errors.createLastError(true);
  const runtime={};
  state.install(runtime);
  assert.equal(Object.hasOwn(runtime,'lastError'),false);
  state.run('refused',()=>{
    assert.equal(Object.hasOwn(runtime,'lastError'),true);
    assert.equal(runtime.lastError.message,'refused');
  });
  assert.equal(Object.hasOwn(runtime,'lastError'),false);
  await flush();
  assert.equal(runtime.lastError,undefined);
});
