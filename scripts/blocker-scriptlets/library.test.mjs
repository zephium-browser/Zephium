// Runs the scriptlet library in a sandbox with stand-ins for the page APIs it
// wraps, on synthetic YouTube-shaped payloads.
import { readFileSync } from 'node:fs';
import { strict as assert } from 'node:assert';
import vm from 'node:vm';

const library = new URL('../../crates/zephium-blocker/src/scriptlets/', import.meta.url);
const source = (name) => readFileSync(new URL(name, library), 'utf8')
  .split('\n').filter((line) => !line.startsWith('//')).join('\n');
const files = ['pattern.js', 'json-pruner.js', 'request-matcher.js', 'timer-booster.js', 'text-replacer.js',
  'json-prune.js', 'json-prune-fetch-response.js', 'json-prune-xhr-response.js', 'nano-settimeout-booster.js',
  'nano-setinterval-booster.js', 'trusted-replace-fetch-response.js', 'trusted-replace-xhr-response.js',
  'trusted-prevent-dom-bypass.js', 'no-xhr-if.js', 'object-editor.js', 'trusted-edit-inbound-object.js',
  'zephium-player-marker.js'];

const page = (fetchBody) => {
  class FakeXhr extends EventTarget {
    open(method, url) { this.method = method; this.url = url; }
    send() {}
    abort() {}
  }
  for (const name of ['responseText', 'response']) {
    Object.defineProperty(FakeXhr.prototype, name, {
      configurable: true,
      enumerable: true,
      get() {
        const textual = this.responseType === undefined || this.responseType === '' || this.responseType === 'text';
        if (name === 'responseText' && !textual) {
          throw new DOMException('not text', 'InvalidStateError');
        }
        return name === 'response' && this.responseType === 'json' ? JSON.parse(this.body) : this.body;
      },
    });
  }
  // Each page wraps its own Response, as each document does.
  const PageResponse = class Response extends globalThis.Response {};
  const timers = [];
  const context = {
    Response: PageResponse, Request, Headers, XMLHttpRequest: FakeXhr, Event, DOMException, URL, TextEncoder, Blob, console, timers,
    setTimeout: (handler, delay) => timers.push(['timeout', delay]),
    setInterval: (handler, delay) => timers.push(['interval', delay]),
  };
  context.window = context;
  context.self = context;
  context.fetch = async (resource) => {
    const response = new PageResponse(fetchBody, { status: 200, headers: { 'content-type': 'application/json' } });
    Object.defineProperty(response, 'url', { value: String(resource) });
    return response;
  };
  vm.createContext(context);
  vm.runInContext(files.map(source).join('\n'), context);
  return context;
};

const player = () => ({
  playabilityStatus: { status: 'OK' },
  adPlacements: [{ adPlacementRenderer: { config: { kind: 'AD_PLACEMENT_KIND_START' } } }],
  adSlots: [{ adSlotRenderer: {} }],
  playerAds: [{ playerLegacyDesktopWatchAdsRenderer: {} }],
  streamingData: { formats: [1, 2] },
});

// The pruner: chains, wildcards, removal tokens and required props.
{
  const context = page('{}');
  const prune = (remove, require, value) => context.zephiumJsonPruner(remove, require)(value);
  assert.deepEqual(prune('a.b', '', { a: { b: 1, c: 2 } }).value, { a: { c: 2 } });
  assert.deepEqual(prune('[].ad', '', [{ ad: 1, x: 1 }, { ad: 2 }]).value, [{ x: 1 }, {}]);
  assert.deepEqual(prune('*.ad', '', { one: { ad: 1 }, two: { ad: 2, y: 3 } }).value, { one: {}, two: { y: 3 } });
  assert.deepEqual(
    prune('entries.[-].command.isAd', '', { entries: [{ command: { isAd: true } }, { id: 2 }, { command: { isAd: 1 } }] }).value,
    { entries: [{ id: 2 }] },
  );
  assert.deepEqual(prune('slots.{-}.promoted', '', { slots: { a: { promoted: 1 }, b: {} } }).value, { slots: { b: {} } });
  const kept = prune('adPlacements', 'playabilityStatus missing', player());
  assert.equal(kept.changed, false);
  assert.ok(kept.value.adPlacements);
  assert.equal(prune('nothing.here', '', player()).changed, false);
}

// The pattern argument.
{
  const context = page('{}');
  assert.ok(context.zephiumPattern('/\\/player(\\?|$)/')('https://www.youtube.com/youtubei/v1/player?key=1'));
  assert.ok(!context.zephiumPattern('/\\/player(\\?|$)/')('https://www.youtube.com/youtubei/v1/player_ads'));
  assert.ok(context.zephiumPattern('player')('/v1/player'));
  assert.ok(context.zephiumPattern('')('anything'));
  assert.ok(!context.zephiumPattern('/(/')('anything'));
}

// json-prune: JSON.parse and Response.json.
{
  const context = page('{}');
  context.zephiumJsonPrune('adPlacements adSlots playerAds');
  const parsed = vm.runInContext(`JSON.parse(${JSON.stringify(JSON.stringify(player()))})`, context);
  assert.deepEqual(Object.keys(parsed), ['playabilityStatus', 'streamingData']);
  const viaResponse = await vm.runInContext(`new Response(${JSON.stringify(JSON.stringify(player()))}).json()`, context);
  assert.deepEqual(Object.keys(viaResponse), ['playabilityStatus', 'streamingData']);
  assert.match(vm.runInContext('String(JSON.parse)', context), /native code/);
}

// json-prune-fetch-response: only matching requests, and the response keeps its address.
{
  const context = page(JSON.stringify(player()));
  context.zephiumJsonPruneFetchResponse('adPlacements adSlots playerAds', '', 'propsToMatch', '/\\/player(\\?|$)/');
  const edited = await context.fetch('https://www.youtube.com/youtubei/v1/player?prettyPrint=false', { method: 'POST' });
  assert.equal(edited.url, 'https://www.youtube.com/youtubei/v1/player?prettyPrint=false');
  assert.equal(edited.status, 200);
  assert.deepEqual(Object.keys(await edited.json()), ['playabilityStatus', 'streamingData']);
  const other = await context.fetch('https://www.youtube.com/youtubei/v1/next');
  assert.ok((await other.json()).adPlacements);
  const named = page(JSON.stringify(player()));
  named.zephiumJsonPruneFetchResponse('adSlots', '', 'propsToMatch', 'method:POST');
  assert.ok((await (await named.fetch('/x', { method: 'GET' })).json()).adSlots);
  assert.equal((await (await named.fetch('/x', { method: 'POST' })).json()).adSlots, undefined);
  const text = page('not json');
  text.zephiumJsonPruneFetchResponse('adSlots', '', 'propsToMatch', 'player');
  assert.equal(await (await text.fetch('/player')).text(), 'not json');
}

// json-prune-xhr-response: text and json response types.
{
  const context = page('{}');
  context.zephiumJsonPruneXhrResponse('adPlacements adSlots', '', 'propsToMatch', '/\\/player(?:\\?.+)?$/');
  const request = (url, responseType) => {
    const xhr = new context.XMLHttpRequest();
    xhr.open('POST', url);
    xhr.responseType = responseType;
    xhr.readyState = 4;
    xhr.body = JSON.stringify(player());
    return xhr;
  };
  const textual = request('https://www.youtube.com/youtubei/v1/player?key=1', '');
  assert.deepEqual(Object.keys(JSON.parse(textual.responseText)), ['playabilityStatus', 'playerAds', 'streamingData']);
  assert.equal(textual.response, textual.responseText);
  const json = request('https://www.youtube.com/youtubei/v1/player', 'json');
  assert.deepEqual(Object.keys(json.response), ['playabilityStatus', 'playerAds', 'streamingData']);
  const untouched = request('https://www.youtube.com/youtubei/v1/next', '');
  assert.ok(JSON.parse(untouched.responseText).adPlacements);
  const early = request('https://www.youtube.com/youtubei/v1/player', '');
  early.readyState = 3;
  assert.ok(JSON.parse(early.responseText).adPlacements);
}

// The text replacer: `*`, strings and regular expressions as String.prototype.replace, and `includes`.
{
  const context = page('{}');
  const edit = (pattern, replacement, text, ...options) => context.zephiumTextReplacer(pattern, replacement, options)(text);
  assert.equal(edit('*', '', 'anything'), '');
  assert.equal(edit('*', '{}', 'anything'), '{}');
  assert.equal(edit('"adSlots"', '"no_ads"', '{"adSlots":[],"x":{"adSlots":1}}'), '{"no_ads":[],"x":{"adSlots":1}}');
  assert.equal(edit('/"ad\\w+"/g', '"no_ads"', '{"adSlots":1,"adPlacements":2}'), '{"no_ads":1,"no_ads":2}');
  assert.equal(edit('/A(b)/i', '$1$1', 'xaby'), 'xbby');
  assert.equal(edit('absent', 'x', 'text'), null);
  assert.equal(edit('a', 'b', 'a', 'includes', 'zzz'), null);
  assert.equal(edit('a', 'b', 'a z', 'includes', '/z$/'), 'b z');
  assert.equal(context.zephiumTextReplacer('', 'x', []), null);
  assert.equal(context.zephiumTextReplacer('/(/', 'x', []), null);
  // One expression serves every response: a sticky or global one starts each from the beginning.
  for (const pattern of ['/a/y', '/a/g']) {
    const reused = context.zephiumTextReplacer(pattern, 'b', []);
    assert.equal(reused('ab'), 'bb', pattern);
    assert.equal(reused('ab'), 'bb', pattern);
  }
}

// A player response shaped like YouTube's, as text, for the replace scriptlets.
const playerText = () => JSON.stringify({
  responseContext: { serviceTrackingParams: [] },
  playabilityStatus: { status: 'OK' },
  streamingData: { formats: [{ itag: 18 }] },
  adPlacements: [{ adPlacementRenderer: { config: { adPlacementConfig: { kind: 'AD_PLACEMENT_KIND_START' } }, adSlotLoggingData: { serializedSlotAdServingDataEntry: 'EAE' } } }],
  adSlots: [{ adSlotRenderer: { adSlotMetadata: { slotType: 'SLOT_TYPE_PLAYER_BYTES' } } }],
  adBreakHeartbeatParams: 'Q0FB',
  videoDetails: { videoId: 'dQw0000000A', title: 'A' },
});

// trusted-replace-fetch-response: the rules' own patterns, only on matching requests, keeping the address.
{
  const context = page(playerText());
  context.zephiumTrustedReplaceFetchResponse('"adPlacements"', '"no_ads"', 'player?');
  const edited = await context.fetch('https://www.youtube.com/youtubei/v1/player?prettyPrint=false', { method: 'POST' });
  assert.equal(edited.url, 'https://www.youtube.com/youtubei/v1/player?prettyPrint=false');
  assert.equal(edited.status, 200);
  const body = await edited.json();
  assert.equal(body.adPlacements, undefined);
  assert.ok(body.no_ads && body.adSlots && body.streamingData);
  assert.ok((await (await context.fetch('https://www.youtube.com/youtubei/v1/next')).json()).adPlacements);
  const both = page(playerText());
  both.zephiumTrustedReplaceFetchResponse('"adPlacements"', '"no_ads"', 'player?');
  both.zephiumTrustedReplaceFetchResponse('"adSlots"', '"no_ads"', 'player?');
  const keys = Object.keys(await (await both.fetch('/youtubei/v1/player?key=1')).json());
  assert.ok(!keys.includes('adPlacements') && !keys.includes('adSlots') && keys.includes('videoDetails'));
  const named = page(playerText());
  named.zephiumTrustedReplaceFetchResponse('*', '', 'method:/GET|HEAD/');
  assert.equal(await (await named.fetch('/x', { method: 'GET' })).text(), '');
  assert.ok((await (await named.fetch('/x', { method: 'POST' })).json()).adSlots);
  const gated = page(playerText());
  gated.zephiumTrustedReplaceFetchResponse('"adSlots"', '"no_ads"', '', 'includes', 'notInThisBody');
  assert.ok((await (await gated.fetch('/player')).json()).adSlots);
  const untouched = page(playerText());
  untouched.zephiumTrustedReplaceFetchResponse('', 'x', 'player');
  assert.ok((await (await untouched.fetch('/player')).json()).adSlots);
  // A URL-only call is a GET, and a method fetch() normalizes matches in upper case.
  const methods = page(playerText());
  methods.zephiumTrustedReplaceFetchResponse('"adSlots"', '"no_ads"', 'method:GET');
  assert.equal((await (await methods.fetch('/x')).json()).adSlots, undefined);
  const posted = page(playerText());
  posted.zephiumTrustedReplaceFetchResponse('"adSlots"', '"no_ads"', 'method:POST');
  assert.equal((await (await posted.fetch('/x', { method: 'post' })).json()).adSlots, undefined);
  assert.ok((await (await posted.fetch('/x', { method: 'patch' })).json()).adSlots);
  // A request from another realm is known by its tag, not its prototype.
  const foreign = page(playerText());
  foreign.zephiumTrustedReplaceFetchResponse('"adSlots"', '"no_ads"', 'player?');
  const request = { url: 'https://www.youtube.com/youtubei/v1/player?key=1', method: 'POST', [Symbol.toStringTag]: 'Request' };
  assert.equal((await (await foreign.fetch(request)).json()).adSlots, undefined);
  // Copies of the edited response, and copies of those, keep its address.
  const copied = await foreign.fetch('https://www.youtube.com/youtubei/v1/player?key=2');
  assert.equal(copied.clone().url, 'https://www.youtube.com/youtubei/v1/player?key=2');
  assert.equal(copied.clone().clone().url, 'https://www.youtube.com/youtubei/v1/player?key=2');
  assert.equal((await copied.clone().json()).adSlots, undefined);
}

// trusted-replace-xhr-response: the rules' regular expressions on text and json responses.
{
  const request = (context, url, responseType) => {
    const xhr = new context.XMLHttpRequest();
    xhr.open('POST', url);
    xhr.responseType = responseType;
    xhr.readyState = 4;
    xhr.body = playerText();
    return xhr;
  };
  const cut = page('{}');
  cut.zephiumTrustedReplaceXhrResponse('/"adPlacements.*?("adSlots"|"adBreakHeartbeatParams")/gms', '$1', '/\\/player(?:\\?.+)?$/');
  const textual = request(cut, 'https://www.youtube.com/youtubei/v1/player?key=1', '');
  assert.deepEqual(Object.keys(JSON.parse(textual.responseText)),
    ['responseContext', 'playabilityStatus', 'streamingData', 'adSlots', 'adBreakHeartbeatParams', 'videoDetails']);
  assert.equal(textual.response, textual.responseText);
  const json = request(cut, 'https://www.youtube.com/youtubei/v1/player', 'json');
  assert.equal(json.response.adPlacements, undefined);
  assert.equal(json.response.videoDetails.videoId, 'dQw0000000A');
  assert.ok(JSON.parse(request(cut, 'https://www.youtube.com/youtubei/v1/next', '').responseText).adPlacements);
  const early = request(cut, 'https://www.youtube.com/youtubei/v1/player', '');
  early.readyState = 3;
  assert.ok(JSON.parse(early.responseText).adPlacements);
  // The other rule takes the placements out with what follows them, up to the closing bracket.
  const whole = page('{}');
  whole.zephiumTrustedReplaceXhrResponse('/"adPlacements.*?([A-Z]"\\}|"\\}{2,4})\\}\\],/', '', '/playlist\\?list=|\\/player(?:\\?.+)?$|watch\\?[tv]=/');
  const pruned = JSON.parse(request(whole, 'https://www.youtube.com/youtubei/v1/player', '').responseText);
  assert.equal(pruned.adPlacements, undefined);
  assert.ok(pruned.adSlots && pruned.videoDetails);
  // An edit that leaves a json response unparsable keeps the original.
  const broken = page('{}');
  broken.zephiumTrustedReplaceXhrResponse('*', '{', '');
  assert.ok(request(broken, '/player', 'json').response.adSlots);
  // A lower-case method open() normalizes still matches, and responseText of json still throws.
  const typed = page('{}');
  typed.zephiumTrustedReplaceXhrResponse('"adSlots"', '"no_ads"', 'method:POST');
  const lower = new typed.XMLHttpRequest();
  lower.open('post', '/player');
  lower.responseType = '';
  lower.readyState = 4;
  lower.body = playerText();
  assert.equal(JSON.parse(lower.responseText).adSlots, undefined);
  const asJson = request(typed, '/player', 'json');
  assert.throws(() => asJson.responseText, { name: 'InvalidStateError' });
  assert.equal(asJson.response.adSlots, undefined);
}

// no-xhr-if: matching requests never leave and complete with the directive's body.
{
  const task = () => new Promise((resolve) => setTimeout(resolve, 0));
  const later = (handler, delay) => setTimeout(handler, delay);
  const context = page('{}');
  context.setTimeout = later;
  context.zephiumNoXhrIf('/\\/api\\/stats\\/atr\\?.+&docid=/ method:POST');
  const sent = [];
  context.XMLHttpRequest.prototype.send = new Proxy(context.XMLHttpRequest.prototype.send, {});
  const open = (method, url, responseType = '') => {
    const xhr = new context.XMLHttpRequest();
    xhr.responseType = responseType;
    xhr.addEventListener('load', () => sent.push(['load', url]));
    xhr.open(method, url);
    return xhr;
  };
  const atr = open('POST', 'https://www.youtube.com/api/stats/atr?ns=yt&rt=1.0&docid=abc');
  const events = [];
  for (const type of ['readystatechange', 'load', 'loadend']) {
    atr.addEventListener(type, () => events.push(type));
  }
  atr.send('body');
  assert.deepEqual(events, []);
  // Like a native request, it completes from a later task, not a microtask.
  await Promise.resolve();
  assert.deepEqual(events, []);
  await task();
  assert.deepEqual(events, ['readystatechange', 'load', 'loadend']);
  assert.throws(() => atr.send(), { name: 'InvalidStateError' });
  assert.equal(atr.readyState, 4);
  assert.equal(atr.status, 200);
  assert.equal(atr.responseText, '');
  assert.equal(atr.responseURL, 'https://www.youtube.com/api/stats/atr?ns=yt&rt=1.0&docid=abc');
  // A relative address is matched as written and reported resolved, without its fragment.
  const relative = page('{}');
  relative.setTimeout = later;
  relative.document = { baseURI: 'https://www.youtube.com/watch?v=1' };
  relative.zephiumNoXhrIf('/^\\/api\\/stats\\/qoe/');
  const qoe = new relative.XMLHttpRequest();
  qoe.open('GET', '/api/stats/qoe?x=1#frag', false);
  qoe.send();
  assert.equal(qoe.responseURL, 'https://www.youtube.com/api/stats/qoe?x=1');
  const get = open('GET', 'https://www.youtube.com/api/stats/atr?ns=yt&docid=abc');
  get.send();
  await task();
  assert.equal(get.readyState, undefined);
  // open() sends a lower-case POST as POST.
  const lower = open('post', 'https://www.youtube.com/api/stats/atr?ns=yt&docid=abc');
  lower.send();
  await task();
  assert.equal(lower.status, 200);
  // An abort before completion ends it; so does a listener that opens the request again.
  const aborted = open('POST', 'https://www.youtube.com/api/stats/atr?ns=yt&docid=abc');
  const abortedEvents = [];
  aborted.addEventListener('load', () => abortedEvents.push('load'));
  aborted.send();
  aborted.abort();
  await task();
  assert.deepEqual(abortedEvents, []);
  assert.equal(aborted.status, undefined);
  const reopened = open('POST', 'https://www.youtube.com/api/stats/atr?ns=yt&docid=abc');
  const reopenedEvents = [];
  reopened.addEventListener('readystatechange', () => {
    reopenedEvents.push('readystatechange');
    reopened.open('GET', 'https://www.youtube.com/next');
  });
  reopened.addEventListener('load', () => reopenedEvents.push('load'));
  reopened.send();
  await task();
  assert.deepEqual(reopenedEvents, ['readystatechange']);
  // One response object per type, and responseText only for text.
  const objects = page('{}');
  objects.setTimeout = later;
  objects.zephiumNoXhrIf('ads.js', 'emptyObj');
  const typed = new objects.XMLHttpRequest();
  typed.responseType = 'json';
  typed.open('GET', 'https://cdn.test/ads.js');
  typed.send();
  await task();
  assert.equal(typeof typed.response, 'object');
  assert.equal(typed.response, typed.response);
  assert.throws(() => typed.responseText, { name: 'InvalidStateError' });
  typed.responseType = 'text';
  assert.equal(typed.response, '{}');
  assert.equal(typed.responseText, '{}');
  const directive = (value, responseType = '') => {
    const other = page('{}');
    other.zephiumNoXhrIf('ads.js !example.com', value);
    const xhr = new other.XMLHttpRequest();
    xhr.responseType = responseType;
    xhr.open('GET', 'https://cdn.test/ads.js', false);
    xhr.send();
    return xhr.response;
  };
  assert.equal(directive(''), '');
  assert.equal(directive('true').length, 10);
  assert.equal(directive('emptyObj'), '{}');
  assert.equal(JSON.stringify(directive('emptyArr', 'json')), '[]');
  assert.equal(directive('length:5-7').length >= 5, true);
  assert.equal(directive('war:noop-vast3.xml'), '');
  const negated = page('{}');
  negated.zephiumNoXhrIf('ads.js !example.com');
  const kept = new negated.XMLHttpRequest();
  kept.open('GET', 'https://example.com/ads.js', false);
  kept.send();
  assert.equal(kept.readyState, undefined);
  const silent = page('{}');
  silent.zephiumNoXhrIf();
  assert.equal(Object.getOwnPropertyDescriptor(silent.XMLHttpRequest.prototype, 'status'), undefined);
}

// trusted-prevent-dom-bypass: a frame inserted by the named method uses the parent's patched function.
{
  const parent = page('{}');
  const frameWindow = vm.createContext({ fetch: async () => 'frame', JSON });
  vm.runInContext('this.fetch = fetch; this.JSON = JSON;', frameWindow);
  // Each querySelectorAll accepts only its own kind of receiver, as the DOM's do.
  class FakeElement {
    constructor(children = []) { this.children = children; }
    querySelectorAll() {
      if (!(this instanceof FakeElement)) { throw new TypeError('Illegal invocation'); }
      return this.children;
    }
  }
  class FakeFrame extends FakeElement {
    constructor(view) { super(); this.contentWindow = null; this.view = view; }
  }
  class FakeFragment {
    constructor(children) { this.children = children; }
    querySelectorAll() {
      if (!(this instanceof FakeFragment)) { throw new TypeError('Illegal invocation'); }
      return this.children;
    }
  }
  // Inserting a fragment moves its children out of it, as the DOM does.
  const makeNode = () => {
    const Node = function Node(connected = true) { this.connected = connected; };
    Object.defineProperty(Node.prototype, 'isConnected', { get() { return this.connected; } });
    Node.prototype.appendChild = function appendChild(child) {
      const inner = child instanceof FakeFragment ? FakeFragment : FakeElement;
      for (const frame of this.connected ? [child, ...inner.prototype.querySelectorAll.call(child)] : []) {
        if (frame instanceof FakeFrame) { frame.contentWindow = frame.view; }
      }
      if (child instanceof FakeFragment) { child.children = []; }
      return child;
    };
    return Node;
  };
  const dom = { HTMLElement: FakeElement, Element: FakeElement, DocumentFragment: FakeFragment };
  const Node = makeNode();
  Object.assign(parent, { Node, ...dom });
  parent.fetch = new Proxy(parent.fetch, { apply: () => Promise.resolve('patched') });
  parent.zephiumTrustedPreventDomBypass('Node.prototype.appendChild', 'fetch');
  const frame = new FakeFrame(frameWindow);
  new Node().appendChild(frame);
  assert.equal(await frameWindow.fetch('/youtubei/v1/player'), 'patched');
  // A dotted target, on a frame inside the inserted element.
  const parsing = page('{}');
  const ParsingNode = makeNode();
  Object.assign(parsing, { Node: ParsingNode, ...dom });
  vm.runInContext('JSON.parse = new Proxy(JSON.parse, { apply: () => "parent" });', parsing);
  parsing.zephiumTrustedPreventDomBypass('Node.prototype.appendChild', 'JSON.parse');
  const nestedWindow = vm.createContext({});
  const nested = new FakeFrame(vm.runInContext('globalThis', nestedWindow));
  new ParsingNode().appendChild(new FakeElement([nested]));
  assert.equal(vm.runInContext('JSON.parse("1")', nestedWindow), 'parent');
  // A frame inside a fragment, which the insertion empties.
  const fragmentWindow = vm.createContext({});
  const inFragment = new FakeFrame(vm.runInContext('globalThis', fragmentWindow));
  new ParsingNode().appendChild(new FakeFragment([inFragment]));
  assert.equal(vm.runInContext('JSON.parse("1")', fragmentWindow), 'parent');
  // A named control shadowing querySelectorAll on the inserted node changes nothing.
  const shadowedWindow = vm.createContext({});
  const shadowed = new FakeElement([new FakeFrame(vm.runInContext('globalThis', shadowedWindow))]);
  shadowed.querySelectorAll = () => { throw new Error('a form control'); };
  new ParsingNode().appendChild(shadowed);
  assert.equal(vm.runInContext('JSON.parse("1")', shadowedWindow), 'parent');
  // A detached receiver gives its frames no window, so nothing is scanned.
  const detached = new FakeElement([new FakeFrame(vm.createContext({}))]);
  let scanned = 0;
  const counted = detached.children[0];
  Object.defineProperty(counted, 'contentWindow', { get() { scanned++; return null; }, configurable: true });
  new ParsingNode(false).appendChild(detached);
  assert.equal(scanned, 0);
  const whole = page('{}');
  const WholeNode = makeNode();
  Object.assign(whole, { Node: WholeNode, ...dom, document: {} });
  whole.zephiumTrustedPreventDomBypass('Node.prototype.appendChild');
  const replaced = new FakeFrame(vm.createContext({}));
  new WholeNode().appendChild(replaced);
  assert.equal(replaced.contentWindow, vm.runInContext('globalThis', whole));
  assert.equal(replaced.contentDocument, whole.document);
  const missing = page('{}');
  missing.zephiumTrustedPreventDomBypass('No.such.method', 'fetch');
}

// Timer boosters: the needle, the exact delay or `*`, and the 50x bound.
{
  const context = page('{}');
  context.zephiumSetTimeoutBooster('[native code]', '17000', '0.001');
  vm.runInContext(`setTimeout(Function.prototype.bind.call(() => {}, null), 17000);
    setTimeout(() => {}, 17000);
    setTimeout(Function.prototype.bind.call(() => {}, null), 16000);`, context);
  assert.deepEqual(context.timers, [['timeout', 340], ['timeout', 17000], ['timeout', 16000]]);
  const any = page('{}');
  any.zephiumSetIntervalBooster('', '*', '');
  vm.runInContext('setInterval(() => {}, 4000)', any);
  assert.deepEqual(any.timers, [['interval', 200]]);
  const defaults = page('{}');
  defaults.zephiumSetTimeoutBooster();
  vm.runInContext('setTimeout(() => {}, 1000); setTimeout(() => {}, 999)', defaults);
  assert.deepEqual(defaults.timers, [['timeout', 50], ['timeout', 999]]);
  const slower = page('{}');
  slower.zephiumSetTimeoutBooster('', '*', '200');
  vm.runInContext('setTimeout(() => {}, 10)', slower);
  assert.deepEqual(slower.timers, [['timeout', 500]]);
}

// Object edits: the queries the compiler admits, as edit-queries.json lists them for both readers.
{
  const context = page('{}');
  const queries = JSON.parse(readFileSync(new URL('edit-queries.json', import.meta.url), 'utf8'));
  for (const query of queries.accepted) {
    assert.notEqual(context.zephiumObjectEditor(query), undefined, query);
  }
  for (const query of queries.rejected) {
    assert.equal(context.zephiumObjectEditor(query), undefined, query);
  }
}

// trusted-edit-inbound-object: today's YouTube rules on the objects the player stringifies.
{
  const queries = JSON.parse(readFileSync(new URL('edit-queries.json', import.meta.url), 'utf8')).accepted.slice(0, 7);
  const context = page('{}');
  for (const query of queries) {
    context.zephiumTrustedEditInboundObject('JSON.stringify', '0', query);
  }
  const stringify = vm.runInContext('JSON.stringify', context);
  const agent = (marker) => `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7${marker === '' ? '' : `; ${marker}`}) AppleWebKit/537.36`;
  const request = (marker) => ({
    context: { client: { clientName: 'WEB', clientScreen: 'WATCH', userAgent: agent(marker) } },
    playbackContext: { contentPlaybackContext: { lactMilliseconds: '-1', referer: 'https://www.youtube.com/' } },
    attestationRequest: { omitBotguardData: true },
    videoId: 'abc',
  });
  const edited = (marker) => {
    const body = request(marker);
    const before = Date.now();
    const text = stringify(body);
    assert.deepEqual(JSON.parse(text), body);
    const lact = body.playbackContext.contentPlaybackContext.lactMilliseconds;
    if (lact !== '-1') {
      assert.match(lact, /^[0-9]+$/);
      assert.ok(Number(lact) >= before && Number(lact) <= Date.now());
      body.playbackContext.contentPlaybackContext.lactMilliseconds = 'now';
    }
    return body;
  };
  const expected = (marker, change) => {
    const body = request(marker);
    change(body.context.client, body.playbackContext.contentPlaybackContext, body);
    return body;
  };
  const reload = 'https://www.youtube.com/#reloadxhr';
  assert.deepEqual(edited('channel'), expected('channel', (client, playback) => {
    client.clientScreen = 'CHANNEL';
    Object.assign(playback, { lactMilliseconds: 'now', referer: reload });
  }));
  assert.deepEqual(edited('lactmilli'), expected('lactmilli', (client, playback, body) => {
    body.params = '8AUB';
    Object.assign(playback, { lactMilliseconds: 'now', referer: reload });
  }));
  assert.deepEqual(edited('yahi'), expected('yahi', (client, playback, body) => {
    body.params = 'YAHI';
    playback.referer = reload;
  }));
  assert.deepEqual(edited('inline'), expected('inline', (client, playback) => {
    Object.assign(playback, { isInlinePlaybackNoAd: true, lactMilliseconds: 'now', referer: reload });
  }));
  assert.deepEqual(edited('eafg'), expected('eafg', (client, playback) => {
    playback.referer = reload;
  }));
  assert.deepEqual(edited(''), request(''));
  const next = request('channel');
  delete next.attestationRequest;
  stringify(next);
  assert.equal(next.context.client.clientScreen, 'WATCH');
  const twice = request('yahi');
  stringify(twice);
  stringify(twice);
  assert.equal(twice.playbackContext.contentPlaybackContext.referer, reload);
  const comment = { createCommentParams: 'x', context: { client: { userAgent: agent('channel') } } };
  stringify(comment);
  assert.equal(comment.context.client.userAgent, agent(''));
  assert.equal(stringify([1, 'a']), '[1,"a"]');
  assert.match(vm.runInContext('String(JSON.stringify)', context), /native code/);
}

// The v2 grammar: removal, filters, regular-expression keys, quantifiers and assignments.
{
  const context = page('{}');
  const edit = (query, value) => {
    const editor = context.zephiumObjectEditor(query);
    assert.notEqual(editor, undefined, query);
    editor(value);
    return value;
  };
  const store = () => ({ store: { bicycle: { price: 19 }, book: [{ price: 8, author: 'Rob' }, { price: 22, author: 'Evelyn' }, { price: 9, author: 'Herman' }] } });
  assert.deepEqual(edit('v2:$..book[?@.price<10]', store()).store.book, [{ price: 22, author: 'Evelyn' }]);
  assert.deepEqual(edit('v2:$..book[?@.price>20].price=0', store()).store.book.map((book) => book.price), [8, 0, 9]);
  assert.deepEqual(edit('v2:$..book[?@.author^="E"]', store()).store.book.length, 2);
  assert.deepEqual(edit('v2:$..book[?@.author=/R/]', store()).store.book.map((book) => book.author), ['Evelyn', 'Herman']);
  assert.deepEqual(edit('v2:$.store.bicycle{0};$..book[?@.price<10]', store()), store());
  assert.deepEqual(edit('v2:$.store.bicycle;$..book[?@.price<10]', store()).store.book.length, 1);
  assert.deepEqual(edit('v2:$../^b/..price', store()), { store: { bicycle: {}, book: [{ author: 'Rob' }, { author: 'Evelyn' }, { author: 'Herman' }] } });
  assert.deepEqual(edit('$..book[-1]', store()).store.book.length, 2);
  assert.deepEqual(edit('v2:$.store+={"open":true}', store()).store.open, true);
  assert.deepEqual(edit('v2:$.store.book[0].author=repl({"pattern":"o","replacement":"0"})', store()).store.book[0].author, 'R0b');
  assert.deepEqual(edit('v2:$.missing=1', store()), store());
  assert.equal(context.zephiumObjectEditor('v2:$.a=call([null,"alert","x"])'), undefined);
}

// zephium-player-marker: marks the client after ytcfg.set, moves on from a hold, and leaves ads, started videos and Premium alone.
{
  const AGENT = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36';
  const marked = (marker) => AGENT.replace(') AppleWebKit', `; ${marker}) AppleWebKit`);
  // A response to a request the quick fixes edited carries their tag in its tracking URL.
  const TAGGED = { playabilityStatus: { status: 'OK' }, playbackTracking: { videostatsPlaybackUrl: { baseUrl: 'https://www.youtube.com/api/stats/playback?referrer=https%3A%2F%2Fwww.youtube.com%2F%23reloadxhr' } } };
  const youtube = ({ premium = 'ytInitialData.logo=PREMIUM', readyState = 'complete', search = '' } = {}) => {
    const listeners = {};
    let interval;
    const player = {
      id: 'v1', time: 0, state: 3, loads: [], response: { playabilityStatus: { status: 'OK' } },
      stats: { buffer_health_seconds: '0.00 s', resolution: '0x0', debug_info: 'USDAI, SABR, s:49' },
      getVideoData() { return { video_id: this.id }; },
      getPlayerState() { return this.state; },
      getCurrentTime() { return this.time; },
      getStatsForNerds() { return this.stats; },
      getPlayerResponse() { return this.response; },
      loadVideoById(id, start) { this.loads.push([id, start]); this.response = TAGGED; },
    };
    const context = {
      console,
      URLSearchParams,
      location: { pathname: '/watch', search },
      document: {
        readyState,
        visibilityState: 'visible',
        addEventListener: (type, handler) => { (listeners[type] ??= []).push(handler); },
        getElementById: (id) => (id === 'movie_player' ? player : null),
      },
      setInterval: (handler, delay) => { assert.equal(delay, 250); interval = handler; return 1; },
      clearInterval: () => { interval = undefined; },
      clock: 0,
    };
    vm.createContext(context);
    vm.runInContext(files.map(source).join('\n'), context);
    vm.runInContext('Date.now = () => clock;', context);
    context.zephiumPlayerMarker('channel lactmilli yahi inline', 'buffer_health_seconds=0.00 s|resolution=0x0', 'debug_info^=SSAP, AD', '1000', premium, 'answer/3037019', 'playbackTracking.videostatsPlaybackUrl.baseUrl*=reloadxhr');
    vm.runInContext(`var ytcfg = { data_: {}, get(key) { return this.data_[key]; }, set(values) { Object.assign(this.data_, values); } };
      ytcfg.set({ INNERTUBE_CONTEXT: { client: { clientName: 'WEB', userAgent: ${JSON.stringify(AGENT)} } } });`, context);
    const agent = () => vm.runInContext('ytcfg.get("INNERTUBE_CONTEXT").client.userAgent', context);
    const at = (ms) => { context.clock = ms; interval?.(); };
    const fire = (type) => listeners[type]?.forEach((handler) => handler());
    return { context, player, agent, at, fire, watching: () => interval !== undefined };
  };

  // The page's own data was inlined, so no marker reached it: its hold is
  // retried with the same marker, and a hold on the edited request's data moves on.
  const page = youtube();
  assert.equal(page.agent(), marked('channel'));
  assert.ok(page.watching());
  page.at(0);
  page.at(999);
  assert.deepEqual(page.player.loads, []);
  page.at(1000);
  assert.equal(page.agent(), marked('channel'));
  assert.deepEqual(page.player.loads, [['v1', 0]]);
  page.at(2000);
  page.at(3000);
  assert.equal(page.agent(), marked('lactmilli'));
  assert.equal(page.player.loads.length, 2);
  // The marker that played is kept for the page's next video.
  page.fire('yt-navigate-start');
  assert.equal(page.agent(), marked('lactmilli'));
  // Every marker held: the list starts over once, then the page is left unmarked.
  for (const [ms, marker] of [[4000, 'yahi'], [5000, 'inline'], [6000, 'channel'], [7000, 'lactmilli'], [8000, 'yahi'], [9000, 'inline']]) {
    page.at(ms);
    page.at(ms + 1000);
    assert.equal(page.agent(), marked(marker), marker);
  }
  page.at(10000);
  page.at(11000);
  assert.equal(page.agent(), AGENT);
  assert.equal(page.player.loads.length, 8);
  assert.ok(!page.watching());
  page.fire('yt-navigate-finish');
  assert.ok(!page.watching());

  // Data YouTube fetched ahead of a click is retried once with the kept
  // marker; when no edited response ever shows the tag, the next hold moves on.
  const clicked = youtube();
  clicked.at(0);
  clicked.at(1000);
  clicked.at(2000);
  clicked.at(3000);
  assert.equal(clicked.agent(), marked('lactmilli'));
  Object.assign(clicked.player, { time: 0.5, state: 1 });
  clicked.at(3250);
  assert.ok(!clicked.watching());
  const untagged = { playabilityStatus: { status: 'OK' }, playbackTracking: { videostatsPlaybackUrl: { baseUrl: 'https://www.youtube.com/api/stats/playback?referrer=https%3A%2F%2Fwww.youtube.com%2F' } } };
  Object.assign(clicked.player, { id: 'v2', time: 0, state: 3, response: untagged });
  clicked.player.loadVideoById = function (id, start) { this.loads.push([id, start]); };
  clicked.fire('yt-navigate-finish');
  clicked.at(4000);
  clicked.at(5000);
  assert.equal(clicked.agent(), marked('lactmilli'));
  assert.deepEqual(clicked.player.loads.slice(2), [['v2', 0]]);
  clicked.at(6000);
  clicked.at(7000);
  assert.equal(clicked.agent(), marked('yahi'));
  assert.deepEqual(clicked.player.loads.slice(2), [['v2', 0], ['v2', 0]]);

  // A video nobody asked to play, or one in a hidden page, is never reloaded:
  // only a page in view whose player is trying to play can be held.
  for (const state of [-1, 2, 5]) {
    const idle = youtube();
    idle.player.state = state;
    for (let ms = 0; ms <= 5000; ms += 250) { idle.at(ms); }
    assert.deepEqual(idle.player.loads, [], `state ${state}`);
    assert.equal(idle.agent(), marked('channel'));
  }
  const hidden = youtube();
  hidden.context.document.visibilityState = 'hidden';
  hidden.player.response = { playabilityStatus: { status: 'UNPLAYABLE', reason: 'answer/3037019' } };
  for (let ms = 0; ms <= 61000; ms += 1000) { hidden.at(ms); }
  assert.deepEqual(hidden.player.loads, []);
  assert.ok(!hidden.watching());
  // Shown after the watch window: it is watched again from then on.
  hidden.player.response = { playabilityStatus: { status: 'OK' } };
  hidden.context.document.visibilityState = 'visible';
  hidden.fire('visibilitychange');
  assert.ok(hidden.watching());
  hidden.at(62000);
  hidden.at(63000);
  assert.deepEqual(hidden.player.loads, [['v1', 0]]);

  // A page that has played keeps playing hidden, so its next video's hold is
  // real: a refused marker there moves on without the page being shown.
  const music = youtube();
  music.at(0);
  music.player.state = 1;
  music.player.time = 0.5;
  music.at(250);
  assert.ok(!music.watching());
  music.context.document.visibilityState = 'hidden';
  Object.assign(music.player, { id: 'v2', time: 0, state: -1, response: { playabilityStatus: { status: 'UNPLAYABLE', reason: 'answer/3037019' } } });
  music.fire('yt-navigate-finish');
  music.at(1000);
  assert.deepEqual(music.player.loads, [['v2', 0]]);
  assert.equal(music.agent(), marked('lactmilli'));
  // The refused marker's successor was asked for, so its hold moves on.
  music.player.state = 3;
  music.at(2000);
  music.at(3000);
  assert.deepEqual(music.player.loads, [['v2', 0], ['v2', 0]]);
  assert.equal(music.agent(), marked('yahi'));
  // A hidden page's next video, fetched ahead, keeps the marker that played.
  const next = youtube();
  for (const ms of [0, 1000, 2000, 3000]) { next.at(ms); }
  assert.equal(next.agent(), marked('lactmilli'));
  Object.assign(next.player, { time: 0.5, state: 1 });
  next.at(3250);
  next.context.document.visibilityState = 'hidden';
  Object.assign(next.player, { id: 'v2', time: 0, state: 3, response: { playabilityStatus: { status: 'OK' } } });
  next.fire('yt-navigate-finish');
  next.at(4000);
  next.at(5000);
  assert.equal(next.agent(), marked('lactmilli'));
  assert.deepEqual(next.player.loads.slice(2), [['v2', 0]]);

  // A playlist's video is reloaded through the playlist, so the player keeps
  // its list and place; a mix, which the player holds no list for, a list
  // whose place is another video, and a player that cannot reload a playlist
  // reload the video alone.
  const listed = (list, index, items, loadPlaylist) => {
    const page = youtube();
    page.player.time = 7;
    Object.assign(page.player, {
      playlists: [],
      getPlaylistId() { return list; },
      getPlaylistIndex() { return index; },
      getPlaylist() { return items; },
      loadPlaylist: loadPlaylist ?? function (options) { this.playlists.push(options); this.response = TAGGED; },
    });
    page.at(0);
    page.at(1000);
    return page.player;
  };
  const playlist = listed('PL1', 1, ['v0', 'v1', 'v2']);
  assert.equal(JSON.stringify(playlist.playlists), JSON.stringify([{ list: 'PL1', index: 1, startSeconds: 7 }]));
  assert.deepEqual(playlist.loads, []);
  for (const [list, index, items] of [[null, -1, undefined], ['PL1', 0, ['v0', 'v1']], ['PL1', 1, undefined]]) {
    const player = listed(list, index, items);
    assert.deepEqual(player.playlists, [], `${list} ${index}`);
    assert.deepEqual(player.loads, [['v1', 7]], `${list} ${index}`);
  }
  const failing = listed('PL1', 1, ['v0', 'v1'], () => { throw new Error('no playlist'); });
  assert.deepEqual(failing.loads, [['v1', 7]]);

  // A server-inserted ad is not a hold, and a video that has started is left alone.
  const ad = youtube();
  ad.player.stats = { ...ad.player.stats, debug_info: 'SSAP, AD, s:49' };
  for (let ms = 0; ms <= 5000; ms += 250) { ad.at(ms); }
  assert.equal(ad.agent(), marked('channel'));
  assert.deepEqual(ad.player.loads, []);
  ad.player.time = 0.5;
  ad.at(5250);
  assert.ok(!ad.watching());
  const started = youtube();
  started.player.stats = { ...started.player.stats, buffer_health_seconds: '4.20 s', resolution: '1280x720' };
  for (let ms = 0; ms <= 3000; ms += 250) { started.at(ms); }
  assert.deepEqual(started.player.loads, []);

  // YouTube's blocker error moves on at once; a URL start time is kept.
  const refused = youtube();
  refused.player.stats = {};
  refused.player.time = 42;
  refused.player.response = { playabilityStatus: { status: 'UNPLAYABLE', errorScreen: { text: 'see support.google.com/youtube/answer/3037019' } } };
  refused.at(0);
  assert.deepEqual(refused.player.loads, [['v1', 42]]);
  assert.equal(refused.agent(), marked('lactmilli'));
  // Even on data no marker reached, and again on the next.
  refused.player.response = { playabilityStatus: { status: 'UNPLAYABLE', errorScreen: { text: 'see support.google.com/youtube/answer/3037019' } } };
  refused.at(250);
  assert.equal(refused.agent(), marked('yahi'));

  // Premium is never marked; a fresh context from YouTube is marked again.
  const premium = youtube();
  vm.runInContext('var ytInitialData = { logo: "PREMIUM" };', premium.context);
  premium.at(0);
  assert.equal(premium.agent(), AGENT);
  assert.ok(!premium.watching());
  const fresh = youtube({ readyState: 'loading' });
  assert.ok(!fresh.watching());
  vm.runInContext(`ytcfg.set({ INNERTUBE_CONTEXT: { client: { userAgent: ${JSON.stringify(AGENT)} } } })`, fresh.context);
  assert.equal(fresh.agent(), marked('channel'));
  fresh.fire('DOMContentLoaded');
  assert.ok(fresh.watching());

  // Arguments it cannot read leave the page untouched.
  const context = { console, URLSearchParams, document: { addEventListener() {} }, location: { pathname: '/watch', search: '' } };
  vm.createContext(context);
  vm.runInContext(files.map(source).join('\n'), context);
  for (const args of [
    ['', 'a=b', '', '1000', 'x=y', '', 'a*=t'],
    ['channel', 'nonsense', '', '1000', 'x=y', '', 'a*=t'],
    ['channel', 'a=b', '', 'soon', 'x=y', '', 'a*=t'],
    ['channel', 'a=b', '', '1000', 'premium', '', 'a*=t'],
    ['channel', 'a=b', '', '1000', 'x=y', '', 'a=t'],
    ['channel', 'a=b', '', '1000', 'x=y', ''],
  ]) {
    context.zephiumPlayerMarker(...args);
    assert.equal(Object.getOwnPropertyDescriptor(vm.runInContext('globalThis', context), 'ytcfg'), undefined, args.join(','));
  }
}

console.log('scriptlet library: all checks passed');
