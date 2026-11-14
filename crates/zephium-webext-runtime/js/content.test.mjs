import assert from 'node:assert/strict';
import { test } from 'node:test';

import { FakeHost, bootstrap, createRealm, flush, plain, surface } from './test/harness.mjs';

const frame = (href, extra = {}) => ({
  href,
  origin:
    extra.origin ??
    (() => {
      try {
        const origin = new URL(href).origin;
        return origin;
      } catch {
        return 'null';
      }
    })(),
  top: extra.top ?? true,
  creator: () => extra.creator ?? null,
});

test("the guard decides exactly as Chrome's matcher", () => {
  const { Z } = createRealm();
  const decide = (data, href, extra) => Z.content.matches(data, frame(href, extra));
  const youtube = { matches: ['*://*.youtube.com/*', 'https://www.youtube-nocookie.com/embed/*'], allFrames: true };
  const cases = [
    [youtube, 'https://www.youtube.com/watch?v=1', true],
    [youtube, 'http://youtube.com/', true],
    [youtube, 'https://notyoutube.com/', false],
    [youtube, 'https://youtube.com.evil.org/', false],
    [youtube, 'wss://www.youtube.com/', false],
    [youtube, 'https://www.youtube-nocookie.com/embed/x', true],
    [youtube, 'https://www.youtube-nocookie.com/embed', true],
    [youtube, 'https://www.youtube-nocookie.com/other', false],
    [youtube, 'https://www.youtube.com./watch', true],
    [{ matches: ['<all_urls>'] }, 'https://example.com/', true],
    [{ matches: ['<all_urls>'] }, 'ftp://example.com/', false],
    [{ matches: ['<all_urls>'] }, 'file:///home/a.txt', false],
    [{ matches: ['<all_urls>'], fileAccess: true }, 'file:///home/a.txt', true],
    [{ matches: ['<all_urls>'] }, 'chrome-extension://abc/page.html', false],
    [{ matches: ['<all_urls>'] }, 'https://chromewebstore.google.com/detail/x', false],
    [{ matches: ['<all_urls>'] }, 'https://chrome.google.com/webstore/detail', false],
    [{ matches: ['<all_urls>'] }, 'https://chrome.google.com/webstorex', true],
    [{ matches: ['https://example.com:8443/*'] }, 'https://example.com:8443/a', true],
    [{ matches: ['https://example.com:443/*'] }, 'https://example.com/a', true],
    [{ matches: ['https://example.com:8443/*'] }, 'https://example.com/a', false],
    [{ matches: ['http://[::1]/*'] }, 'http://[::1]:80/x', true],
    [{ matches: ['http://*.127.0.0.1/*', 'http://*.example.com/*'] }, 'http://127.0.0.1/', false],
    [{ matches: ['https://example.com/a?b*'] }, 'https://example.com/a?bc', true],
    [{ matches: ['https://example.com/a?b'] }, 'https://example.com/a?b#frag', true],
    [{ matches: ['https://example.com/a?'] }, 'https://example.com/a?', true],
    [{ matches: ['https://example.com/a?'] }, 'https://example.com/a', false],
    [{ matches: ['https://example.com/*a*b'] }, 'https://example.com/xxaxxbb', true],
    [{ matches: ['https://example.com/*a*b'] }, 'https://example.com/xxbxxa', false],
    [{ matches: ['*://*/*'], excludeMatches: ['*://*/*.xml'] }, 'https://example.com/feed.xml', false],
    [{ matches: ['*://*/*'], excludeMatches: ['*://*/*.xml'] }, 'https://example.com/feed.html', true],
    [{ matches: ['*://*/*'], includeGlobs: ['*example.com/??/*'] }, 'https://example.com/en/x', true],
    [{ matches: ['*://*/*'], includeGlobs: ['*example.com/??/*'] }, 'https://example.com/eng/x', false],
    [{ matches: ['*://*/*'], excludeGlobs: ['*#private'] }, 'https://example.com/#private', false],
    [{ matches: ['*://*/*'], excludeGlobs: ['*#private'] }, 'https://example.com/#public', true],
  ];
  for (const [data, href, expected] of cases)
    assert.equal(decide(data, href), expected, `${JSON.stringify(data)} ${href}`);
});

test('subframes, about:blank, srcdoc and origin fallback follow the declaration', () => {
  const { Z } = createRealm();
  const decide = (data, href, extra) => Z.content.matches(data, frame(href, extra));
  const all = { matches: ['https://example.com/*'], allFrames: true };
  assert.equal(decide({ matches: ['https://example.com/*'] }, 'https://example.com/', { top: false }), false);
  assert.equal(decide(all, 'https://example.com/', { top: false }), true);

  const blank = { origin: 'https://example.com', creator: 'https://example.com/parent', top: false };
  assert.equal(decide(all, 'about:blank', blank), false);
  assert.equal(decide({ ...all, matchAboutBlank: true }, 'about:blank', blank), true);
  assert.equal(decide({ ...all, matchAboutBlank: true }, 'about:srcdoc', blank), true);
  assert.equal(
    decide({ matches: ['https://example.com/parent'], allFrames: true, matchAboutBlank: true }, 'about:blank', blank),
    true,
  );
  assert.equal(
    decide({ ...all, matchAboutBlank: true }, 'about:blank', { ...blank, origin: 'https://other.org' }),
    false,
    "a blank frame that does not share its creator's origin is matched by its own URL",
  );
  assert.equal(decide({ ...all, matchAboutBlank: true }, 'about:blank', { ...blank, creator: null }), false);

  const fallback = { matches: ['https://example.com/*'], allFrames: true, matchOriginAsFallback: true };
  assert.equal(
    decide(
      { matches: ['https://example.com/parent'], allFrames: true, matchOriginAsFallback: true },
      'about:blank',
      blank,
    ),
    false,
    'the origin, not the creator path, is matched',
  );
  assert.equal(decide(fallback, 'about:blank', blank), true);
  assert.equal(decide(fallback, 'blob:https://example.com/2c0f', { origin: 'https://example.com', top: false }), true);
  assert.equal(decide(fallback, 'data:text/html,x', { origin: 'null', top: false }), false);
  assert.equal(
    decide({ matches: ['<all_urls>'], allFrames: true }, 'data:text/html,x', { origin: 'null', top: false }),
    false,
  );
});

test('the live frame reader walks to the closest non-blank creator', () => {
  const { Z } = createRealm();
  class FakeWindow {
    constructor(href, parent, origin) {
      this.location = { href };
      this.originValue = origin;
      this.parentValue = parent;
    }
    get origin() {
      return this.originValue;
    }
    get parent() {
      return this.parentValue ?? this;
    }
    get top() {
      return this.parentValue ? this.parentValue.top : this;
    }
    get opener() {
      return null;
    }
  }
  const shared = (name) => Object.getOwnPropertyDescriptor(FakeWindow.prototype, name);
  const make = (href, parent, origin) => {
    const window = new FakeWindow(href, parent, origin);
    for (const name of ['origin', 'parent', 'top', 'opener']) Object.defineProperty(window, name, shared(name));
    Object.defineProperty(window.location, 'href', { get: () => href });
    return window;
  };
  const top = make('https://example.com/top', null, 'https://example.com');
  const blank = make('about:blank', top, 'https://example.com');
  const nested = make('about:srcdoc', blank, 'https://example.com');
  const reader = Z.content.frameReader(nested);
  assert.equal(reader.href, 'about:srcdoc');
  assert.equal(reader.top, false);
  assert.equal(reader.creator(), 'https://example.com/top');
  assert.equal(Z.content.frameReader(top).top, true);
  const foreign = new FakeWindow('x', null, 'https://b.example');
  Object.defineProperty(foreign, 'location', {
    get: () => {
      throw new Error('SecurityError');
    },
  });
  const crossOrigin = make('about:blank', foreign, 'https://a.example');
  assert.equal(Z.content.frameReader(crossOrigin).creator(), null);
});

function contentRealm(href) {
  const realm = createRealm();
  const host = new FakeHost(realm.Z, 'contentScript');
  const global = realm.run('globalThis');
  const booted = realm.Z.content.boot(surface, bootstrap({ contextKind: 'contentScript' }), {
    link: host.link,
    frame: frame(href),
  });
  const guard = realm.run("globalThis[Symbol.for('zephium.content.v1')]");
  return { ...realm, host, global, booted, guard };
}

test('boot publishes a fixed guard and creates the API only when a script matches', () => {
  const { guard, run, booted } = contentRealm('https://example.com/');
  assert.equal(run('typeof chrome'), 'undefined');
  assert.equal(guard.run({ matches: ['https://other.org/*'] }), false);
  assert.equal(run('typeof chrome'), 'undefined');
  assert.equal(guard.run({ matches: ['https://example.com/*'] }), true);
  assert.deepEqual(plain(Object.keys(run('chrome')).sort()), ['dom', 'extension', 'i18n', 'runtime', 'storage']);
  const chrome = run('chrome');
  assert.equal(guard.run({ matches: ['<all_urls>'] }), true);
  assert.equal(run('chrome'), chrome, 'one API per world');
  assert.equal(booted.context.chrome, chrome);
  run("globalThis[Symbol.for('zephium.content.v1')] = { run: () => true }");
  assert.equal(run("globalThis[Symbol.for('zephium.content.v1')]"), guard);
  assert.equal(guard.run(null), false);
  assert.equal(guard.run({ matches: 'https://example.com/*' }), false);
});

test('document_idle waits for a trusted load or 200 ms after document_end', async () => {
  const { Z } = createRealm();
  const target = new EventTarget();
  target.document = {};
  const ran = [];
  Z.content.scheduleIdle(target, () => ran.push('complete'));
  await flush();
  assert.deepEqual(ran, ['complete'], 'without a readable readyState the document counts as loaded');
});

test('document_idle in a loading document', async () => {
  class Document {
    get readyState() {
      return 'interactive';
    }
  }
  const realm = createRealm({ Document });
  const target = new EventTarget();
  target.document = new Document();
  const ran = [];
  realm.Z.content.scheduleIdle(target, () => ran.push('timer'));
  target.dispatchEvent(new Event('load'));
  await flush();
  assert.deepEqual(ran, [], 'an untrusted load event does not count');
  await new Promise((resolve) => setTimeout(resolve, 230));
  assert.deepEqual(ran, ['timer']);
});

test('the idle guard runs the bundle later only when it matches', async () => {
  const { guard } = contentRealm('https://example.com/');
  const ran = [];
  assert.equal(
    guard.idle({ matches: ['https://other.org/*'] }, () => ran.push('no')),
    false,
  );
  assert.equal(
    guard.idle({ matches: ['https://example.com/*'] }, () => ran.push('yes')),
    true,
  );
  assert.equal(guard.idle({ matches: ['https://example.com/*'] }, 'not a function'), false);
  assert.deepEqual(ran, []);
  await flush();
  assert.deepEqual(ran, ['yes']);
});
