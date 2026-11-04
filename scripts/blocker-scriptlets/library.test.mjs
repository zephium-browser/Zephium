// Runs the scriptlet library in a sandbox with stand-ins for the page APIs it
// wraps, on synthetic YouTube-shaped payloads.
import { readFileSync } from 'node:fs';
import { strict as assert } from 'node:assert';
import vm from 'node:vm';

const library = new URL('../../crates/zephium-blocker/src/scriptlets/', import.meta.url);
const source = (name) => readFileSync(new URL(name, library), 'utf8')
  .split('\n').filter((line) => !line.startsWith('//')).join('\n');
const files = ['pattern.js', 'json-pruner.js', 'request-matcher.js', 'timer-booster.js', 'json-prune.js',
  'json-prune-fetch-response.js', 'json-prune-xhr-response.js', 'nano-settimeout-booster.js',
  'nano-setinterval-booster.js'];

const page = (fetchBody) => {
  class FakeXhr {
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
    Response: PageResponse, Request, Headers, XMLHttpRequest: FakeXhr, console, timers,
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
