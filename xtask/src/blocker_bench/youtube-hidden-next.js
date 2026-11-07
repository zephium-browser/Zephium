// Body of an async function run on a watch page that played in view and is
// now behind another tab: opens the first other video the page links to by
// clicking it, as autoplay or a playlist moves on, and watches it hidden for
// `__SECONDS__` s. A hidden page's timers run about once a second, so times
// are read from the clock between samples.
const seconds = __SECONDS__;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const tail = () => {
  try {
    const agent = window.ytcfg?.get?.('INNERTUBE_CONTEXT')?.client?.userAgent ?? '';
    return /^Mozilla\/5\.0 \(([^)]*)\)/.exec(agent)?.[1].split('; ').at(-1) ?? null;
  } catch { return null; }
};
const before = new URLSearchParams(location.search).get('v');
const ADS = 'ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer, ytd-promoted-sparkles-web-renderer';
const links = () => [...document.querySelectorAll('#secondary a[href^="/watch?v="], #related a[href^="/watch?v="]')]
  .filter((element) => new URL(element.href, location.href).searchParams.get('v') !== before);
const qualifies = (element) => !new URL(element.href, location.href).searchParams.has('list') && !element.closest(ADS);
const waited = performance.now();
let link = links().find(qualifies);
while (!link && performance.now() - waited < 20000) {
  await sleep(250);
  link = links().find(qualifies);
}
if (!link) {
  const other = links();
  const candidates = {
    links: other.length,
    list: other.filter((element) => new URL(element.href, location.href).searchParams.has('list')).length,
    adSlot: other.filter((element) => element.closest(ADS)).length,
  };
  throw new Error(`the watch page links no other video: ${JSON.stringify(candidates)}`);
}
const markers = [tail()];
const requests = (window.__benchPlayerRequests ?? []).length;
const clicked = performance.now();
link.click();
let id = null, player = null, video = null;
while (performance.now() - clicked < 15000) {
  await sleep(250);
  id = new URLSearchParams(location.search).get('v');
  player = document.getElementById('movie_player');
  if (id !== before && player?.getVideoData?.()?.video_id === id) { video = player.querySelector('video'); break; }
}
if (!video) return { visibility: document.visibilityState, error: 'no in-page navigation to a video' };
let stalledMs = 0, holdMs = 0, contentFrameMs = null, last = video.currentTime, at = performance.now();
while (performance.now() - clicked < seconds * 1000) {
  await sleep(250);
  const now = performance.now();
  const step = now - at;
  at = now;
  if (markers.at(-1) !== tail()) markers.push(tail());
  let nerds = null;
  try { nerds = player.getStatsForNerds?.() ?? null; } catch {}
  if (nerds?.buffer_health_seconds === '0.00 s' && nerds?.resolution === '0x0') holdMs += step;
  const time = video.currentTime;
  if (!video.paused && time === last) stalledMs += step;
  if (contentFrameMs === null && time > 0 && time !== last) contentFrameMs = Math.round(now - clicked);
  last = time;
}
return {
  visibility: document.visibilityState,
  videoId: id,
  contentFrameMs,
  stalledMs: Math.round(stalledMs),
  holdMs: Math.round(holdMs),
  played: Math.round(video.currentTime * 10) / 10,
  rotations: markers.length - 1,
  markers,
  playerRequests: (window.__benchPlayerRequests ?? []).slice(requests).map((request) => request.userAgentTail),
};
