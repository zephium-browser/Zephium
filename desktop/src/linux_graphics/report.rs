use std::fmt::Write;
use zephium_ipc::linux_graphics::{GpuReport, PluginPresence};

pub(super) fn render(report: &GpuReport) -> String {
    let detection = &report.detection;
    let mut text = String::from("Zephium Linux GPU report\n");
    let _ = writeln!(
        text,
        "Session: {:?}; compositor: {}; probe: {:?}",
        detection.session,
        detection.compositor.as_deref().unwrap_or("unknown"),
        detection.probe_outcome
    );
    let _ = writeln!(
        text,
        "GTK: {}; WebKitGTK: {}; driver class: {:?}",
        detection.gtk_version, detection.webkit_version, detection.driver_class
    );
    let _ = writeln!(
        text,
        "Kernel GPU status: {:?}; truncated: {}",
        detection.kernel.status, detection.kernel.truncated
    );
    for (index, device) in detection.kernel.devices.iter().enumerate() {
        let _ = writeln!(
            text,
            "GPU {index}: {} {}; vendor/device: {}/{}; cards: {}; render nodes: {}",
            device.driver.as_deref().unwrap_or("unknown"),
            device
                .driver_version
                .as_deref()
                .unwrap_or("version unknown"),
            device.vendor_id.as_deref().unwrap_or("unknown"),
            device.device_id.as_deref().unwrap_or("unknown"),
            device.card_nodes.join(","),
            device.render_nodes.join(",")
        );
    }
    if let Some(gl) = &detection.gl {
        let _ = writeln!(
            text,
            "GL: {:?}; vendor: {}; renderer: {}; version: {}",
            gl.api,
            gl.vendor.as_deref().unwrap_or("unknown"),
            gl.renderer.as_deref().unwrap_or("unknown"),
            gl.version.as_deref().unwrap_or("unknown")
        );
        let _ = writeln!(
            text,
            "Mesa: {}; NVIDIA: {}; software: {}; Zink: {}",
            gl.mesa_version.as_deref().unwrap_or("unknown"),
            gl.nvidia_version.as_deref().unwrap_or("unknown"),
            gl.software,
            gl.zink
        );
    } else {
        text.push_str("GL: unknown\n");
    }
    let _ = writeln!(
        text,
        "Preference: {:?}; running: {:?}; restart required: {}",
        report.preference, report.running_preference, report.restart_required
    );
    let _ = writeln!(
        text,
        "Selected: {:?} ({:?}); {}",
        report.selection.path, report.selection.source, report.selection.reason
    );
    let recovery = &report.selection.crash_recovery;
    let _ = writeln!(
        text,
        "Startup: {}; recovery used for this launch: {}; previous incomplete: {:?}",
        report.startup_status, recovery.fallback_steps, recovery.previous_incomplete_path
    );
    let _ = writeln!(
        text,
        "Held after repeated failures: {:?}; retry requested: {}",
        recovery.held_path, report.retry_requested
    );
    if report.environment.is_empty() {
        text.push_str("Environment overrides: none\n");
    }
    for entry in &report.environment {
        let _ = writeln!(text, "Environment: {}={}", entry.name, entry.value);
    }
    let _ = writeln!(
        text,
        "GStreamer: va={}; vaapi={}; decode capabilities: not queried",
        presence(detection.video_decode.va_plugin),
        presence(detection.video_decode.vaapi_plugin)
    );
    if detection.monitors.is_empty() {
        text.push_str("Monitors: unknown\n");
    }
    for monitor in &detection.monitors {
        let _ = writeln!(
            text,
            "Monitor {}: {}x{} at {},{}; scale {}; fractional {}; refresh {} mHz; primary {}",
            monitor.index,
            monitor.width,
            monitor.height,
            monitor.x,
            monitor.y,
            monitor.integer_scale,
            monitor
                .fractional_scale
                .map(|scale| scale.to_string())
                .unwrap_or_else(|| "unknown (GTK3 integer scale only)".into()),
            monitor.refresh_rate_millihertz,
            monitor.primary
        );
    }
    if detection.monitors_truncated {
        text.push_str("Monitor list truncated\n");
    }
    for warning in &report.state_warnings {
        let _ = writeln!(text, "State warning: {warning}");
    }
    text
}

fn presence(value: PluginPresence) -> &'static str {
    match value {
        PluginPresence::PresentInSystemDirectory => "system plugin file present",
        PluginPresence::NotFoundInSystemDirectories => "not found in system plugin directories",
        PluginPresence::Unknown => "unknown",
    }
}
