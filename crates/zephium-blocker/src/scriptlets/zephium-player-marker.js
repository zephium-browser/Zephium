// Zephium's own scriptlet, run only from Zephium's built-in rules: puts a marker into YouTube's client user agent, which the player-request edits of uBlock Origin's quick fixes are keyed on, and moves to the next marker when the player holds a video with no buffer before it starts. The marker that last played is kept for the page's next videos; when every marker has held, the list starts over once, then the page is left unmarked. Every value YouTube may change is an argument: the markers in order, the hold's stats fields (`key=value`, `|`-separated), the stats that excuse it (`key^=prefix`), how long a hold lasts before the next marker, a Premium page's `path=value`, and text that marks a playability error as a block. See docs/adblock.md.
function zephiumPlayerMarker(markers, hold, excuse, holdMs, premium, blockText) {
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
    if (names.length === 0 || held.includes(undefined) || excused.includes(undefined) || !Number.isFinite(wait) || wait < 0 || premiumAt <= 0) {
        return;
    }
    const premiumPath = String(premium).slice(0, premiumAt).split('.');
    const premiumValue = String(premium).slice(premiumAt + 1);
    const platform = /^(Mozilla\/5\.0 \([^)]*)/;
    // Watching ends a minute after a video is asked for: a hold that long is not one this can end.
    const WATCH_MS = 60000;
    let index = 0;
    let restarted = false;
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

    const isPremium = () => {
        let value = globalThis;
        for (const key of premiumPath) {
            if (value === null || typeof value !== 'object') {
                return false;
            }
            value = value[key];
        }
        return value === premiumValue;
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
    let timer;
    let since;
    let video;
    let base;
    let heldSince;
    // The page's own video came with its player data inlined in the page, so
    // no request carried the marker: its first hold is retried with the same
    // marker before moving on.
    let inlined = new URLSearchParams(location.search).get('v');
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
            stop();
            return;
        }
        // Only a page in view whose player is trying to play can be held: a
        // hidden page's player may wait to start with the same stats, and an
        // unstarted or paused one was never asked to play.
        if (document.visibilityState !== 'visible') {
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
        if (id === inlined) {
            inlined = undefined;
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
            player.loadVideoById(id, Math.max(0, Math.floor(base)));
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
