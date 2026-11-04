// Spike only: lets YouTube's own client-side ad run hidden, muted and fast, and presses skip when offered.
function zephiumFastAd() {
    const hide = '.ad-showing video, .ad-showing .ytp-ad-player-overlay-layout, .ad-showing .ytp-ad-module, .ad-showing .ytp-ad-player-overlay { opacity: 0 !important; }';
    const style = () => {
        if (document.documentElement !== null && !document.getElementById('zephium-fast-ad')) {
            const node = document.createElement('style');
            node.id = 'zephium-fast-ad';
            node.textContent = hide;
            document.documentElement.append(node);
        }
    };
    let saved = null;
    const tick = () => {
        style();
        const player = document.getElementById('movie_player');
        const video = player?.querySelector('video');
        if (!video) {
            return;
        }
        if (player.classList.contains('ad-showing')) {
            if (saved === null) {
                saved = { muted: video.muted, rate: video.playbackRate };
            }
            video.muted = true;
            for (const rate of [16, 8, 4, 2]) {
                try {
                    if (video.playbackRate !== rate) {
                        video.playbackRate = rate;
                    }
                    break;
                } catch (e) {
                    // A rate the engine refuses; try the next.
                }
            }
            player.querySelector('.ytp-skip-ad-button, .ytp-ad-skip-button, .ytp-ad-skip-button-modern')?.click();
        } else if (saved !== null) {
            video.muted = saved.muted;
            video.playbackRate = saved.rate;
            saved = null;
        }
    };
    new MutationObserver(tick).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['class'] });
    setInterval(tick, 100);
}
