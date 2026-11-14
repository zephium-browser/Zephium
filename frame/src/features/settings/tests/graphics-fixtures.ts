import type { GpuReport } from "$shared/ipc/bindings";

type Overrides = Partial<Omit<GpuReport, "detection" | "selection">> & {
  detection?: Partial<GpuReport["detection"]>;
  selection?: Partial<GpuReport["selection"]>;
};

/** An AMD laptop on GNOME Wayland, as the native report describes it. */
export function gpuReport(overrides: Overrides = {}): GpuReport {
  const { detection, selection, ...rest } = overrides;
  const report: GpuReport = {
    detection: {
      kernel: {
        status: "available",
        devices: [
          {
            driver: "amdgpu",
            driver_version: null,
            vendor_id: "0x1002",
            device_id: "0x73ff",
            card_nodes: ["card1"],
            render_nodes: ["renderD128"],
          },
        ],
        truncated: false,
      },
      gl: {
        vendor: "AMD",
        renderer: "AMD Radeon RX 6600 (radeonsi, navi23, LLVM 20.1.8, DRM 3.64, 6.16.8)",
        version: "OpenGL ES 3.2 Mesa 25.2.4",
        api: "gles",
        mesa_version: "25.2.4",
        nvidia_version: null,
        zink: false,
        software: false,
      },
      session: "wayland",
      gtk_version: "3.24.52",
      webkit_version: "2.54.2",
      monitors: [],
      monitors_truncated: false,
      compositor: "Mutter",
      video_decode: {
        va_plugin: "present_in_system_directory",
        vaapi_plugin: "not_found_in_system_directories",
        capabilities_queried: false,
      },
      probe_outcome: "supported",
      driver_class: "mesa_radeonsi",
      ...detection,
    },
    preference: "automatic",
    running_preference: "automatic",
    selection: {
      path: "default",
      source: "policy",
      reason: "no evidenced renderer exception; retain WebKit defaults",
      crash_recovery: { fallback_steps: 0, previous_incomplete_path: null },
      ...selection,
    },
    environment: [{ name: "GDK_BACKEND", value: "wayland" }],
    restart_required: false,
    startup_status: "window shown; recovery reset",
    state_warnings: [],
    text: "Zephium Linux GPU report\nSession: Wayland; compositor: Mutter; probe: Supported\n",
    ...rest,
  };
  return report;
}

export const environmentOverride = (): Overrides => ({
  selection: {
    path: "disable_dmabuf",
    source: "environment",
    reason: "explicit renderer environment overrides take precedence",
  },
  environment: [
    { name: "WEBKIT_DISABLE_DMABUF_RENDERER", value: "1" },
    { name: "GDK_BACKEND", value: "wayland" },
  ],
});

export const steppedDown = (): Overrides => ({
  selection: {
    path: "force_shm",
    source: "crash_recovery",
    reason:
      "previous automatic startup did not show a window; fallback 1/2 (reset after a successful start); policy: no evidenced renderer exception; retain WebKit defaults",
    crash_recovery: { fallback_steps: 1, previous_incomplete_path: "default" },
  },
});
