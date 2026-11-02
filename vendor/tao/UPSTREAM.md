# Upstream

Based on the exact crates.io tao 0.35.3 package (SHA-256 `d1c93047acf68669466a34690ac58cca7010bd1b201e1ec86f1fd0a75d3dd4a9`, packaged from tauri-apps/tao commit `5a14e624c81b7a799728129417e9218be25f17d9`). The normalized Cargo.toml, `src`, README and licenses are the package contents; the upstream examples are not carried.

One local change, in `src/platform_impl/windows/event_loop.rs` (`WM_NCCALCSIZE`): a fullscreen window keeps its whole window rectangle as its client area. Upstream clamps a maximized undecorated window to the monitor work area even while it is fullscreen, which leaves a taskbar-sized strip unpainted and pillarboxes fullscreen video. This is tauri-apps/tao#1353, released as tao 0.37.2; drop this directory and its `[patch.crates-io]` entry when `vendor/wry` and `vendor/tauri-runtime-wry` move to a tao that contains it.
