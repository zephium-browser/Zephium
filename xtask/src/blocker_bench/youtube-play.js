// Body of an async function run on a watch page in view: plays the video
// muted for `__SECONDS__` s, so the page has played before it is sent behind
// another tab.
const seconds = __SECONDS__;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let player = null;
for (let i = 0; i < 60 && !(player = document.getElementById('movie_player'))?.querySelector('video'); i += 1) await sleep(250);
const video = player?.querySelector('video');
if (!video) throw new Error('the player has no video element');
try { player.mute(); player.playVideo(); } catch {}
await sleep(seconds * 1000);
return { videoId: new URLSearchParams(location.search).get('v'), played: Math.round(video.currentTime * 10) / 10 };
