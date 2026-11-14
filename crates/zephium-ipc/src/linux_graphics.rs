use serde::{Deserialize, Serialize};
use specta::Type;
use zephium_core::linux_graphics::{DriverClass, SessionType};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct Detection {
    pub kernel: KernelGraphics,
    pub gl: Option<GlInfo>,
    pub session: SessionType,
    pub gtk_version: String,
    pub webkit_version: String,
    pub monitors: Vec<MonitorInfo>,
    pub monitors_truncated: bool,
    pub compositor: Option<String>,
    pub video_decode: VideoDecodeInfo,
    pub probe_outcome: ProbeOutcome,
    pub driver_class: DriverClass,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct ProbeData {
    pub outcome: ProbeOutcome,
    pub session: SessionType,
    pub gl: Option<GlInfo>,
    pub monitors: Vec<MonitorInfo>,
    pub monitors_truncated: bool,
    pub compositor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Supported,
    Unavailable,
    Inconclusive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct KernelGraphics {
    pub status: KernelStatus,
    pub devices: Vec<KernelDevice>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum KernelStatus {
    Available,
    NoGpu,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct KernelDevice {
    pub driver: Option<String>,
    pub driver_version: Option<String>,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    pub card_nodes: Vec<String>,
    pub render_nodes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct GlInfo {
    pub vendor: Option<String>,
    pub renderer: Option<String>,
    pub version: Option<String>,
    pub api: GlApi,
    pub mesa_version: Option<String>,
    pub nvidia_version: Option<String>,
    pub zink: bool,
    pub software: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GlApi {
    Desktop,
    Gles,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct MonitorInfo {
    pub index: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub integer_scale: i32,
    pub fractional_scale: Option<f64>,
    pub refresh_rate_millihertz: i32,
    pub primary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct VideoDecodeInfo {
    pub va_plugin: PluginPresence,
    pub vaapi_plugin: PluginPresence,
    pub capabilities_queried: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PluginPresence {
    PresentInSystemDirectory,
    NotFoundInSystemDirectories,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct GraphicsEnvironmentOverride {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct GpuReport {
    pub detection: Detection,
    pub preference: zephium_core::linux_graphics::RendererPreference,
    pub running_preference: zephium_core::linux_graphics::RendererPreference,
    pub selection: zephium_core::linux_graphics::RendererSelection,
    pub environment: Vec<GraphicsEnvironmentOverride>,
    pub restart_required: bool,
    /// The person asked the next automatic start to try a held-back faster path again.
    pub retry_requested: bool,
    pub startup_status: String,
    pub state_warnings: Vec<String>,
    pub text: String,
}
