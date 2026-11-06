// Zephium's own scriptlet, run only from Zephium's built-in rules: puts a marker into YouTube's client user agent, which the player-request edits of uBlock Origin's quick fixes are keyed on, and moves to the next marker when the player holds a video with no buffer before it starts. A hold counts against a marker only when the held data came from a request the quick fixes edited, which their tag in the response's `path*=text` shows; other data, inlined in the page or fetched by YouTube ahead of a click, is retried with the same marker, at most twice per video. A player counts as trying to play while playing or buffering, or unstarted with its video asked to play, as after a consent dialog stopped it. A playlist's video is reloaded through the playlist when the player holds that list, so its place there is kept. The marker that last played is kept for the page's next videos; when every marker has held, the list starts over once, then the page is left unmarked. Every value YouTube may change is an argument: the markers in order, the hold's stats fields (`key=value`, `|`-separated), the stats that excuse it (`key^=prefix`), how long a hold lasts before the next marker, a Premium page's `path=value`, text that marks a playability error as a block, and the response's tag of an edited request, and the player event that announces a hold, which ends the wait at once, once per video, when the player's stats show the hold. See docs/adblock.md.
function zephiumPlayerMarker(markers, hold, excuse, holdMs, premium, blockText, requested, holdEvent) {
    const names = String(markers ?? '').split(/\s+/).filter((name) => /^[a-z0-9][a-z0-9-]*$/.test(name));
    const pairs = (spec, operator) => String(spec ?? '').split('|').map((pair) => {
        const at = pair.indexOf(operator);
        return at > 0 ? [pair.slice(0, at), pair.slice(at + operator.length)] : undefined;
    });
    const held = pairs(hold, '=');
    const excused = String(excuse ?? '') === '' ? [] : pairs(excuse, '^=');
    const wait = Number(holdMs);
    const premiumAt = String(premium ?? '').indexOf('=');
    const blocked = String(blockText ?? '');
    const tagAt = String(requested ?? '').indexOf('*=');
    const announced = String(holdEvent ?? '');
    if (names.length === 0 || held.includes(undefined) || excused.includes(undefined) || !Number.isFinite(wait) || wait < 0 || premiumAt <= 0 || tagAt <= 0 || !/^[A-Za-z]*$/.test(announced)) {
        return;
    }
    const tagPath = String(requested).slice(0, tagAt).split('.');
    const tag = String(requested).slice(tagAt + 2);
    const premiumPath = String(premium).slice(0, premiumAt).split('.');
    const premiumValue = String(premium).slice(premiumAt + 1);
    const platform = /^(Mozilla\/5\.0 \([^)]*)/;
    // Watching ends a minute after a video is asked for: a hold that long is not one this can end.
    const WATCH_MS = 60000;
    // A navigation that never finished, as one given up, stops guarding the player after this long.
    const NAVIGATE_MS = 10000;
    let index = 0;
    let restarted = false;
    // Whether a video on this page has really played: once one has, a hidden
    // page keeps playing the next, so a hold there is a real one.
    let played = false;
    let active = true;
    let config;
    let original;
    let written;
    const read = (root, path) => {
        let value = root;
        for (const key of path) {
            if (value === null || typeof value !== 'object') {
                return undefined;
            }
            value = value[key];
        }
        return value;
    };
    const isPremium = () => {
        try {
            return read(globalThis, premiumPath) === premiumValue;
        } catch (e) {
            return false;
        }
    };
    const client = () => {
        try {
            return config?.get?.('INNERTUBE_CONTEXT')?.client;
        } catch (e) {
            return undefined;
        }
    };
    const mark = () => {
        const target = client();
        if (target === null || typeof target !== 'object' || typeof target.userAgent !== 'string') {
            return;
        }
        if (target.userAgent !== written) {
            original = target.userAgent;
        }
        if (active && isPremium()) {
            active = false;
        }
        const marker = active && index < names.length ? names[index] : '';
        written = marker === '' || !platform.test(original) ? original : original.replace(platform, `$1; ${marker}`);
        target.userAgent = written;
    };
    const adopt = (value) => {
        config = value;
        if (value !== null && typeof value === 'object' && typeof value.set === 'function') {
            const set = value.set;
            value.set = function (...args) {
                const result = Reflect.apply(set, this, args);
                mark();
                return result;
            };
        }
        mark();
    };
    let stored = globalThis.ytcfg;
    if (stored !== undefined) {
        adopt(stored);
    }
    try {
        Object.defineProperty(globalThis, 'ytcfg', {
            configurable: true,
            enumerable: true,
            get: () => stored,
            set: (value) => {
                stored = value;
                adopt(value);
            },
        });
    } catch (e) {
        return;
    }

    const edited = (player) => {
        try {
            const value = read(player.getPlayerResponse?.(), tagPath);
            return typeof value === 'string' && tag !== '' && value.includes(tag);
        } catch (e) {
            return false;
        }
    };
    const holds = (stats) => stats !== null && typeof stats === 'object'
        && held.every(([key, value]) => stats[key] === value)
        && !excused.some(([key, prefix]) => typeof stats[key] === 'string' && stats[key].startsWith(prefix));
    const refused = (player) => {
        if (blocked === '') {
            return false;
        }
        try {
            const status = player.getPlayerResponse?.()?.playabilityStatus;
            return status?.status === 'UNPLAYABLE' && JSON.stringify(status).includes(blocked);
        } catch (e) {
            return false;
        }
    };
    // An unstarted player whose video was asked to play waits on a hold too,
    // as when a dialog stopped it and playback was asked for again; one this
    // reloaded is unstarted until the new request is answered, which is no hold.
    const trying = (player, state, id) => {
        if (state === 1 || state === 3) {
            return true;
        }
        try {
            return state === -1 && pending !== id && player.querySelector?.('video')?.paused === false;
        } catch (e) {
            return false;
        }
    };
    // A playlist's video is reloaded through the playlist, which keeps the
    // player's list and place; a mix, which the player holds no list for, and
    // any player that cannot, reload the video alone.
    const reload = (player, id, at) => {
        try {
            const list = player.getPlaylistId?.();
            const index = player.getPlaylistIndex?.();
            if (typeof list === 'string' && list !== '' && player.getPlaylist?.()?.[index] === id) {
                player.loadPlaylist({ list, index, startSeconds: at });
                return;
            }
        } catch (e) {}
        player.loadVideoById(id, at);
    };
    let timer;
    let since;
    let video;
    let base;
    let heldSince;
    // The video this last reloaded, until its player starts again.
    let pending;
    // Reloads with the same marker on data no edited request returned, per
    // video: past them the hold is left alone, since no marker can change it.
    const RETRIES = 2;
    let retries = 0;
    let counted;
    let navigating;
    // The video the last navigation left, and the one the player's event last
    // ended a wait for.
    let leaving;
    let hastened;
    const stop = () => {
        clearInterval(timer);
        timer = undefined;
    };
    const sample = (signalled = false) => {
        const guarded = navigating !== undefined && Date.now() - navigating < NAVIGATE_MS;
        if (guarded && !signalled) {
            return;
        }
        if (Date.now() - since > WATCH_MS) {
            stop();
            return;
        }
        if (isPremium()) {
            active = false;
            mark();
            stop();
            return;
        }
        const player = document.getElementById('movie_player');
        if (!location.pathname.startsWith('/watch') || player === null) {
            return;
        }
        listen(player);
        let id;
        let now;
        let stats;
        try {
            id = player.getVideoData?.()?.video_id;
            now = Number(player.getCurrentTime?.() ?? 0);
            stats = player.getStatsForNerds?.();
        } catch (e) {
            return;
        }
        // The player's event names the next video's hold while a navigation
        // is still finishing; the video being left is still not reloaded.
        if (typeof id !== 'string' || id === '' || (guarded && (leaving === undefined || id === leaving))) {
            return;
        }
        if (id !== video) {
            video = id;
            base = now;
            heldSince = undefined;
        }
        if (id !== counted) {
            counted = id;
            retries = 0;
            hastened = undefined;
        }
        // A seek moves the time too; only a player no longer held has started.
        if (now - base > 0.1 && !holds(stats)) {
            try {
                played ||= player.getPlayerState?.() === 1;
            } catch (e) {}
            stop();
            return;
        }
        // Only a player trying to play, on a page in view or one that has
        // played, can be held: a hidden page that never played may wait to
        // start with the same stats, and a paused player, or an unstarted one
        // whose video was not asked to play, never was.
        if (document.visibilityState !== 'visible' && !played) {
            heldSince = undefined;
            return;
        }
        const refusing = refused(player);
        let state;
        try {
            state = player.getPlayerState?.();
        } catch (e) {
            return;
        }
        const reloading = pending === id;
        if (state === 1 || state === 3) {
            pending = undefined;
        }
        if (!refusing && !(trying(player, state, id) && holds(stats))) {
            heldSince = undefined;
            return;
        }
        heldSince ??= Date.now();
        const early = signalled && !reloading && (state === 1 || state === 3) && hastened !== id;
        if (!refusing && !early && Date.now() - heldSince < wait) {
            return;
        }
        if (early) {
            hastened = id;
        }
        heldSince = undefined;
        if (!refusing && !edited(player)) {
            if (retries >= RETRIES) {
                return;
            }
            retries += 1;
        } else if (index + 1 < names.length) {
            index += 1;
        } else if (!restarted) {
            restarted = true;
            index = 0;
        } else {
            active = false;
            mark();
            stop();
            return;
        }
        mark();
        pending = id;
        try {
            reload(player, id, Math.max(0, Math.floor(base)));
        } catch (e) {
            stop();
        }
    };
    // Each player element gets the listener once its API is ready; YouTube
    // announces a hold as soon as the player exists, so it is looked for
    // often until found.
    const listened = new WeakSet();
    const listen = (player) => {
        if (announced === '' || listened.has(player)) {
            return true;
        }
        if (typeof player.getPlayerState !== 'function' || typeof player.addEventListener !== 'function') {
            return false;
        }
        listened.add(player);
        try {
            player.addEventListener(announced, () => {
                if (timer === undefined && navigating === undefined) {
                    watch();
                }
                if (active && timer !== undefined) {
                    sample(true);
                }
            });
        } catch (e) {}
        return true;
    };
    let finding;
    let findingSince;
    const watching = () => location.pathname.startsWith('/watch');
    const find = () => {
        const player = document.getElementById('movie_player');
        const elsewhere = !watching() && navigating === undefined;
        if ((player !== null && listen(player)) || elsewhere || Date.now() - findingSince > WATCH_MS) {
            clearInterval(finding);
            finding = undefined;
        }
    };
    // Only a watch page, or a navigation that may lead to one, has a player
    // to look for.
    const look = () => {
        if (announced === '' || finding !== undefined || (!watching() && navigating === undefined)) {
            return;
        }
        findingSince = Date.now();
        finding = setInterval(find, 50);
    };
    const watch = () => {
        navigating = undefined;
        if (!active) {
            return;
        }
        since = Date.now();
        video = undefined;
        look();
        if (timer === undefined) {
            timer = setInterval(() => sample(), 250);
        }
    };
    // A navigation's old player is not reloaded while the next video loads.
    document.addEventListener('yt-navigate-start', () => {
        navigating = Date.now();
        heldSince = undefined;
        try {
            leaving = document.getElementById('movie_player')?.getVideoData?.()?.video_id;
        } catch (e) {
            leaving = undefined;
        }
        look();
    });
    document.addEventListener('yt-navigate-finish', watch);
    // Playback asked for after watching ended, as after a long dialog, or
    // after a navigation that never finished.
    document.addEventListener('play', () => {
        navigating = undefined;
        if (timer === undefined) {
            watch();
        }
    }, true);
    document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') {
            watch();
        }
    });
    look();
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', watch, { once: true });
    } else {
        watch();
    }
}
