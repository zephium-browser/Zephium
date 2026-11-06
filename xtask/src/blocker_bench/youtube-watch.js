// Body of an async function run on a /watch page: plays the video muted,
// watches the player for 30 s, then seeks twice. `__SECONDS__` and
// `__SEEK_WITHIN_MS__` are replaced by the caller.
const seconds = __SECONDS__;
const seekWithinMs = __SEEK_WITHIN_MS__;
const player = document.getElementById('movie_player');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
for (let i = 0; i < 40 && !document.querySelector('#movie_player video'); i += 1) await sleep(250);
const video = document.querySelector('#movie_player video');
if (!video) throw new Error('the player has no video element');
const playCalledMs = Math.round(performance.now());
try { player.mute(); player.playVideo(); } catch {}
const response = (() => { try { return player.getPlayerResponse(); } catch { return null; } })()
  ?? window.ytInitialPlayerResponse ?? null;
const visible = (selector) => [...document.querySelectorAll(selector)].some((element) => {
  const box = element.getBoundingClientRect();
  return box.width > 0 && box.height > 0 && (!element.checkVisibility || element.checkVisibility());
});
const ENFORCEMENT = 'ytd-enforcement-message-view-model, #enforcement-message, tp-yt-paper-dialog ytd-enforcement-message-view-model';
const AD_UI = '.ytp-ad-player-overlay, .ytp-ad-player-overlay-layout, .ytp-ad-text, .ytp-ad-preview-container, .ytp-skip-ad-button, .ytp-ad-skip-button-modern';
const COMPANION = '#player-ads, ytd-companion-slot-renderer, ytd-action-companion-ad-renderer, ytd-engagement-panel-section-list-renderer[target-id="engagement-panel-ads"]';
let adMs = 0, adSeen = false, adUi = false, companion = false, enforcement = false, stalledMs = 0, started = false;
// YouTube's notice for a player it slows down for a blocker.
const INTERRUPTIONS = /experiencing interruptions/i;
let interruptions = false, interruptionsText = null;
// The notice's text and what surrounds it, the first time it shows.
const notice = () => {
  const text = document.body.innerText;
  const at = text.search(INTERRUPTIONS);
  if (at >= 0) interruptionsText ??= text.slice(Math.max(0, at - 80), at + 120).replace(/\s+/g, ' ');
  return at >= 0;
};
// The player's own view: `debug_info` starts with "SSAP, AD" while the server
// plays an inserted ad, and a hold shows as no buffer at no resolution.
const innertubeUserAgent = () => { try { return window.ytcfg?.data_?.INNERTUBE_CONTEXT?.client?.userAgent ?? null; } catch { return null; } };
const userAgentAtPlay = innertubeUserAgent();
let ssapAdMs = 0, holdMs = 0, firstDebugInfo = null, debugAtFirstStall = null, contentMs = null;
const stats = () => { try { return player.getStatsForNerds?.() ?? null; } catch { return null; } };
let last = video?.currentTime ?? 0;
const step = 250;
for (let elapsed = 0; elapsed < seconds * 1000; elapsed += step) {
  await sleep(step);
  const showing = player?.classList.contains('ad-showing') || player?.classList.contains('ad-interrupting');
  if (showing) { adMs += step; adSeen = true; }
  adUi ||= visible(AD_UI);
  companion ||= visible(COMPANION);
  enforcement ||= visible(ENFORCEMENT);
  if (elapsed % 2000 === 0) interruptions ||= notice();
  const now = video?.currentTime ?? 0;
  started ||= now > last;
  const nerds = stats();
  const debug = typeof nerds?.debug_info === 'string' ? nerds.debug_info : null;
  firstDebugInfo ??= debug?.slice(0, 160) ?? null;
  if (debug?.startsWith('SSAP, AD')) ssapAdMs += step;
  if (nerds?.buffer_health_seconds === '0.00 s' && nerds?.resolution === '0x0') holdMs += step;
  if (!showing && video && !video.paused && now === last) {
    stalledMs += step;
    debugAtFirstStall ??= debug?.slice(0, 160) ?? '';
  }
  if (contentMs === null && now > 0 && !showing && !debug?.startsWith('SSAP, AD')) contentMs = playCalledMs + elapsed + step;
  last = now;
}
// A video that never played, ad or not, observed nothing, unless a blocker
// warning still stands over it at the end, which is the finding.
if (!started && !visible(ENFORCEMENT)) throw new Error('playback never started');
const played = Math.round((video?.currentTime ?? 0) * 10) / 10;
// From the seek until the player has moved past the new position, playing,
// by its own events; `null` when it did not within the limit or the video is
// too short. An ad the seek brings up counts toward the time.
const seek = (fraction) => new Promise((resolve) => {
  const duration = video.duration;
  if (!Number.isFinite(duration) || duration < 60) return resolve(null);
  const target = Math.floor(duration * fraction);
  const begun = performance.now();
  let seeked = null;
  const done = (value) => {
    clearTimeout(limit);
    video.removeEventListener('seeked', onSeeked);
    video.removeEventListener('timeupdate', onTime);
    resolve(value);
  };
  const onSeeked = () => { seeked = video.currentTime; };
  const onTime = () => {
    if (seeked !== null && !video.paused && !video.seeking && video.currentTime > seeked && video.currentTime < target + 10) {
      done(Math.round(performance.now() - begun));
    }
  };
  const limit = setTimeout(() => done(null), seekWithinMs);
  video.addEventListener('seeked', onSeeked);
  video.addEventListener('timeupdate', onTime);
  try { player.seekTo(target, true); } catch { video.currentTime = target; }
  try { player.playVideo(); } catch {}
});
const seekMs = [await seek(0.4), await seek(0.7)];
interruptions ||= notice();
const playability = (() => {
  try {
    const status = player.getPlayerResponse?.()?.playabilityStatus;
    return status ? { status: status.status ?? null, reason: String(status.reason ?? '').slice(0, 120) } : null;
  } catch { return null; }
})();
const start = window.__benchVideo ?? {};
// The page's own timeline of YouTube's API and media requests, in ms since
// navigation, with the protocol each used.
const timing = performance.getEntriesByType('resource')
  .filter((entry) => /\/youtubei\/v1\/(player|next)|googlevideo\.com\/videoplayback|\/s\/player\/.*base\.js/.test(entry.name))
  .slice(0, 60)
  .map((entry) => ({
    kind: /videoplayback/.test(entry.name) ? 'media' : /base\.js/.test(entry.name) ? 'player-js' : /\/player/.test(entry.name) ? 'player' : 'next',
    startMs: Math.round(entry.startTime),
    requestMs: Math.round(entry.requestStart),
    firstByteMs: Math.round(entry.responseStart),
    endMs: Math.round(entry.responseEnd),
    protocol: entry.nextHopProtocol || null,
    bytes: entry.transferSize,
  }));
const navigation = performance.getEntriesByType('navigation')[0];
return {
  url: location.href,
  userAgentAtPlay,
  userAgentAtEnd: innertubeUserAgent(),
  playerRequests: window.__benchPlayerRequests ?? null,
  ssapAdMs,
  holdMs,
  firstDebugInfo,
  debugAtFirstStall,
  contentSampledMs: contentMs,
  timing,
  documentMs: navigation ? { firstByte: Math.round(navigation.responseStart), domContentLoaded: Math.round(navigation.domContentLoadedEventEnd), load: Math.round(navigation.loadEventEnd), protocol: navigation.nextHopProtocol || null } : null,
  adPlacements: response?.adPlacements?.length ?? 0,
  playerAds: response?.playerAds?.length ?? 0,
  adSlots: response?.adSlots?.length ?? 0,
  adSeen,
  adMs,
  adUi,
  companion,
  enforcement,
  interruptions,
  interruptionsText,
  apiCalls: window.__benchApiCalls ?? null,
  playability,
  played,
  stalledMs,
  playCalledMs,
  firstFrameMs: start.firstFrameMs ?? null,
  contentFrameMs: start.contentFrameMs ?? null,
  seekMs,
};
