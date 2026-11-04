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
  'trusted-prevent-dom-bypass.js', 'no-xhr-if.js'];

const page = (fetchBody) => {
  class FakeXhr extends EventTarget {
    open(method, url) { this.method = method; this.url = url; }
    send() {}
  }
  for (const name of ['responseText', 'response']) {
    Object.defineProperty(FakeXhr.prototype, name, {
      configurable: true,
      enumerable: true,
      get() { return name === 'response' && this.responseType === 'json' ? JSON.parse(this.body) : this.body; },
    });
  }
  // Each page wraps its own Response, as each document does.
  const PageResponse = class Response extends globalThis.Response {};
  const timers = [];
  const context = {
    Response: PageResponse, Request, Headers, XMLHttpRequest: FakeXhr, Event, TextEncoder, Blob, console, timers,
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
}

// no-xhr-if: matching requests never leave and complete with the directive's body.
{
  const context = page('{}');
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
  await Promise.resolve();
  assert.deepEqual(events, ['readystatechange', 'load', 'loadend']);
  assert.equal(atr.readyState, 4);
  assert.equal(atr.status, 200);
  assert.equal(atr.responseText, '');
  assert.equal(atr.responseURL, 'https://www.youtube.com/api/stats/atr?ns=yt&rt=1.0&docid=abc');
  const get = open('GET', 'https://www.youtube.com/api/stats/atr?ns=yt&docid=abc');
  get.send();
  await Promise.resolve();
  assert.equal(get.readyState, undefined);
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
  class FakeElement {
    constructor(children = []) { this.children = children; }
    querySelectorAll() { return this.children; }
  }
  class FakeFrame extends FakeElement {
    constructor(view) { super(); this.contentWindow = null; this.view = view; }
  }
  const makeNode = () => {
    const Node = function Node() {};
    Node.prototype.appendChild = function appendChild(child) {
      for (const frame of [child, ...child.querySelectorAll()]) {
        if (frame instanceof FakeFrame) { frame.contentWindow = frame.view; }
      }
      return child;
    };
    return Node;
  };
  const Node = makeNode();
  Object.assign(parent, { Node, HTMLElement: FakeElement });
  parent.fetch = new Proxy(parent.fetch, { apply: () => Promise.resolve('patched') });
  parent.zephiumTrustedPreventDomBypass('Node.prototype.appendChild', 'fetch');
  const frame = new FakeFrame(frameWindow);
  new Node().appendChild(frame);
  assert.equal(await frameWindow.fetch('/youtubei/v1/player'), 'patched');
  // A dotted target, on a frame inside the inserted element.
  const parsing = page('{}');
  const ParsingNode = makeNode();
  Object.assign(parsing, { Node: ParsingNode, HTMLElement: FakeElement });
  vm.runInContext('JSON.parse = new Proxy(JSON.parse, { apply: () => "parent" });', parsing);
  parsing.zephiumTrustedPreventDomBypass('Node.prototype.appendChild', 'JSON.parse');
  const nestedWindow = vm.createContext({});
  const nested = new FakeFrame(vm.runInContext('globalThis', nestedWindow));
  new ParsingNode().appendChild(new FakeElement([nested]));
  assert.equal(vm.runInContext('JSON.parse("1")', nestedWindow), 'parent');
  const whole = page('{}');
  const WholeNode = makeNode();
  Object.assign(whole, { Node: WholeNode, HTMLElement: FakeElement, document: {} });
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

console.log('scriptlet library: all checks passed');
