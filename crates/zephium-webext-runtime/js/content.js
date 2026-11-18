// Content-script worlds. `boot` runs once per document in the install's
// isolated world before any content script: it captures the bridge and
// publishes a guard that each script bundle calls first. The guard decides
// with Chrome's matcher (mirroring `ContentScriptMatcher::matches`) against
// this frame's URL, or its creator's for about:blank/srcdoc and
// `match_origin_as_fallback`; the API is created when a script first runs.
//
// Threat model. The page shares the DOM but not this world's globals, so it
// cannot replace anything below; it can dispatch synthetic events (ignored:
// only trusted `load` counts), clobber named window properties (nothing is
// looked up through the window after boot), and navigate same-document
// (the guard reads the URL when the script runs, as Chrome does). Extension
// code in this world can replace built-ins after boot; the binding only uses
// the intrinsics captured at boot. MAIN-world bundles evaluate `matches` in
// the page's world, which the page controls; they get no API.
(function content(Z) {
  'use strict';
  const I = Z.I;

  const MAX_MATCH_URL_BYTES = 32 * 1024;
  const MAX_GLOB_URL_BYTES = 2 * MAX_MATCH_URL_BYTES;
  const IDLE_TIMEOUT_MS = 200;
  const DEFAULT_PORTS = { __proto__: null, 'http:': 80, 'https:': 443, 'ws:': 80, 'wss:': 443, 'ftp:': 21 };
  const FALLBACK_SCHEMES = { __proto__: null, 'about:': true, 'data:': true, 'blob:': true, 'filesystem:': true };
  const GUARD_KEY = I.SymbolFor('zephium.content.v1');

  const startsWith = (text, prefix) => I.StringStartsWith(text, prefix);
  const endsWith = (text, suffix) => I.StringEndsWith(text, suffix);

  const parseUrl = (href) => {
    try {
      return new I.URL(href);
    } catch {
      return null;
    }
  };

  // Path plus query, without the fragment, as `Url` slices it: an empty query
  // still keeps its `?`.
  function pathAndQuery(url) {
    const href = I.URLHref(url);
    const hash = I.StringIndexOf(href, '#');
    const noHash = hash === -1 ? href : I.StringSlice(href, 0, hash);
    const search = I.URLSearch(url);
    const question = I.StringIndexOf(noHash, '?');
    return I.URLPathname(url) + (search !== '' ? search : question !== -1 ? '?' : '');
  }

  // `*`-only glob over the path (`LinearGlob`), with Chrome's `/dir/*` also
  // matching `/dir`.
  function pathMatches(pattern, input) {
    if (input.length > MAX_MATCH_URL_BYTES) return false;
    if (I.StringIndexOf(pattern, '*') === -1) return pattern === input;
    if (endsWith(pattern, '/*') && pattern.length > 2 && input === I.StringSlice(pattern, 0, pattern.length - 2))
      return true;
    return wildcard(pattern, input, false);
  }

  // Greedy leftmost placement of `*`-separated segments; `?` is a one-byte
  // wildcard only for globs (`UrlGlob`).
  function wildcard(pattern, text, questionMarks) {
    const segments = [];
    let start = 0;
    for (let i = 0; i <= pattern.length; i++) {
      if (i === pattern.length || pattern[i] === '*') {
        if (i > start) I.ArrayPush(segments, I.StringSlice(pattern, start, i));
        start = i + 1;
      }
    }
    const leading = pattern.length > 0 && pattern[0] === '*';
    const trailing = pattern.length > 0 && pattern[pattern.length - 1] === '*';
    const at = (segment, position) => {
      if (position < 0 || position + segment.length > text.length) return false;
      for (let i = 0; i < segment.length; i++) {
        const expected = segment[i];
        if (expected !== text[position + i] && !(questionMarks && expected === '?')) return false;
      }
      return true;
    };
    if (!leading && !trailing && segments.length <= 1) {
      return segments.length === 0 ? text.length === 0 : segments[0].length === text.length && at(segments[0], 0);
    }
    let first = 0;
    let last = segments.length;
    let cursor = 0;
    let end = text.length;
    if (!leading) {
      if (segments.length === 0) return true;
      if (!at(segments[0], 0)) return false;
      cursor = segments[0].length;
      first = 1;
    }
    if (!trailing && last > first) {
      const tail = segments[last - 1];
      if (tail.length > end - cursor || !at(tail, end - tail.length)) return false;
      end -= tail.length;
      last--;
    }
    for (let s = first; s < last; s++) {
      const segment = segments[s];
      let found = -1;
      for (let p = cursor; p + segment.length <= end; p++) {
        if (at(segment, p)) {
          found = p;
          break;
        }
      }
      if (found === -1) return false;
      cursor = found + segment.length;
    }
    return true;
  }

  const isIpv4 = (host) => {
    let dots = 0;
    for (let i = 0; i < host.length; i++) {
      const code = I.StringCharCodeAt(host, i);
      if (code === 0x2e) dots++;
      else if (code < 0x30 || code > 0x39) return false;
    }
    return dots === 3;
  };
  const isDomain = (host) => host !== '' && host[0] !== '[' && !isIpv4(host);

  // Parses a canonical pattern as `MatchPattern::as_str` prints it.
  function compilePattern(source) {
    if (source === '<all_urls>') return { __proto__: null, all: true };
    if (typeof source !== 'string') return null;
    const separator = I.StringIndexOf(source, '://');
    if (separator === -1) return null;
    const scheme = I.StringSlice(source, 0, separator);
    const rest = I.StringSlice(source, separator + 3);
    const slash = I.StringIndexOf(rest, '/');
    if (slash === -1) return null;
    const authority = I.StringSlice(rest, 0, slash);
    const path = I.StringSlice(rest, slash);
    let host = null;
    let port = null;
    if (scheme === 'file') {
      if (authority !== '') return null;
    } else {
      host = authority;
      const close = I.StringLastIndexOf(authority, ']');
      const colon = I.StringLastIndexOf(authority, ':');
      if (colon > close) {
        host = I.StringSlice(authority, 0, colon);
        port = +I.StringSlice(authority, colon + 1);
        if (!I.isInteger(port) || port < 0 || port > 65535) return null;
      }
    }
    if (scheme !== 'http' && scheme !== 'https' && scheme !== 'file' && scheme !== '*') return null;
    return { __proto__: null, all: false, scheme, host, port, path };
  }

  function patternMatches(pattern, target) {
    if (pattern.all) return target.scheme === 'http' || target.scheme === 'https' || target.scheme === 'file';
    if (
      pattern.scheme === '*' ? target.scheme !== 'http' && target.scheme !== 'https' : pattern.scheme !== target.scheme
    )
      return false;
    if (pattern.host !== null) {
      const host = target.host;
      if (pattern.host === '*') {
        if (host === '') return false;
      } else if (startsWith(pattern.host, '*.')) {
        const base = I.StringSlice(pattern.host, 2);
        if (!isDomain(host)) return false;
        if (host !== base && !(endsWith(host, base) && host[host.length - base.length - 1] === '.')) return false;
      } else if (host !== pattern.host) {
        return false;
      }
    }
    if (pattern.port !== null && target.port !== pattern.port) return false;
    return pathMatches(pattern.path, target.path);
  }

  function targetOf(url) {
    const protocol = I.URLProtocol(url);
    const portText = I.URLPort(url);
    let host = I.URLHostname(url);
    let href = I.URLHref(url);
    if (endsWith(host, '.') && host.length > 1 && isDomain(host)) {
      const stripped = I.StringSlice(host, 0, host.length - 1);
      const copy = new I.URL(href);
      I.URLSetHostname(copy, stripped);
      href = I.URLHref(copy);
      host = stripped;
    }
    const known = DEFAULT_PORTS[protocol];
    return {
      __proto__: null,
      scheme: I.StringSlice(protocol, 0, protocol.length - 1),
      host,
      port: portText !== '' ? +portText : known === undefined ? null : known,
      path: pathAndQuery(url),
      href,
    };
  }

  const isGallery = (target) =>
    target.host === 'chromewebstore.google.com' ||
    (target.host === 'chrome.google.com' &&
      (target.path === '/webstore' || startsWith(target.path, '/webstore/') || startsWith(target.path, '/webstore?')));

  // Compiles a match declaration: canonical `matches`/`excludeMatches`,
  // `includeGlobs`/`excludeGlobs`, the two related-frame options, `allFrames`
  // and the person's file-access choice.
  function compile(data) {
    const list = (value, map) => {
      const out = [];
      if (!I.isArray(value)) return out;
      for (let i = 0; i < value.length; i++) {
        const compiled = map(value[i]);
        if (compiled !== null) I.ArrayPush(out, compiled);
      }
      return out;
    };
    const keep = (value) => (typeof value === 'string' ? value : null);
    const field = (key) => (data !== null && typeof data === 'object' && I.hasOwn(data, key) ? data[key] : undefined);
    return {
      __proto__: null,
      matches: list(field('matches'), compilePattern),
      excludeMatches: list(field('excludeMatches'), compilePattern),
      includeGlobs: list(field('includeGlobs'), keep),
      excludeGlobs: list(field('excludeGlobs'), keep),
      matchAboutBlank: field('matchAboutBlank') === true,
      matchOriginAsFallback: field('matchOriginAsFallback') === true,
      allFrames: field('allFrames') === true,
      fileAccess: field('fileAccess') === true,
    };
  }

  const originOf = (href) => {
    const url = parseUrl(href);
    if (url === null) return 'null';
    const origin = I.URLOrigin(url);
    return I.URLProtocol(url) === 'file:' ? 'null' : origin;
  };

  // `frame` describes the document: `href`, `origin` (the document's
  // security origin), `top` (is the top frame) and `creator()` (the closest
  // parent or opener whose URL is not about:, or null).
  function decide(compiled, frame) {
    if (!compiled.allFrames && !frame.top) return false;
    let url = parseUrl(frame.href);
    if (url === null) return false;
    const protocol = I.URLProtocol(url);
    if (compiled.matchOriginAsFallback && FALLBACK_SCHEMES[protocol] === true) {
      if (frame.origin !== 'null' && frame.origin !== '') url = parseUrl(`${frame.origin}/`) || url;
    } else if (compiled.matchAboutBlank && protocol === 'about:') {
      const creator = frame.creator();
      const creatorUrl = creator === null ? null : parseUrl(creator);
      if (creatorUrl !== null && I.URLProtocol(creatorUrl) !== 'about:') {
        const creatorOrigin = originOf(creator);
        if (creatorOrigin !== 'null' && creatorOrigin === frame.origin) url = creatorUrl;
      }
    }
    const target = targetOf(url);
    if (target.href.length > MAX_GLOB_URL_BYTES) return false;
    if (target.scheme === 'file' && !compiled.fileAccess) return false;
    if (isGallery(target)) return false;
    if (target.path.length > MAX_MATCH_URL_BYTES) return false;
    let included = false;
    for (let i = 0; i < compiled.matches.length && !included; i++)
      included = patternMatches(compiled.matches[i], target);
    if (!included) return false;
    for (let i = 0; i < compiled.excludeMatches.length; i++)
      if (patternMatches(compiled.excludeMatches[i], target)) return false;
    if (compiled.includeGlobs.length > 0) {
      let globbed = false;
      for (let i = 0; i < compiled.includeGlobs.length && !globbed; i++)
        globbed = wildcard(compiled.includeGlobs[i], target.href, true);
      if (!globbed) return false;
    }
    for (let i = 0; i < compiled.excludeGlobs.length; i++)
      if (wildcard(compiled.excludeGlobs[i], target.href, true)) return false;
    return true;
  }

  // The live frame as this world sees it, read through accessors captured
  // at boot.
  function frameReader(global) {
    const read = (object, key) => {
      for (let target = object; target !== null && target !== undefined; target = I.getPrototypeOf(target)) {
        const descriptor = I.getOwnPropertyDescriptor(target, key);
        if (descriptor) return descriptor.get ? I.uncurry(descriptor.get) : (window) => window[key];
      }
      return (window) => window[key];
    };
    const location = global.location;
    const hrefOf = location ? read(location, 'href') : null;
    const originGetter = read(global, 'origin');
    const parentGetter = read(global, 'parent');
    const topGetter = read(global, 'top');
    const openerGetter = read(global, 'opener');
    const href = () => (location ? I.String(hrefOf(location)) : '');
    const relative = (getter, window) => {
      try {
        return getter(window);
      } catch {
        return null;
      }
    };
    const hrefOfWindow = (window) => {
      try {
        return I.String(window.location.href);
      } catch {
        return null;
      }
    };
    return {
      __proto__: null,
      get href() {
        return href();
      },
      get origin() {
        const origin = originGetter(global);
        return typeof origin === 'string' ? origin : originOf(href());
      },
      get top() {
        const top = relative(topGetter, global);
        return top === null || top === global;
      },
      creator() {
        let window = global;
        for (let depth = 0; depth < 64; depth++) {
          const parent = relative(parentGetter, window);
          const next = parent !== null && parent !== window ? parent : relative(openerGetter, window);
          if (next === null || next === undefined) return null;
          const nextHref = hrefOfWindow(next);
          if (nextHref === null) return null;
          if (!startsWith(nextHref, 'about:')) return nextHref;
          window = next;
        }
        return null;
      },
    };
  }

  // Runs `fn` at Chrome's document_idle: the first of window load and
  // DOMContentLoaded plus 200 ms, or soon if the document already loaded.
  function scheduleIdle(global, fn) {
    let done = false;
    let timer = null;
    let listening = false;
    const run = () => {
      if (done) return;
      done = true;
      if (timer !== null) I.clearTimeout(timer);
      if (listening) I.removeEventListener(global, 'load', onLoad, true);
      try {
        I.apply(fn, global, []);
      } catch (error) {
        I.consoleError(error);
      }
    };
    const onLoad = (event) => {
      if (event && event.isTrusted === true) run();
    };
    const state = I.DocumentReadyState && global.document ? I.DocumentReadyState(global.document) : 'complete';
    if (state === 'complete') {
      timer = I.setTimeout(run, 0);
      return;
    }
    if (I.addEventListener) {
      I.addEventListener(global, 'load', onLoad, true);
      listening = true;
    }
    timer = I.setTimeout(run, IDLE_TIMEOUT_MS);
  }

  // Boots the isolated world. Bundles then call
  // `globalThis[Symbol.for('zephium.content.v1')].run(data)` (start/end)
  // or `.idle(data, function () { ... })` (idle).
  function boot(surface, bootstrap, options) {
    const option = (key) =>
      options !== null && typeof options === 'object' && I.hasOwn(options, key) ? options[key] : undefined;
    const global = option('global') || I.global;
    const link = option('link') !== undefined ? option('link') : Z.transport.nativeLink(global);
    const frame = option('frame') || frameReader(global);
    let context = null;
    const ensure = () => {
      if (context === null) context = Z.api.create(surface, bootstrap, link, { global });
      return context;
    };
    const allowed = (data) => {
      try {
        return decide(compile(data), frame);
      } catch {
        return false;
      }
    };
    const guard = I.freeze({
      __proto__: null,
      run(data) {
        if (!allowed(data)) return false;
        ensure();
        return true;
      },
      idle(data, fn) {
        if (typeof fn !== 'function' || !allowed(data)) return false;
        ensure();
        scheduleIdle(global, fn);
        return true;
      },
    });
    try {
      I.defineProperty(global, GUARD_KEY, { value: guard, enumerable: false, configurable: false, writable: false });
    } catch {
      // Booted twice in one world: the first guard stays.
    }
    return {
      __proto__: null,
      guard,
      get context() {
        return context;
      },
    };
  }

  return I.freeze({
    __proto__: null,
    boot,
    compile,
    decide,
    matches: (data, frame) => decide(compile(data), frame || frameReader(I.global)),
    frameReader,
    scheduleIdle,
    GUARD_KEY,
  });
})
