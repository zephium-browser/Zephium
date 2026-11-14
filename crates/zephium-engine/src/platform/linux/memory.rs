//! WebKitGTK construction-time memory policy.

/// Per-Web-process budget. WebKit releases caches above the conservative
/// fraction and additionally collects garbage above the strict one. No kill
/// threshold is set, so a heavy page is never terminated for its size.
pub(crate) const WEB_PROCESS: wry::WebProcessMemoryPressure = wry::WebProcessMemoryPressure {
    memory_limit_mib: 1024,
    conservative_threshold: 0.33,
    strict_threshold: 0.75,
    poll_interval: 30.0,
};

/// Network-process budget, with the same fractions and no kill threshold.
const NETWORK_PROCESS_LIMIT_MIB: u32 = 256;

/// Applies the policy on the GTK main thread before the first WebContext
/// and network process exist; WebKit ignores it for processes already running.
pub fn configure_memory_pressure() {
    let _ = wry::set_web_process_memory_pressure(WEB_PROCESS);
    let mut network = webkit2gtk::MemoryPressureSettings::new();
    network.set_memory_limit(NETWORK_PROCESS_LIMIT_MIB);
    network.set_conservative_threshold(WEB_PROCESS.conservative_threshold);
    network.set_strict_threshold(WEB_PROCESS.strict_threshold);
    network.set_poll_interval(WEB_PROCESS.poll_interval);
    webkit2gtk::WebsiteDataManager::set_memory_pressure_settings(&mut network);
}

// Ordered fractions, cache release above 300 MiB, and no busy polling.
const _: () = assert!(
    0.0 < WEB_PROCESS.conservative_threshold
        && WEB_PROCESS.conservative_threshold < WEB_PROCESS.strict_threshold
        && WEB_PROCESS.strict_threshold < 1.0
        && WEB_PROCESS.memory_limit_mib as f64 * WEB_PROCESS.conservative_threshold >= 300.0
        && NETWORK_PROCESS_LIMIT_MIB >= 128
        && WEB_PROCESS.poll_interval >= 10.0
);
