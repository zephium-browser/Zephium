// Body of an async function run on the first page of a fresh profile, a watch
// page whose consent prompt was answered on it: whether the document is the
// one the prompt was shown in, and for `__SECONDS__` s what the marker's poll
// would read every 250 ms, with the preload's player requests, marker changes
// and hold samples since the document began.
const seconds = __SECONDS__;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const consent = document.querySelector('form[action*="consent"] button');
if (location.hostname.startsWith('consent.') && consent) return { consent: true };
const navigation = performance.getEntriesByType('navigation')[0];
const document_ = {
  type: navigation?.type ?? null,
  startedMs: Math.round(performance.now()),
  referrer: document.referrer.slice(0, 120),
  readyState: document.readyState,
};
const timeline = [];
let started = false;
for (let elapsed = 0; elapsed < seconds * 1000; elapsed += 250) {
  const player = document.getElementById('movie_player');
  if (player && !started && player.getVideoData?.()?.video_id) {
    try { player.mute(); player.playVideo(); } catch {}
    started = true;
  }
  let nerds = null;
  let state = null;
  try { nerds = player?.getStatsForNerds?.() ?? null; state = player?.getPlayerState?.() ?? null; } catch {}
  const video = player?.querySelector('video');
  timeline.push({
    atMs: Math.round(performance.now()),
    visible: document.visibilityState,
    state,
    hold: nerds?.buffer_health_seconds === '0.00 s' && nerds?.resolution === '0x0',
    time: video ? Math.round(video.currentTime * 10) / 10 : null,
    paused: video?.paused ?? null,
    id: player?.getVideoData?.()?.video_id ?? null,
  });
  await sleep(250);
}
// Only the samples where something the poll reads changed.
const changes = timeline.filter((sample, at) => at === 0 || ['visible', 'state', 'hold', 'paused', 'id']
  .some((key) => sample[key] !== timeline[at - 1][key]) || (sample.time > 0 && !(timeline[at - 1].time > 0)));
return {
  url: location.href,
  document: document_,
  changes,
  heldMs: timeline.filter((sample) => sample.hold).length * 250,
  firstPlayedMs: timeline.find((sample) => sample.time > 0.1)?.atMs ?? null,
  playerRequests: (window.__benchPlayerRequests ?? []).map((request) => ({ atMs: request.atMs, tail: request.userAgentTail ?? null })),
  markers: window.__benchMarkers ?? null,
  holdStates: (window.__benchHoldStates ?? []).slice(0, 8),
  holdSamples: (window.__benchHoldStates ?? []).length,
  consent: window.__benchConsent ?? null,
  playability: (() => { try { return document.getElementById('movie_player')?.getPlayerResponse?.()?.playabilityStatus?.status ?? null; } catch { return null; } })(),
};
