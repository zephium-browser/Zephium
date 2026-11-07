# Upstream

Based on the exact crates.io tao 0.35.3 package (SHA-256 `d1c93047acf68669466a34690ac58cca7010bd1b201e1ec86f1fd0a75d3dd4a9`, packaged from tauri-apps/tao commit `5a14e624c81b7a799728129417e9218be25f17d9`). The normalized Cargo.toml, `src`, README and licenses are the package contents; the upstream examples are not carried.

Two local changes, both in `src/platform_impl/windows/event_loop.rs`.

`WM_NCCALCSIZE`: a fullscreen window keeps its whole window rectangle as its client area. Upstream clamps a maximized undecorated window to the monitor work area even while it is fullscreen, which leaves a taskbar-sized strip unpainted and pillarboxes fullscreen video. This is tauri-apps/tao#1353, released as tao 0.37.2; that part can go when `vendor/wry` and `vendor/tauri-runtime-wry` move to a tao that contains it.

Mouse capture: a window gives up the capture it took for a button press once no button holds it (`WM_MOUSEMOVE` without a button, `WM_KILLFOCUS` with none down), and whenever Windows cancels modes (`WM_CANCELMODE`, `WM_ACTIVATEAPP` losing activation). Upstream releases only when its own button-up arrives; a press whose release lands on another window left the browser window holding the capture, and Windows sends no `WM_SETCURSOR` to anyone while it is held, so the pointer kept whatever shape it last had. Not yet upstream, so this directory stays until tao carries an equivalent.
