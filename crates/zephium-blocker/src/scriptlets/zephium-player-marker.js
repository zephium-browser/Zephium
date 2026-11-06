// Zephium's own scriptlet, run only from Zephium's built-in rules: puts a marker into YouTube's client user agent, which the player-request edits of uBlock Origin's quick fixes are keyed on, and moves to the next marker when the player holds a video with no buffer before it starts. A hold counts against a marker only when the held data came from a request the quick fixes edited, which their tag in the response's `path*=text` shows; other data, inlined in the page or fetched by YouTube ahead of a click, is retried once with the same marker. A playlist's video is reloaded through the playlist when the player holds that list, so its place there is kept. The marker that last played is kept for the page's next videos; when every marker has held, the list starts over once, then the page is left unmarked. Every value YouTube may change is an argument: the markers in order, the hold's stats fields (`key=value`, `|`-separated), the stats that excuse it (`key^=prefix`), how long a hold lasts before the next marker, a Premium page's `path=value`, text that marks a playability error as a block, and the response's tag of an edited request. See docs/adblock.md.
function zephiumPlayerMarker(markers, hold, excuse, holdMs, premium, blockText, requested) {
    const names = String(markers ?? '').split(/\s+/).filter((name) => /^[a-z0-9]+$/.test(name));
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
    if (names.length === 0 || held.includes(undefined) || excused.includes(undefined) || !Number.isFinite(wait) || wait < 0 || premiumAt <= 0 || tagAt <= 0) {
        return;
    }
    const tagPath = String(requested).slice(0, tagAt).split('.');
    const tag = String(requested).slice(tagAt + 2);
    const premiumPath = String(premium).slice(0, premiumAt).split('.');
    const premiumValue = String(premium).slice(premiumAt + 1);
    const platform = /^(Mozilla\/5\.0 \([^)]*)/;
    // Watching ends a minute after a video is asked for: a hold that long is not one this can end.
    const WATCH_MS = 60000;
    let index = 0;
    let restarted = false;
    // Whether a video on this page has really played: once one has, a hidden
    // page keeps playing the next, so a hold there is a real one.
    let played = false;
    let active = true;
    let config;
    let original;
    let written;
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
    const isPremium = () => read(globalThis, premiumPath) === premiumValue;
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
    let retried;
    const stop = () => {
        clearInterval(timer);
        timer = undefined;
    };
    const sample = () => {
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
        if (typeof id !== 'string' || id === '') {
            return;
        }
        if (id !== video) {
            video = id;
            base = now;
            heldSince = undefined;
        }
        if (now - base > 0.1) {
            try {
                played ||= player.getPlayerState?.() === 1;
            } catch (e) {}
            stop();
            return;
        }
        // Only a player trying to play, on a page in view or one that has
        // played, can be held: a hidden page that never played may wait to
        // start with the same stats, and an unstarted or paused player was
        // never asked to play.
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
        if (!refusing && !((state === 1 || state === 3) && holds(stats))) {
            heldSince = undefined;
            return;
        }
        heldSince ??= Date.now();
        if (!refusing && Date.now() - heldSince < wait) {
            return;
        }
        heldSince = undefined;
        if (!refusing && !edited(player) && retried !== id) {
            retried = id;
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
        try {
            reload(player, id, Math.max(0, Math.floor(base)));
        } catch (e) {
            stop();
        }
    };
    const watch = () => {
        if (!active) {
            return;
        }
        since = Date.now();
        video = undefined;
        if (timer === undefined) {
            timer = setInterval(sample, 250);
        }
    };
    document.addEventListener('yt-navigate-finish', watch);
    document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') {
            watch();
        }
    });
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', watch, { once: true });
    } else {
        watch();
    }
}
