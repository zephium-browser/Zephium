// Body of an async function run on a /watch page: plays the video muted and
// watches the player for 30 s. `__SECONDS__` is replaced by the caller.
const seconds = __SECONDS__;
const player = document.getElementById('movie_player');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
for (let i = 0; i < 40 && !document.querySelector('#movie_player video'); i += 1) await sleep(250);
const video = document.querySelector('#movie_player video');
if (!video) throw new Error('the player has no video element');
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
let adMs = 0, adSeen = false, adUi = false, companion = false, enforcement = false, stalledMs = 0;
let last = video?.currentTime ?? 0;
const step = 250;
for (let elapsed = 0; elapsed < seconds * 1000; elapsed += step) {
  await sleep(step);
  const showing = player?.classList.contains('ad-showing') || player?.classList.contains('ad-interrupting');
  if (showing) { adMs += step; adSeen = true; }
  adUi ||= visible(AD_UI);
  companion ||= visible(COMPANION);
  enforcement ||= visible(ENFORCEMENT);
  const now = video?.currentTime ?? 0;
  if (!showing && video && !video.paused && now === last) stalledMs += step;
  last = now;
}
return {
  url: location.href,
  adPlacements: response?.adPlacements?.length ?? 0,
  playerAds: response?.playerAds?.length ?? 0,
  adSlots: response?.adSlots?.length ?? 0,
  adSeen,
  adMs,
  adUi,
  companion,
  enforcement,
  played: Math.round((video?.currentTime ?? 0) * 10) / 10,
  stalledMs,
};
