// Added after the bench's own preload in `--youtube-snackbar` only. Listens to
// the player's own "onSnackbarMessage" event through its public
// addEventListener, attached two ways: by looking for the player every 50 ms,
// as the marker scriptlet does, and by finding the player's container once
// and observing only that subtree until the player's API is ready. Each
// listener only records the call, with the player's stats then and shortly
// after, and the observer counts its callbacks. As the reference, a
// pass-through Map.prototype.has records when the player's event bus looks
// that name up, the call the hooks run found on every hold.
if (/(^|\.)youtube\.com$/.test(location.hostname)) {
  const record = { attached: [], events: [], lookups: [], observer: { containerMs: null, container: null, callbacks: 0, attachedMs: null } };
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
  const ready = (player) => player !== null && typeof player.getPlayerState === 'function';
  const listened = { poll: new WeakSet(), observer: new WeakSet() };
  const attach = (player, how) => {
    if (!ready(player) || listened[how].has(player)) return false;
    listened[how].add(player);
    const entry = { how, atMs: Math.round(performance.now()), error: null };
    record.attached.push(entry);
    try {
      player.addEventListener('onSnackbarMessage', (...args) => {
        if (record.events.length >= 60) return;
        const event = { how, args: args.length, types: args.map((arg) => typeof arg), first: brief(args[0]), at: snapshot(player), later: [] };
        record.events.push(event);
        for (const delay of [250, 500, 1000]) setTimeout(() => event.later.push(snapshot(player)), delay);
      });
    } catch (e) {
      entry.error = String(e).slice(0, 200);
    }
    return true;
  };
  const poll = setInterval(() => {
    attach(document.getElementById('movie_player'), 'poll');
    if (performance.now() > 120000) clearInterval(poll);
  }, 50);
  // The container is looked for every 50 ms until found; then only its
  // subtree is observed, and the observer disconnects once it attached.
  const containers = 'ytd-player, ytd-watch-flexy';
  const find = setInterval(() => {
    const container = document.querySelector(containers);
    if (container === null) {
      if (performance.now() > 120000) clearInterval(find);
      return;
    }
    clearInterval(find);
    record.observer.containerMs = Math.round(performance.now());
    record.observer.container = container.localName;
    const done = () => {
      record.observer.attachedMs = Math.round(performance.now());
    };
    if (attach(container.querySelector('#movie_player'), 'observer')) {
      done();
      return;
    }
    const observer = new MutationObserver(() => {
      record.observer.callbacks += 1;
      if (attach(container.querySelector('#movie_player'), 'observer')) {
        observer.disconnect();
        done();
      }
    });
    observer.observe(container, { childList: true, subtree: true });
  }, 50);
}
