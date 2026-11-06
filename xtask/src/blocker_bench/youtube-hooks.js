// Body of an async function run on a YouTube watch page in `--youtube-hooks`:
// plays the video muted for `__SECONDS__` s, timing the hold from the
// player's stats and the first frame that played, scrolls down through the
// comments in half-screen steps and back, and returns what the hooks, if this
// launch has them, counted and recorded.
const seconds = __SECONDS__;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (test, ms) => {
  for (let waited = 0; waited < ms; waited += 250) {
    const value = test();
    if (value) return value;
    await sleep(250);
  }
  return null;
};
const consent = document.querySelector('form[action*="consent"] button');
if (location.hostname.startsWith('consent.') && consent) return { consent: true };
const player = await until(() => {
  const element = document.getElementById('movie_player');
  return element?.getVideoData?.()?.video_id && element.querySelector('video') ? element : null;
}, 15000);
if (!player) throw new Error('the player did not load the video');
const video = player.querySelector('video');
try { player.mute(); player.playVideo(); } catch {}
let holdStartMs = null, holdMs = 0, playedMs = null, last = video.currentTime;
for (let elapsed = 0; elapsed < seconds * 1000; elapsed += 250) {
  await sleep(250);
  let nerds = null;
  try { nerds = player.getStatsForNerds?.() ?? null; } catch {}
  const now = Math.round(performance.now());
  if (nerds?.buffer_health_seconds === '0.00 s' && nerds?.resolution === '0x0') {
    holdStartMs ??= now;
    holdMs += 250;
  }
  const showing = player.classList.contains('ad-showing') || player.classList.contains('ad-interrupting');
  if (playedMs === null && video.currentTime > 0 && video.currentTime !== last && !showing) playedMs = now;
  last = video.currentTime;
}
const scrolled = performance.now();
for (let step = 0; step < 30; step += 1) {
  scrollBy({ top: innerHeight / 2, behavior: 'instant' });
  await sleep(500);
}
const comments = document.querySelectorAll('ytd-comment-thread-renderer').length;
scrollTo({ top: 0, behavior: 'instant' });
await sleep(1500);
const scrollMs = Math.round(performance.now() - scrolled);
const hooks = window.__benchHooks;
return {
  videoId: player.getVideoData?.()?.video_id ?? null,
  holdStartMs,
  holdMs,
  playedMs,
  played: Math.round(video.currentTime * 10) / 10,
  comments,
  scrollMs,
  pageMs: Math.round(performance.now()),
  markers: window.__benchMarkers ?? null,
  hooks: hooks ? {
    pushes: hooks.pushes,
    lookups: hooks.lookups,
    bytes: hooks.bytes,
    snackbar: hooks.snackbar,
    signals: hooks.signals,
    calibration: hooks.calibrate(),
  } : null,
};
