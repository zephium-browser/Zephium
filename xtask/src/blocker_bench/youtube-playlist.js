// Body of an async function run on a YouTube watch page opened with a
// playlist (`&list=`): records the player's playlist context after the page's
// own start, after each way of reloading the video a held start may use, and
// whether the next item plays in order when the video ends. Loop is switched
// on before the reloads, so the record shows whether it survives them.
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
const call = (name, ...args) => {
  try { return player[name]?.(...args) ?? null; } catch { return null; }
};
const panel = () => document.querySelector('ytd-playlist-panel-renderer:not([hidden])') ?? document.querySelector('ytd-playlist-panel-renderer');
// The panel's items in order, as video ids.
const items = () => {
  const ids = [...(panel()?.querySelectorAll('ytd-playlist-panel-video-renderer a[href*="/watch?v="]') ?? [])]
    .map((link) => new URL(link.href, location.href).searchParams.get('v'));
  return ids.filter((id, at) => id && ids.indexOf(id) === at);
};
// The panel's loop and shuffle controls, by their labels and pressed state.
const toggles = () => [...(panel()?.querySelectorAll('button[aria-label]') ?? [])]
  .map((button) => ({ label: button.getAttribute('aria-label'), pressed: button.getAttribute('aria-pressed') }))
  .filter(({ label }) => /loop|repeat|shuffle/i.test(label ?? ''));
const context = () => {
  const params = new URLSearchParams(location.search);
  const listed = items();
  const videoId = call('getVideoData')?.video_id ?? null;
  return {
    videoId,
    playlistId: call('getPlaylistId'),
    playlistIndex: call('getPlaylistIndex'),
    playlistLength: (() => { const list = call('getPlaylist'); return Array.isArray(list) ? list.length : null; })(),
    urlList: params.get('list'),
    urlIndex: params.get('index'),
    panelItems: listed.length,
    panelAt: listed.indexOf(videoId),
    toggles: toggles(),
    time: Math.round(video.currentTime * 10) / 10,
  };
};
const playing = (from) => until(() => !video.paused && video.currentTime > from + 0.5, 20000);
// Seeks to just before the end and waits for the player to move on.
const ending = async () => {
  const before = context();
  const expected = items()[before.panelAt + 1] ?? null;
  const duration = call('getDuration') ?? video.duration;
  call('seekTo', Math.max(0, duration - 3), true);
  call('playVideo');
  const next = await until(() => {
    const id = call('getVideoData')?.video_id;
    return id && id !== before.videoId ? id : null;
  }, 30000);
  if (next) await playing(0);
  await sleep(1000);
  return { expected, next, inOrder: next !== null && next === expected, after: context() };
};
// Reloads the current item with `how`, as a held start would, then lets it end.
const reload = async (how) => {
  await playing(0);
  const before = context();
  const at = Math.floor(video.currentTime);
  let error = null;
  try { await how(before, at); } catch (e) { error = String(e).slice(0, 200); }
  await playing(at);
  await sleep(2000);
  const after = context();
  return { before, after, error, kept: after.playlistId !== null && after.playlistId === before.playlistId, ended: await ending() };
};

try { player.mute(); player.playVideo(); } catch {}
const requests = () => (window.__benchPlayerRequests ?? []).length;
await playing(0);
await sleep(1000);
const start = { requests: requests(), markers: window.__benchMarkers ?? null, context: context() };
const natural = await ending();
const loop = toggles().find(({ label }) => /loop|repeat/i.test(label));
if (loop && loop.pressed !== 'true') {
  [...panel().querySelectorAll('button[aria-label]')].find((button) => button.getAttribute('aria-label') === loop.label)?.click();
  await sleep(500);
}
const byId = await reload((c, at) => player.loadVideoById(c.videoId, at));
const byPlaylist = await reload((c, at) => player.loadPlaylist({ list: c.playlistId ?? c.urlList, index: c.playlistIndex ?? c.panelAt, startSeconds: at }));
const byIdRestored = await reload(async (c, at) => {
  player.loadVideoById(c.videoId, at);
  await playing(at);
  const manager = document.querySelector('yt-playlist-manager');
  const data = manager?.getPlaylistData?.();
  if (call('getPlaylistId') === null && data) {
    manager.setPlaylistData?.(data);
    manager.setPlayerPlaybackControlData?.({ playlistPanelRenderer: data });
  }
});
return { url: location.href, start, natural, byId, byPlaylist, byIdRestored };
