import * as m from "$shared/i18n/messages";
import type {
  DriverClass,
  GpuReport,
  RendererPath,
  RendererPreference,
  SelectionSource,
  SessionType,
} from "$shared/ipc/bindings";

export const modes = [
  {
    value: "automatic",
    label: m.graphics_mode_automatic,
    description: m.graphics_mode_automatic_desc,
  },
  { value: "gpu", label: m.graphics_mode_gpu, description: m.graphics_mode_gpu_desc },
  {
    value: "compatibility",
    label: m.graphics_mode_compatibility,
    description: m.graphics_mode_compatibility_desc,
  },
  { value: "safe", label: m.graphics_mode_safe, description: m.graphics_mode_safe_desc },
] as const satisfies readonly {
  value: RendererPreference;
  label: () => string;
  description: () => string;
}[];

/** The fixed mode that runs each path, so a path is named as the person chose it. */
const PATH_MODE: Record<RendererPath, Exclude<RendererPreference, "automatic">> = {
  default: "gpu",
  force_shm: "compatibility",
  disable_dmabuf: "safe",
};
const MODE_PATH: Partial<Record<RendererPreference, RendererPath>> = {
  gpu: "default",
  compatibility: "force_shm",
  safe: "disable_dmabuf",
};

export const modeName = (mode: RendererPreference) =>
  modes.find((entry) => entry.value === mode)!.label();
const pathMode = (path: RendererPath) => PATH_MODE[path];

/** Only these outrank the saved choice; the report lists the others as context. */
const RENDERER_VARIABLES = new Set([
  "WEBKIT_DISABLE_DMABUF_RENDERER",
  "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
]);

export type GraphicsStatus =
  | { kind: "environment"; title: string; detail: string; variables: string[] }
  | { kind: "restart"; title: string; detail: string }
  | { kind: "recovered"; title: string; detail: string; keep: RendererPreference }
  | { kind: "current"; title: string; detail: string };

/** What runs now and why, in the order that explains the most. */
export function graphicsStatus(report: GpuReport): GraphicsStatus {
  const { selection, preference } = report;
  const running = modeName(pathMode(selection.path));
  const title = m.graphics_running({ mode: running });
  if (selection.source === "environment")
    return {
      kind: "environment",
      title: m.graphics_environment({ mode: running }),
      detail: m.graphics_environment_detail(),
      variables: report.environment
        .filter((entry) => RENDERER_VARIABLES.has(entry.name))
        .map((entry) => `${entry.name}=${entry.value}`),
    };
  // A saved mode whose path already runs needs no restart the person would notice.
  const unchanged = MODE_PATH[preference] === selection.path;
  if (report.restart_required && !unchanged)
    return {
      kind: "restart",
      title: m.graphics_restart({ mode: modeName(preference) }),
      detail: m.graphics_restart_detail({ mode: running }),
    };
  if (report.restart_required) return { kind: "current", title, detail: m.graphics_running_same() };
  if (selection.source === "crash_recovery")
    return {
      kind: "recovered",
      title: m.graphics_recovered({ mode: running }),
      detail: m.graphics_recovered_detail(),
      keep: pathMode(selection.path),
    };
  if (selection.source === "preference")
    return { kind: "current", title, detail: m.graphics_running_chosen() };
  if (report.detection.gl?.software || report.detection.driver_class === "software")
    return { kind: "current", title, detail: m.graphics_running_software() };
  return {
    kind: "current",
    title,
    detail:
      selection.path === "default"
        ? m.graphics_running_policy()
        : m.graphics_running_policy_adjusted(),
  };
}

const DRIVERS: Record<DriverClass, () => string> = {
  mesa_radeonsi: m.graphics_driver_radeonsi,
  mesa_iris: m.graphics_driver_iris,
  mesa_crocus: m.graphics_driver_crocus,
  mesa_i915: m.graphics_driver_i915,
  nvidia_proprietary: m.graphics_driver_nvidia,
  nouveau: m.graphics_driver_nouveau,
  nvk: m.graphics_driver_nvk,
  virtio_virgl: m.graphics_driver_virtio,
  software: m.graphics_driver_software,
  asahi: m.graphics_driver_asahi,
  other_mesa: m.graphics_driver_mesa,
  unknown: m.graphics_unknown,
};

const SESSIONS: Record<SessionType, () => string> = {
  wayland: () => "Wayland",
  x11: () => "X11",
  unknown: m.graphics_unknown,
};

const SOURCES: Record<SelectionSource, (inputs: { mode: string }) => string> = {
  policy: m.graphics_selected_policy,
  preference: m.graphics_selected_preference,
  environment: m.graphics_selected_environment,
  crash_recovery: m.graphics_selected_recovery,
};

export type DetailRow = {
  id: string;
  label: string;
  value: string;
  /** Diagnostic text from the native report, which is not translated. */
  note?: string;
};

/** The few facts someone diagnosing a GPU problem reads first. The rest is in the copied report. */
export function graphicsDetails(report: GpuReport): DetailRow[] {
  const { detection, selection } = report;
  const gl = detection.gl;
  const device = detection.kernel.devices[0];
  const unknown = m.graphics_unknown();
  const driverVersion = gl?.nvidia_version
    ? `NVIDIA ${gl.nvidia_version}`
    : gl?.mesa_version
      ? `Mesa ${gl.mesa_version}`
      : (device?.driver_version ?? unknown);
  const session = SESSIONS[detection.session]();
  return [
    { id: "driver", label: m.graphics_driver(), value: DRIVERS[detection.driver_class]() },
    { id: "gpu", label: m.graphics_gpu(), value: gl?.renderer ?? device?.driver ?? unknown },
    { id: "opengl", label: m.graphics_opengl(), value: gl?.version ?? unknown },
    { id: "driver-version", label: m.graphics_driver_version(), value: driverVersion },
    {
      id: "session",
      label: m.graphics_session(),
      value: detection.compositor ? `${session} · ${detection.compositor}` : session,
    },
    {
      id: "engine",
      label: m.graphics_engine(),
      value: `WebKitGTK ${detection.webkit_version} · GTK ${detection.gtk_version}`,
    },
    {
      id: "selected",
      label: m.graphics_selected(),
      value: SOURCES[selection.source]({ mode: modeName(pathMode(selection.path)) }),
      note: selection.reason,
    },
  ];
}

export type Warning = { text: string; native: boolean };

export function graphicsWarnings(report: GpuReport): Warning[] {
  const { detection } = report;
  const warnings: Warning[] = [];
  if (detection.kernel.status === "no_gpu")
    warnings.push({ text: m.graphics_warning_no_gpu(), native: false });
  if (detection.gl?.software || detection.driver_class === "software")
    warnings.push({ text: m.graphics_warning_software(), native: false });
  if (detection.probe_outcome !== "supported")
    warnings.push({ text: m.graphics_warning_probe(), native: false });
  for (const text of report.state_warnings) warnings.push({ text, native: true });
  return warnings;
}
