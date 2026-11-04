// Runs at the start of every top-level document in every browser.
// A heavy page must not lose requests from the probe's count to the default
// buffer of 250.
performance.setResourceTimingBufferSize(10000);
// When a video first reported playing past 0 s, and the first time one did
// with no ad showing, in ms since navigation: the first `playing` or
// `timeupdate` event that qualifies, not a painted frame.
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
