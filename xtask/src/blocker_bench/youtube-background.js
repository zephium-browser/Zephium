// Body of an async function run on a watch page opened behind another tab,
// after the bench waited: what the page did while nobody asked it to play.
// `reloads` counts the player requests for the page's own video, each one a
// reload, and `markers` every marker its client carried.
const player = document.getElementById('movie_player');
const video = player?.querySelector('video') ?? null;
const id = new URLSearchParams(location.search).get('v');
let playerState = null;
try { playerState = player?.getPlayerState?.() ?? null; } catch {}
return {
  url: location.href,
  visibility: document.visibilityState,
  playerState,
  paused: video ? video.paused : null,
  played: video ? Math.round(video.currentTime * 10) / 10 : null,
  reloads: (window.__benchPlayerRequests ?? []).filter((request) => request.videoId === id).length,
  rotations: Math.max(0, (window.__benchMarkers ?? []).length - 1),
  markers: window.__benchMarkers ?? null,
  holdStates: (window.__benchHoldStates ?? []).length,
  apiCalls: window.__benchApiCalls ?? null,
};
