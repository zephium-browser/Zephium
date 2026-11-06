// Runs at the start of every top-level document in every browser.
// A heavy page must not lose requests from the probe's count to the default
// buffer of 250.
performance.setResourceTimingBufferSize(10000);
// When a video first reported playing past 0 s, and the first time one did
// with no ad showing, in ms since navigation: the first `playing` or
// `timeupdate` event that qualifies, not a painted frame.
(() => {
  const record = { firstFrameMs: null, contentFrameMs: null };
  Object.defineProperty(window, '__benchVideo', { value: record });
  const ad = () => document.querySelector('#movie_player.ad-showing, #movie_player.ad-interrupting') !== null;
  const frame = (event) => {
    const video = event.target;
    if (!(video instanceof HTMLVideoElement) || video.paused || video.currentTime <= 0) return;
    const now = Math.round(performance.now());
    record.firstFrameMs ??= now;
    if (record.contentFrameMs === null && !ad()) record.contentFrameMs = now;
  };
  document.addEventListener('playing', frame, true);
  document.addEventListener('timeupdate', frame, true);
})();
// On YouTube, what each player request asked for, read from its JSON body as
// sent, after anything a blocker edited: the fields uBlock Origin's quick
// fixes set to ask for no server-inserted ad; and the user agent of the first
// `next` requests, which carry comments and continuations. Reads only; the
// calls go on unchanged.
if (/(^|\.)youtube\.com$/.test(location.hostname)) {
  const requests = [];
  const nexts = [];
  Object.defineProperty(window, '__benchPlayerRequests', { value: requests });
  Object.defineProperty(window, '__benchNextRequests', { value: nexts });
  const PLAYER = /\/youtubei\/v1\/player(?:\?|$)/;
  const NEXT = /\/youtubei\/v1\/next(?:\?|$)/;
  const tail = (client) => /^Mozilla\/5\.0 \(([^)]*)\)/.exec(String(client.userAgent ?? ''))?.[1].split('; ').at(-1) ?? null;
  const note = (url, body) => {
    const player = PLAYER.test(String(url));
    if (typeof body !== 'string' || !(player || (NEXT.test(String(url)) && nexts.length < 20))) return;
    let value;
    try { value = JSON.parse(body); } catch { return; }
    const client = value?.context?.client ?? {};
    if (!player) {
      nexts.push({ atMs: Math.round(performance.now()), userAgentTail: tail(client), clientScreen: client.clientScreen ?? null });
      return;
    }
    const playback = value?.playbackContext?.contentPlaybackContext ?? {};
    requests.push({
      atMs: Math.round(performance.now()),
      videoId: value?.videoId ?? null,
      clientScreen: client.clientScreen ?? null,
      params: value?.params ?? null,
      inlineNoAd: playback.isInlinePlaybackNoAd ?? null,
      lactMilliseconds: playback.lactMilliseconds ?? null,
      refererReload: String(playback.referer ?? '').endsWith('#reloadxhr'),
      // The last item of the platform part, where the quick fixes add a marker.
      userAgentTail: tail(client),
      attestation: Object.hasOwn(value ?? {}, 'attestationRequest'),
      playbackKeys: Object.keys(playback).sort(),
    });
  };
  window.fetch = new Proxy(window.fetch, {
    apply(target, self, args) {
      try {
        const [input, init] = args;
        if (input instanceof Request) {
          if ((PLAYER.test(input.url) || NEXT.test(input.url)) && init?.body === undefined) {
            input.clone().text().then((body) => note(input.url, body), () => {});
          } else note(input.url, init?.body);
        } else note(input, init?.body);
      } catch {}
      return Reflect.apply(target, self, args);
    },
  });
  const urls = new WeakMap();
  const prototype = XMLHttpRequest.prototype;
  prototype.open = new Proxy(prototype.open, {
    apply(target, self, args) {
      try { urls.set(self, String(args[1])); } catch {}
      return Reflect.apply(target, self, args);
    },
  });
  prototype.send = new Proxy(prototype.send, {
    apply(target, self, args) {
      try { note(urls.get(self) ?? '', args[0]); } catch {}
      return Reflect.apply(target, self, args);
    },
  });
}
