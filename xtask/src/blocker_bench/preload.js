// Runs at the start of every top-level document in every browser.
// A heavy page must not lose requests from the probe's count to the default
// buffer of 250.
performance.setResourceTimingBufferSize(10000);
// When the first video frame played, and the first one that was not an ad,
// in ms since navigation, from the video element's own events.
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
