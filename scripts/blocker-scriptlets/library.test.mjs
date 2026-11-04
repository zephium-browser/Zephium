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


// The node-text scriptlets, on a minimal document: inserted nodes are
// delivered to the observer the way a parser inserts a script and then its text.
{
  const dom = () => {
    const observers = [];
    const listeners = {};
    class FakeNode {
      constructor(nodeType, nodeName, value = null) {
        this.nodeType = nodeType;
        this.nodeName = nodeName;
        this.nodeValue = value;
        this.childNodes = [];
        this.parentNode = null;
      }
      get firstChild() { return this.childNodes[0] ?? null; }
      get textContent() {
        return this.nodeType === 3 ? this.nodeValue : this.childNodes.map((child) => child.textContent).join('');
      }
      set textContent(value) {
        this.childNodes = [];
        const text = new FakeNode(3, '#text', value);
        text.parentNode = this;
        this.childNodes.push(text);
        for (const observer of observers) observer.pending.push({ addedNodes: [text] });
      }
      append(child) {
        child.parentNode = this;
        this.childNodes.push(child);
        for (const observer of observers) observer.pending.push({ addedNodes: [child] });
        return child;
      }
    }
    const root = new FakeNode(1, 'HTML');
    const document = {
      readyState: 'loading',
      documentElement: root,
      addEventListener: (type, handler) => { listeners[type] = handler; },
      createTreeWalker: (node) => {
        const all = [];
        const walk = (n) => { for (const child of n.childNodes) { all.push(child); walk(child); } };
        walk(node);
        let i = -1;
        return { get currentNode() { return all[i]; }, nextNode: () => (++i < all.length ? all[i] : null) };
      },
    };
    class MutationObserver {
      constructor(callback) { this.callback = callback; this.pending = []; this.on = false; }
      observe() { this.on = true; observers.push(this); }
      disconnect() { this.on = false; observers.splice(observers.indexOf(this), 1); }
    }
    // Delivers queued records, as the checkpoint before a parser-inserted script runs.
    const flush = () => {
      for (const observer of [...observers]) {
        const records = observer.pending.splice(0);
        if (records.length > 0) observer.callback(records);
      }
    };
    const context = {
      document, MutationObserver, Node: { TEXT_NODE: 3, ELEMENT_NODE: 1 }, NodeFilter: { SHOW_ELEMENT: 1, SHOW_TEXT: 4 },
      setTimeout: (handler) => handler(), console,
    };
    vm.createContext(context);
    vm.runInContext(['pattern.js', 'node-text.js', 'replace-node-text.js', 'remove-node-text.js'].map(source).join('\n'), context);
    const script = (text) => {
      const element = root.append(new FakeNode(1, 'SCRIPT'));
      element.append(new FakeNode(3, '#text', text));
      flush();
      return element;
    };
    return { context, script, root, FakeNode, flush, loaded: () => listeners.DOMContentLoaded?.(), observers };
  };

  // Captured groups and the includes/excludes tokens.
  {
    const page = dom();
    page.context.zephiumReplaceNodeText('script', '/(function startPlayer\\(\\))/', 'prepare();$1', 'condition', '/startPlayer|bootstrap/', 'excludes', 'prepare();');
    assert.equal(page.script('function startPlayer() {}').textContent, 'prepare();function startPlayer() {}');
    assert.equal(page.script('function other() {}').textContent, 'function other() {}');
    assert.equal(page.script('prepare();function startPlayer() {}').textContent, 'prepare();function startPlayer() {}');
  }
  // sedCount stops after that many replacements.
  {
    const page = dom();
    page.context.zephiumReplaceNodeText('script', 'ad', 'x', 'sedCount', '1');
    assert.equal(page.script('ad=1').textContent, 'x=1');
    assert.equal(page.observers.length, 0);
    assert.equal(page.script('ad=2').textContent, 'ad=2');
  }
  // It quits at DOMContentLoaded unless told to stay.
  {
    const leaving = dom();
    leaving.context.zephiumReplaceNodeText('script', 'ad', 'x');
    leaving.loaded();
    assert.equal(leaving.script('ad=1').textContent, 'ad=1');
    const staying = dom();
    staying.context.zephiumReplaceNodeText('script', 'ad', 'x', 'stay', '1');
    staying.loaded();
    assert.equal(staying.script('ad=1').textContent, 'x=1');
  }
  // Text nodes by their own name; other node types are left alone.
  {
    const page = dom();
    page.context.zephiumReplaceNodeText('#text', '/^Advertisement$/', '');
    const label = page.root.append(new page.FakeNode(3, '#text', 'Advertisement'));
    page.flush();
    assert.equal(label.nodeValue, '');
    const note = page.root.append(new page.FakeNode(8, '#comment', 'Advertisement'));
    page.flush();
    assert.equal(note.nodeValue, 'Advertisement');
  }
  // Script text goes through the scriptlet's own Trusted Types policy.
  {
    const page = dom();
    const made = [];
    page.context.window = { trustedTypes: { createPolicy: (name, rules) => { made.push(name); return { createScript: (text) => `trusted:${rules.createScript(text)}` }; } } };
    page.context.zephiumReplaceNodeText('script', 'ad', 'x');
    assert.equal(page.script('ad=1').textContent, 'trusted:x=1');
    assert.deepEqual(made, ['zephium-node-text']);
  }
  // remove-node-text empties the whole node.
  {
    const page = dom();
    page.context.zephiumRemoveNodeText('script', 'window,"fetch"');
    assert.equal(page.script('a(); Object.defineProperty(window,"fetch",{})').textContent, '');
    assert.equal(page.script('b();').textContent, 'b();');
  }
}

// The JSONPath editor: the older form, v2 queries, assignments and trust.
{
  const context = { Date };
  vm.createContext(context);
  vm.runInContext(['json-path.js'].map(source).join('\n'), context);
  const edit = (query, value, trusted = true) => [context.zephiumJsonPath(query, trusted).edit(value), value];
  const request = (agent) => ({
    signedRequest: {},
    context: { client: { clientName: 'DESKTOP', agent } },
    playback: { options: { origin: 'https://example.test/view' } },
  });
  // Older form: filters test the node they follow.
  assert.deepEqual(
    edit('[?.signedRequest][?.context.client.agent*="tag"].context.client[?.clientName=="DESKTOP"]+={"screen":"FULL"}', request('a; tag'))[1].context.client,
    { clientName: 'DESKTOP', agent: 'a; tag', screen: 'FULL' },
  );
  assert.equal(edit('[?.signedRequest][?.context.client.agent*="tag"]+={"x":1}', request('plain'))[0], false);
  assert.match(edit('[?.signedRequest][?.context.client.agent=/tag|mark/].playback.options.origin="${now}"', request('mark'))[1].playback.options.origin, /^\d{13}$/);
  assert.equal(edit('[?.signedRequest][?.context.client.agent=/tag|mark/].playback.options.origin="${now}"', request('none'))[0], false);
  assert.equal(
    edit('[?.signedRequest].playback.options.origin=repl({"regex":"(?:#again)?$","replacement":"#again"})', request('x'))[1].playback.options.origin,
    'https://example.test/view#again',
  );
  assert.equal(
    edit('.context.client.agent=repl({"pattern":"; tag","replacement":""})', request('a; tag'))[1].context.client.agent,
    'a',
  );
  assert.equal(edit('[?!.signedRequest].context', request('x'))[0], false);
  // Untrusted queries only remove; an assignment does nothing.
  assert.deepEqual(edit('[?.signedRequest].context', request('x'), false)[1].context, undefined);
  assert.equal(edit('[?.signedRequest].context.client.agent="b"', request('x'), false)[0], false);
  // v2 filters select children.
  assert.deepEqual(edit('v2:$.shelf.items[?@.price>10]', { shelf: { items: [{ price: 5 }, { price: 20 }] } }, false)[1], { shelf: { items: [{ price: 5 }] } });
  assert.deepEqual(edit('$..items[?@.name^="E"].price=0', { shelf: { items: [{ name: 'Eve', price: 5 }, { name: 'Bo', price: 9 }] } })[1].shelf.items, [{ name: 'Eve', price: 0 }, { name: 'Bo', price: 9 }]);
  assert.deepEqual(edit('..section.contents.*[?.promoted]', { a: { section: { contents: [{ promoted: 1 }, { video: 1 }] } } }, false)[1], { a: { section: { contents: [{ video: 1 }] } } });
  assert.deepEqual(edit('$.list[-1]', { list: [1, 2, 3] }, false)[1], { list: [1, 2] });
  // Malformed queries edit nothing.
  for (const query of ['bad[[', '[?.a', '.a=repl({"regex":"("})', '.a+=[1]', '']) {
    assert.equal(edit(query, { a: 1 })[0], false, query);
  }
}

console.log('scriptlet library: all checks passed');
