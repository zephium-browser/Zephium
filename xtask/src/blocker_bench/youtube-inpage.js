// Body of an async function run on a YouTube watch page: opens the first
// other video the page links to by clicking it, then the first one that page
// links to, as a person would and without a page load, and watches each for
// `__SECONDS__` s. In these steps the player asks YouTube for each video's data, so the
// requests the preload records show what was sent, after any blocker's
// edits. The first navigation's numbers are also the result's own.
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
const visible = (element) => {
  const box = element.getBoundingClientRect();
  return box.width > 0 && box.height > 0 && (!element.checkVisibility || element.checkVisibility());
};
const ENFORCEMENT = 'ytd-enforcement-message-view-model, #enforcement-message';
const INTERRUPTIONS = /experiencing interruptions/i;
const ADS = 'ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer, ytd-promoted-sparkles-web-renderer';
// The notice's text and what surrounds it, or null.
const notice = () => {
  const text = document.body.innerText;
  const at = text.search(INTERRUPTIONS);
  return at < 0 ? null : text.slice(Math.max(0, at - 80), at + 120).replace(/\s+/g, ' ');
};
const videoId = () => new URLSearchParams(location.search).get('v');
const consent = document.querySelector('form[action*="consent"] button');
if (location.hostname.startsWith('consent.') && consent) return { consent: true };

const watch = async (open) => {
  const players = (window.__benchPlayerRequests ?? []).length;
  const nexts = (window.__benchNextRequests ?? []).length;
  const before = videoId();
  const openedMs = performance.now();
  open();
  const id = await until(() => location.pathname === '/watch' && videoId() !== before && videoId(), 15000);
  if (!id) return { error: 'no in-page navigation to a video' };
  const player = await until(() => {
    const element = document.getElementById('movie_player');
    return element?.getVideoData?.()?.video_id === id && element.querySelector('video') ? element : null;
  }, 15000);
  if (!player) return { videoId: id, error: 'the player did not load the video' };
  const video = player.querySelector('video');
  try { player.mute(); player.playVideo(); } catch {}
  let adMs = 0, adSeen = false, enforcement = false, interruptionsText = null, stalledMs = 0, holdMs = 0, ssapAdMs = 0;
  let contentFrameMs = null, debugAtFirstStall = null, last = video.currentTime;
  const step = 250;
  for (let elapsed = 0; elapsed < seconds * 1000; elapsed += step) {
    await sleep(step);
    const showing = player.classList.contains('ad-showing') || player.classList.contains('ad-interrupting');
    if (showing) { adMs += step; adSeen = true; }
    enforcement ||= [...document.querySelectorAll(ENFORCEMENT)].some(visible);
    if (elapsed % 2000 === 0) interruptionsText ??= notice();
    let nerds = null;
    try { nerds = player.getStatsForNerds?.() ?? null; } catch {}
    const debug = typeof nerds?.debug_info === 'string' ? nerds.debug_info : null;
    if (debug?.startsWith('SSAP, AD')) ssapAdMs += step;
    if (nerds?.buffer_health_seconds === '0.00 s' && nerds?.resolution === '0x0') holdMs += step;
    const now = video.currentTime;
    if (!showing && !video.paused && now === last) {
      stalledMs += step;
      debugAtFirstStall ??= debug?.slice(0, 160) ?? '';
    }
    if (contentFrameMs === null && now > 0 && now !== last && !showing && !debug?.startsWith('SSAP, AD')) {
      contentFrameMs = Math.round(performance.now() - openedMs);
    }
    last = now;
  }
  interruptionsText ??= notice();
  let playability = null;
  try {
    const status = player.getPlayerResponse?.()?.playabilityStatus;
    playability = status ? { status: status.status ?? null, reason: String(status.reason ?? '').slice(0, 120) } : null;
  } catch {}
  return {
    videoId: id,
    contentFrameMs,
    stalledMs,
    holdMs,
    ssapAdMs,
    adSeen,
    adMs,
    enforcement,
    interruptions: interruptionsText !== null,
    interruptionsText,
    playability,
    played: Math.round(video.currentTime * 10) / 10,
    debugAtFirstStall,
    userAgent: (() => { try { return window.ytcfg?.get?.('INNERTUBE_CONTEXT')?.client?.userAgent ?? null; } catch { return null; } })(),
    playerRequests: (window.__benchPlayerRequests ?? []).slice(players),
    nextRequests: (window.__benchNextRequests ?? []).slice(nexts, nexts + 1),
  };
};

const opening = await until(() => document.getElementById('movie_player')?.getVideoData?.()?.video_id, 15000);
if (!opening) throw new Error('the opening video did not load');
const seen = new Set([opening]);
const related = () => until(() => [...document.querySelectorAll('#secondary a[href^="/watch?v="], #related a[href^="/watch?v="]')]
  .find((element) => {
    const url = new URL(element.href, location.href);
    return visible(element) && !seen.has(url.searchParams.get('v')) && !url.searchParams.has('list')
      && !element.closest(ADS);
  }), 20000);
// Why no link qualified: how many the page had, visible, not yet seen, and of
// those how many were in a playlist or an ad slot.
const candidates = () => {
  const links = [...document.querySelectorAll('#secondary a[href^="/watch?v="], #related a[href^="/watch?v="]')];
  const shown = links.filter(visible);
  const unseen = shown.filter((element) => !seen.has(new URL(element.href, location.href).searchParams.get('v')));
  return {
    links: links.length,
    visible: shown.length,
    unseen: unseen.length,
    list: unseen.filter((element) => new URL(element.href, location.href).searchParams.has('list')).length,
    adSlot: unseen.filter((element) => element.closest(ADS)).length,
  };
};
const navigations = [];
for (let i = 0; i < 2; i += 1) {
  const link = await related();
  if (!link) {
    if (i === 0) throw new Error(`the watch page links no other video: ${JSON.stringify(candidates())}`);
    navigations.push({ error: 'no second link', candidates: candidates() });
    break;
  }
  const navigation = await watch(() => link.click());
  navigations.push(navigation);
  if (navigation.error) break;
  seen.add(navigation.videoId);
}
const first = navigations[0];
return {
  url: location.href,
  navigations,
  contentFrameMs: first.contentFrameMs ?? null,
  stalledMs: first.stalledMs ?? null,
  adSeen: first.adSeen ?? null,
  adMs: first.adMs ?? null,
  enforcement: navigations.some((navigation) => navigation.enforcement),
  interruptions: navigations.some((navigation) => navigation.interruptions),
  played: first.played ?? null,
  apiCalls: window.__benchApiCalls ?? null,
  holdStates: window.__benchHoldStates ?? null,
  markers: window.__benchMarkers ?? null,
  snackbar: window.__benchSnackbar ?? null,
};
