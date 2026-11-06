// Added after the bench's own preload in `--youtube-snackbar` only. Listens to
// the player's own "onSnackbarMessage" event through its public
// addEventListener and records each call with the player's stats then and
// shortly after; the listener only records. As the reference, a pass-through
// Map.prototype.has records when the player's event bus looks that name up,
// the call the hooks run found on every hold.
if (/(^|\.)youtube\.com$/.test(location.hostname)) {
  const record = { attached: [], events: [], lookups: [] };
  Object.defineProperty(window, '__benchSnackbar', { value: record });
  const nativeHas = Map.prototype.has;
  Map.prototype.has = new Proxy(nativeHas, {
    apply(target, self, args) {
      if (args[0] === 'onSnackbarMessage' && record.lookups.length < 80) {
        record.lookups.push(Math.round(performance.now()));
      }
      return Reflect.apply(target, self, args);
    },
  });
  const brief = (value) => {
    try {
      return JSON.stringify(value, (key, item) => (typeof item === 'function' ? '[function]' : item))?.slice(0, 400) ?? String(value);
    } catch (e) {
      return `[${typeof value}] ${String(e).slice(0, 80)}`;
    }
  };
  const snapshot = (player) => {
    const read = (call) => { try { return call(); } catch { return null; } };
    const stats = read(() => player.getStatsForNerds());
    const video = player.querySelector('video');
    return {
      atMs: Math.round(performance.now()),
      id: read(() => player.getVideoData().video_id),
      state: read(() => player.getPlayerState()),
      buffer: stats?.buffer_health_seconds ?? null,
      resolution: stats?.resolution ?? null,
      paused: video?.paused ?? null,
      time: video ? Math.round(video.currentTime * 10) / 10 : null,
      tagged: read(() => String(player.getPlayerResponse().playbackTracking.videostatsPlaybackUrl.baseUrl).includes('reloadxhr')),
      visible: document.visibilityState,
      path: location.pathname,
    };
  };
  const seen = new WeakSet();
  const attach = () => {
    const player = document.getElementById('movie_player');
    if (!player || seen.has(player) || typeof player.getPlayerState !== 'function') return;
    seen.add(player);
    const entry = { atMs: Math.round(performance.now()), error: null };
    record.attached.push(entry);
    try {
      player.addEventListener('onSnackbarMessage', (...args) => {
        if (record.events.length >= 40) return;
        const event = { args: args.length, types: args.map((arg) => typeof arg), first: brief(args[0]), at: snapshot(player), later: [] };
        record.events.push(event);
        for (const delay of [250, 500, 1000]) setTimeout(() => event.later.push(snapshot(player)), delay);
      });
    } catch (e) {
      entry.error = String(e).slice(0, 200);
    }
  };
  const poll = setInterval(() => {
    attach();
    if (performance.now() > 120000) clearInterval(poll);
  }, 50);
}
