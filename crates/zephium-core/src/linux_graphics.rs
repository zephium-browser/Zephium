//! Pure Linux renderer selection; native discovery and early persistence live in desktop.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum DriverClass {
    MesaRadeonsi,
    MesaIris,
    MesaCrocus,
    MesaI915,
    NvidiaProprietary,
    Nouveau,
    Nvk,
    VirtioVirgl,
    Software,
    Asahi,
    OtherMesa,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    Wayland,
    X11,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum RendererPath {
    #[default]
    Default,
    ForceShm,
    DisableDmabuf,
}

impl RendererPath {
    pub const fn step_down(self) -> Self {
        match self {
            Self::Default => Self::ForceShm,
            Self::ForceShm | Self::DisableDmabuf => Self::DisableDmabuf,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum RendererPreference {
    #[default]
    Automatic,
    Gpu,
    Compatibility,
    Safe,
}

impl RendererPreference {
    pub const fn path(self) -> Option<RendererPath> {
        match self {
            Self::Automatic => None,
            Self::Gpu => Some(RendererPath::Default),
            Self::Compatibility => Some(RendererPath::ForceShm),
            Self::Safe => Some(RendererPath::DisableDmabuf),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GraphicsVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GraphicsVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value.len() > 32 {
            return None;
        }
        let mut parts = value.split('.');
        let mut component = || {
            let raw = parts.next()?;
            if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            raw.parse::<u32>().ok()
        };
        let major = component()?;
        let minor = component()?;
        let patch = component()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self::new(major, minor, patch))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DriverEvidence<'a> {
    pub vendor: &'a str,
    pub renderer: &'a str,
    pub version: &'a str,
    pub kernel_drivers: &'a [&'a str],
}

pub fn is_software_renderer(renderer: &str) -> bool {
    let renderer = bounded_lowercase(renderer);
    [
        "llvmpipe",
        "softpipe",
        "swrast",
        "software rasterizer",
        "lavapipe",
        "swiftshader",
    ]
    .iter()
    .any(|marker| renderer.contains(marker))
}

pub fn classify_driver(evidence: DriverEvidence<'_>) -> DriverClass {
    let vendor = bounded_lowercase(evidence.vendor);
    let renderer = bounded_lowercase(evidence.renderer);
    let version = bounded_lowercase(evidence.version);
    if is_software_renderer(&renderer) {
        return DriverClass::Software;
    }
    // The active GL renderer takes priority over other GPUs installed in a hybrid system.
    if ["virgl", "virtio", "venus"]
        .iter()
        .any(|name| renderer.contains(name))
    {
        return DriverClass::VirtioVirgl;
    }
    if renderer.contains("zink")
        && kernel_driver_class(evidence.kernel_drivers) == DriverClass::VirtioVirgl
    {
        return DriverClass::VirtioVirgl;
    }
    if renderer
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == "nvk")
    {
        return DriverClass::Nvk;
    }
    if renderer.contains("nouveau") || vendor.contains("nouveau") {
        return DriverClass::Nouveau;
    }
    if (vendor.contains("nvidia") || renderer.contains("nvidia") || version.contains("nvidia"))
        && !version.contains("mesa")
    {
        return DriverClass::NvidiaProprietary;
    }
    if renderer.contains("radeonsi") {
        return DriverClass::MesaRadeonsi;
    }
    if renderer.contains("crocus") {
        return DriverClass::MesaCrocus;
    }
    if renderer == "iris"
        || renderer.starts_with("iris ")
        || renderer.contains("(iris")
        || renderer.contains(", iris")
        || (renderer.contains("intel") && renderer.contains(" xe "))
    {
        return DriverClass::MesaIris;
    }
    if renderer.contains("i915") || renderer.contains("i965") {
        return DriverClass::MesaI915;
    }
    if renderer.contains("zink") {
        return DriverClass::OtherMesa;
    }
    if renderer.contains("asahi") || renderer.contains("agx") || vendor.contains("asahi") {
        return DriverClass::Asahi;
    }
    if version.contains("mesa") || vendor.contains("mesa") {
        if renderer.contains("intel") {
            // Without the userspace driver name the kernel's i915 family is all we know.
            return DriverClass::MesaI915;
        }
        if matches!(
            kernel_driver_class(evidence.kernel_drivers),
            DriverClass::Nouveau | DriverClass::VirtioVirgl | DriverClass::Asahi
        ) {
            return kernel_driver_class(evidence.kernel_drivers);
        }
        return DriverClass::OtherMesa;
    }
    if !renderer.is_empty() || !vendor.is_empty() || !version.is_empty() {
        return DriverClass::Unknown;
    }
    kernel_driver_class(evidence.kernel_drivers)
}

fn kernel_driver_class(kernel_drivers: &[&str]) -> DriverClass {
    if kernel_drivers.len() > 32 {
        return DriverClass::Unknown;
    }
    let mut drivers =
        kernel_drivers
            .iter()
            .take(32)
            .map(|driver| match bounded_lowercase(driver).as_str() {
                "virtio_gpu" => DriverClass::VirtioVirgl,
                "nvidia" => DriverClass::NvidiaProprietary,
                "nouveau" => DriverClass::Nouveau,
                "i915" | "xe" => DriverClass::MesaI915,
                "asahi" => DriverClass::Asahi,
                _ => DriverClass::Unknown,
            });
    let Some(first) = drivers.next() else {
        return DriverClass::Unknown;
    };
    if drivers.all(|driver| driver == first) {
        first
    } else {
        DriverClass::Unknown
    }
}

fn bounded_lowercase(value: &str) -> String {
    value
        .chars()
        .take(4096)
        .collect::<String>()
        .to_ascii_lowercase()
}

#[derive(Clone, Copy, Debug)]
pub enum VersionPredicate {
    Any,
    Exact(GraphicsVersion),
    Range {
        minimum: GraphicsVersion,
        maximum_exclusive: GraphicsVersion,
    },
}

impl VersionPredicate {
    pub fn matches(self, version: Option<GraphicsVersion>) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(required) => version == Some(required),
            Self::Range {
                minimum,
                maximum_exclusive,
            } => version.is_some_and(|version| version >= minimum && version < maximum_exclusive),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RendererPolicyRule {
    pub driver_class: DriverClass,
    pub session: Option<SessionType>,
    pub webkit_version: VersionPredicate,
    pub mesa_version: VersionPredicate,
    pub nvidia_version: VersionPredicate,
    pub path: RendererPath,
    pub reason: &'static str,
}

const VIRTIO_REASON: &str = "virtio/virgl: DMA-BUF icon flicker observed with WebKitGTK 2.54.2; shared-memory acceptance pending";

pub const RENDERER_POLICY: &[RendererPolicyRule] = &[
    RendererPolicyRule {
        driver_class: DriverClass::VirtioVirgl,
        session: Some(SessionType::Wayland),
        webkit_version: VersionPredicate::Exact(GraphicsVersion::new(2, 54, 2)),
        mesa_version: VersionPredicate::Any,
        nvidia_version: VersionPredicate::Any,
        path: RendererPath::DisableDmabuf,
        reason: VIRTIO_REASON,
    },
    RendererPolicyRule {
        driver_class: DriverClass::VirtioVirgl,
        session: Some(SessionType::X11),
        webkit_version: VersionPredicate::Exact(GraphicsVersion::new(2, 54, 2)),
        mesa_version: VersionPredicate::Any,
        nvidia_version: VersionPredicate::Any,
        path: RendererPath::DisableDmabuf,
        reason: VIRTIO_REASON,
    },
    RendererPolicyRule {
        driver_class: DriverClass::Software,
        session: None,
        webkit_version: VersionPredicate::Any,
        mesa_version: VersionPredicate::Any,
        nvidia_version: VersionPredicate::Any,
        path: RendererPath::Default,
        reason: "software rasterizer: retain WebKit's GL/device capability checks",
    },
];

#[derive(Clone, Copy, Debug)]
pub struct PolicyContext {
    pub driver_class: DriverClass,
    pub session: SessionType,
    pub webkit_version: Option<GraphicsVersion>,
    pub mesa_version: Option<GraphicsVersion>,
    pub nvidia_version: Option<GraphicsVersion>,
}

pub fn renderer_policy(context: PolicyContext) -> (RendererPath, &'static str) {
    for rule in RENDERER_POLICY {
        if rule.driver_class == context.driver_class
            && rule
                .session
                .is_none_or(|session| session == context.session)
            && rule.webkit_version.matches(context.webkit_version)
            && rule.mesa_version.matches(context.mesa_version)
            && rule.nvidia_version.matches(context.nvidia_version)
        {
            return (rule.path, rule.reason);
        }
    }
    (
        RendererPath::Default,
        "no evidenced renderer exception; retain WebKit defaults",
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RendererOverrides<'a> {
    pub disable_dmabuf: Option<&'a str>,
    pub force_shm: Option<&'a str>,
}

impl RendererOverrides<'_> {
    pub fn is_present(self) -> bool {
        self.disable_dmabuf.is_some() || self.force_shm.is_some()
    }

    pub fn path(self) -> RendererPath {
        if self.disable_dmabuf.is_some_and(|value| value != "0") {
            RendererPath::DisableDmabuf
        } else if self.force_shm.is_some_and(|value| value != "0") {
            RendererPath::ForceShm
        } else {
            RendererPath::Default
        }
    }
}

pub const MAX_CRASH_FALLBACK_STEPS: u8 = 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct CrashRecovery {
    pub fallback_steps: u8,
    pub previous_incomplete_path: Option<RendererPath>,
    /// The path Automatic keeps because a faster one failed to start twice with this
    /// graphics fingerprint, until the fingerprint changes or the person retries.
    pub held_path: Option<RendererPath>,
}

impl CrashRecovery {
    pub fn record_incomplete(self, path: RendererPath) -> Self {
        Self {
            fallback_steps: self
                .fallback_steps
                .saturating_add(1)
                .min(MAX_CRASH_FALLBACK_STEPS),
            previous_incomplete_path: Some(path),
            held_path: self.held_path,
        }
    }

    pub fn reset(self) -> Self {
        Self::default()
    }

    pub fn bounded(self) -> Self {
        Self {
            fallback_steps: self.fallback_steps.min(MAX_CRASH_FALLBACK_STEPS),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    Environment,
    Preference,
    Policy,
    CrashRecovery,
    RepeatedFailure,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct RendererSelection {
    pub path: RendererPath,
    pub source: SelectionSource,
    pub reason: String,
    pub crash_recovery: CrashRecovery,
}

pub fn select_renderer(
    context: PolicyContext,
    preference: RendererPreference,
    overrides: RendererOverrides<'_>,
    recovery: CrashRecovery,
) -> RendererSelection {
    let recovery = recovery.bounded();
    if overrides.is_present() {
        return RendererSelection {
            path: overrides.path(),
            source: SelectionSource::Environment,
            reason: "explicit renderer environment overrides take precedence".into(),
            crash_recovery: recovery,
        };
    }
    if let Some(path) = preference.path() {
        return RendererSelection {
            path,
            source: SelectionSource::Preference,
            reason: "renderer preference applies until changed and restarted".into(),
            crash_recovery: recovery,
        };
    }
    let (policy_path, policy_reason) = renderer_policy(context);
    let mut path = policy_path;
    for _ in 0..recovery.fallback_steps {
        path = path.step_down();
    }
    if let Some(previous) = recovery.previous_incomplete_path {
        path = path.max(previous.step_down());
    }
    if let Some(held) = recovery
        .held_path
        .filter(|held| *held >= path && *held > policy_path)
    {
        return RendererSelection {
            path: held,
            source: SelectionSource::RepeatedFailure,
            reason: format!("automatic startup failed twice with this graphics fingerprint; staying on {held:?} until the driver, WebKitGTK, session or GPU changes, or a retry is requested; policy: {policy_reason}"),
            crash_recovery: recovery,
        };
    }
    if recovery.fallback_steps > 0 || recovery.previous_incomplete_path.is_some() {
        RendererSelection {
            path,
            source: SelectionSource::CrashRecovery,
            reason: format!("previous automatic startup did not show a window; fallback {}/{} (reset after a successful start); policy: {}", recovery.fallback_steps, MAX_CRASH_FALLBACK_STEPS, policy_reason),
            crash_recovery: recovery,
        }
    } else {
        RendererSelection {
            path,
            source: SelectionSource::Policy,
            reason: policy_reason.into(),
            crash_recovery: recovery,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(driver_class: DriverClass) -> PolicyContext {
        PolicyContext {
            driver_class,
            session: SessionType::Wayland,
            webkit_version: Some(GraphicsVersion::new(2, 54, 2)),
            mesa_version: Some(GraphicsVersion::new(25, 3, 0)),
            nvidia_version: None,
        }
    }

    #[test]
    fn realistic_gl_strings_identify_driver_families() {
        let fixtures = [
            (
                "AMD",
                "AMD Radeon RX 6600 (radeonsi, navi23, LLVM 20.1.8, DRM 3.64, 6.16.8)",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::MesaRadeonsi,
            ),
            (
                "Intel",
                "Mesa Intel(R) Iris(R) Xe Graphics (TGL GT2)",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::MesaIris,
            ),
            (
                "Intel Open Source Technology Center",
                "Mesa DRI Intel(R) HD Graphics 4000 (IVB GT2, crocus)",
                "4.2 (Core Profile) Mesa 25.2.4",
                DriverClass::MesaCrocus,
            ),
            (
                "Intel Open Source Technology Center",
                "Mesa DRI Intel(R) 945GM (i915)",
                "2.1 Mesa 25.2.4",
                DriverClass::MesaI915,
            ),
            (
                "NVIDIA Corporation",
                "NVIDIA GeForce RTX 4070/PCIe/SSE2",
                "4.6.0 NVIDIA 580.95.05",
                DriverClass::NvidiaProprietary,
            ),
            (
                "nouveau",
                "NV117",
                "4.3 (Core Profile) Mesa 25.2.4",
                DriverClass::Nouveau,
            ),
            (
                "Mesa",
                "zink Vulkan 1.4(NVIDIA GeForce RTX 3060 (NVK GA106) (Mesa 25.2.4))",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::Nvk,
            ),
            (
                "Mesa",
                "virgl (Apple M1 (ANGLE Metal Renderer: Apple M1, Unspecified Version))",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::VirtioVirgl,
            ),
            (
                "Mesa",
                "zink Vulkan 1.3(virtio-gpu Venus (Mesa 25.2.4))",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::VirtioVirgl,
            ),
            (
                "Mesa",
                "llvmpipe (LLVM 20.1.8, 128 bits)",
                "4.5 (Core Profile) Mesa 25.2.4",
                DriverClass::Software,
            ),
            (
                "Mesa",
                "softpipe",
                "3.3 (Core Profile) Mesa 25.2.4",
                DriverClass::Software,
            ),
            ("Mesa", "swrast", "2.1 Mesa 25.2.4", DriverClass::Software),
            (
                "Mesa",
                "zink Vulkan 1.3(llvmpipe (LLVM 20.1.8, 128 bits))",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::Software,
            ),
            (
                "Mesa",
                "zink Vulkan 1.4(AMD Radeon RX 6600 (RADV NAVI23))",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::OtherMesa,
            ),
            (
                "Mesa",
                "Apple M2 (G14G B0) (Asahi)",
                "4.6 (Core Profile) Mesa 25.2.4",
                DriverClass::Asahi,
            ),
            (
                "Mesa",
                "V3D 7.1.10.2",
                "3.1 Mesa 25.2.4",
                DriverClass::OtherMesa,
            ),
            (
                "Unidentified vendor",
                "New renderer",
                "1.0",
                DriverClass::Unknown,
            ),
        ];
        for (vendor, renderer, version, expected) in fixtures {
            assert_eq!(
                classify_driver(DriverEvidence {
                    vendor,
                    renderer,
                    version,
                    kernel_drivers: &[]
                }),
                expected,
                "renderer {renderer}"
            );
        }
    }

    #[test]
    fn active_renderer_wins_over_other_installed_gpus() {
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: "Intel",
                renderer: "Mesa Intel(R) Iris(R) Xe Graphics (TGL GT2)",
                version: "4.6 (Core Profile) Mesa 25.2.4",
                kernel_drivers: &["nvidia", "i915"],
            }),
            DriverClass::MesaIris
        );
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: "NVIDIA Corporation",
                renderer: "NVIDIA GeForce RTX 4070/PCIe/SSE2",
                version: "4.6.0 NVIDIA 580.95.05",
                kernel_drivers: &["nvidia", "i915"],
            }),
            DriverClass::NvidiaProprietary
        );
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: "Mesa",
                renderer: "llvmpipe (LLVM 20.1.8, 128 bits)",
                version: "4.5 (Core Profile) Mesa 25.2.4",
                kernel_drivers: &["nvidia", "i915"],
            }),
            DriverClass::Software
        );
    }

    #[test]
    fn kernel_fallback_keeps_missing_and_ambiguous_hardware_unknown() {
        let classify = |kernel_drivers| {
            classify_driver(DriverEvidence {
                vendor: "",
                renderer: "",
                version: "",
                kernel_drivers,
            })
        };
        assert_eq!(classify(&[]), DriverClass::Unknown);
        assert_eq!(classify(&["virtio_gpu"]), DriverClass::VirtioVirgl);
        assert_eq!(classify(&["i915", "i915"]), DriverClass::MesaI915);
        assert_eq!(classify(&["nvidia"]), DriverClass::NvidiaProprietary);
        assert_eq!(classify(&["nouveau"]), DriverClass::Nouveau);
        assert_eq!(classify(&["asahi"]), DriverClass::Asahi);
        assert_eq!(classify(&["amdgpu"]), DriverClass::Unknown);
        assert_eq!(classify(&["i915", "nvidia"]), DriverClass::Unknown);
        assert_eq!(classify(&["virtio_gpu", "amdgpu"]), DriverClass::Unknown);
        assert_eq!(classify(&["nvidia"; 33]), DriverClass::Unknown);
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: "Mesa",
                renderer: "NV117",
                version: "4.3 Mesa 25.2.4",
                kernel_drivers: &["nouveau"]
            }),
            DriverClass::Nouveau
        );
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: "Mesa",
                renderer: "Apple M1",
                version: "4.6 Mesa 25.2.4",
                kernel_drivers: &["virtio_gpu"]
            }),
            DriverClass::VirtioVirgl
        );
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: "Mesa",
                renderer: "zink Vulkan 1.3(Apple M4 (AGX, MoltenVK 1.3))",
                version: "4.6 Mesa 25.2.4",
                kernel_drivers: &["virtio_gpu"]
            }),
            DriverClass::VirtioVirgl
        );
    }

    #[test]
    fn virtualized_zink_prioritizes_software_then_unique_guest_driver() {
        let fixtures: &[(&str, &[&str], DriverClass)] = &[
            (
                "zink Vulkan 1.4(NVIDIA GeForce RTX 3060 (NVK GA106))",
                &["virtio_gpu"],
                DriverClass::VirtioVirgl,
            ),
            (
                "zink Vulkan 1.3(Apple M4 (AGX, MoltenVK 1.3))",
                &["virtio_gpu"],
                DriverClass::VirtioVirgl,
            ),
            (
                "zink Vulkan 1.3(llvmpipe (LLVM 20.1.8, 128 bits))",
                &["virtio_gpu"],
                DriverClass::Software,
            ),
            (
                "zink Vulkan 1.4(NVIDIA GeForce RTX 3060 (NVK GA106))",
                &["virtio_gpu", "nouveau"],
                DriverClass::Nvk,
            ),
            (
                "zink Vulkan 1.3(Apple M4 (MoltenVK 1.3))",
                &[],
                DriverClass::OtherMesa,
            ),
        ];
        for &(renderer, kernel_drivers, expected) in fixtures {
            assert_eq!(
                classify_driver(DriverEvidence {
                    vendor: "Mesa",
                    renderer,
                    version: "4.6 Mesa 25.2.4",
                    kernel_drivers,
                }),
                expected,
                "renderer {renderer}, kernel {kernel_drivers:?}"
            );
        }
    }

    #[test]
    fn marketing_names_do_not_assert_a_userspace_driver() {
        for (renderer, expected) in [
            (
                "Mesa Intel(R) Iris(R) Pro 5200 (HSW GT3)",
                DriverClass::MesaI915,
            ),
            (
                "Mesa Intel(R) HD Graphics 4000 (IVB GT2)",
                DriverClass::MesaI915,
            ),
            ("ATI Radeon X1950 (R580)", DriverClass::OtherMesa),
            ("AMD Radeon HD 6450 (CAICOS)", DriverClass::OtherMesa),
        ] {
            assert_eq!(
                classify_driver(DriverEvidence {
                    vendor: "Mesa",
                    renderer,
                    version: "4.2 Mesa 25.2.4",
                    kernel_drivers: &[],
                }),
                expected
            );
        }
    }

    #[test]
    fn classification_tolerates_malformed_and_bounded_strings() {
        for renderer in ["", "\0\n\r", "🚀 unknown GPU", "NVK", "SWRAST"] {
            let result = classify_driver(DriverEvidence {
                vendor: "",
                renderer,
                version: "",
                kernel_drivers: &[],
            });
            assert_eq!(
                result,
                match renderer {
                    "NVK" => DriverClass::Nvk,
                    "SWRAST" => DriverClass::Software,
                    _ => DriverClass::Unknown,
                }
            );
        }
        let oversized = format!("{}llvmpipe", "x".repeat(4096));
        assert!(!is_software_renderer(&oversized));
        let unicode = "🚀".repeat(5000);
        assert_eq!(
            classify_driver(DriverEvidence {
                vendor: &unicode,
                renderer: &unicode,
                version: &unicode,
                kernel_drivers: &[]
            }),
            DriverClass::Unknown
        );
    }

    #[test]
    fn policy_exceptions_apply_only_to_evidenced_versions_and_sessions() {
        for session in [SessionType::Wayland, SessionType::X11] {
            let mut target = context(DriverClass::VirtioVirgl);
            target.session = session;
            assert_eq!(renderer_policy(target).0, RendererPath::DisableDmabuf);
            for version in [
                None,
                Some(GraphicsVersion::new(2, 52, 6)),
                Some(GraphicsVersion::new(2, 54, 3)),
                Some(GraphicsVersion::new(2, 56, 0)),
            ] {
                target.webkit_version = version;
                assert_eq!(renderer_policy(target).0, RendererPath::Default);
            }
        }
        let mut target = context(DriverClass::VirtioVirgl);
        target.session = SessionType::Unknown;
        assert_eq!(renderer_policy(target).0, RendererPath::Default);
        for driver in [
            DriverClass::MesaRadeonsi,
            DriverClass::MesaIris,
            DriverClass::MesaCrocus,
            DriverClass::MesaI915,
            DriverClass::NvidiaProprietary,
            DriverClass::Nouveau,
            DriverClass::Nvk,
            DriverClass::Software,
            DriverClass::Asahi,
            DriverClass::OtherMesa,
            DriverClass::Unknown,
        ] {
            assert_eq!(
                renderer_policy(context(driver)).0,
                RendererPath::Default,
                "driver {driver:?}"
            );
        }
    }

    #[test]
    fn version_predicates_require_known_matching_versions() {
        let minimum = GraphicsVersion::new(25, 1, 0);
        let maximum_exclusive = GraphicsVersion::new(25, 3, 0);
        let range = VersionPredicate::Range {
            minimum,
            maximum_exclusive,
        };
        assert!(VersionPredicate::Any.matches(None));
        assert!(!VersionPredicate::Exact(minimum).matches(None));
        assert!(!range.matches(None));
        assert!(!range.matches(Some(GraphicsVersion::new(25, 0, 9))));
        assert!(range.matches(Some(minimum)));
        assert!(range.matches(Some(GraphicsVersion::new(25, 2, 9))));
        assert!(!range.matches(Some(maximum_exclusive)));
        for (raw, expected) in [
            ("2.54.2", Some(GraphicsVersion::new(2, 54, 2))),
            ("580.95.05", Some(GraphicsVersion::new(580, 95, 5))),
            ("", None),
            ("2.54", None),
            ("2.54.2.0", None),
            ("2.54.2-devel", None),
            ("2.-54.2", None),
            ("2.+54.2", None),
            ("2. 54.2", None),
            ("2.4294967296.2", None),
        ] {
            assert_eq!(GraphicsVersion::parse(raw), expected, "version {raw}");
        }
    }

    #[test]
    fn explicit_environment_always_wins_including_zero_and_empty() {
        let recovery = CrashRecovery::default().record_incomplete(RendererPath::ForceShm);
        for (disable_dmabuf, force_shm, expected) in [
            (Some("1"), Some("1"), RendererPath::DisableDmabuf),
            (Some(""), None, RendererPath::DisableDmabuf),
            (Some("false"), None, RendererPath::DisableDmabuf),
            (Some("0"), Some("1"), RendererPath::ForceShm),
            (Some("0"), Some(""), RendererPath::ForceShm),
            (Some("0"), Some("0"), RendererPath::Default),
            (Some("0"), None, RendererPath::Default),
            (None, Some("0"), RendererPath::Default),
        ] {
            let selected = select_renderer(
                context(DriverClass::VirtioVirgl),
                RendererPreference::Safe,
                RendererOverrides {
                    disable_dmabuf,
                    force_shm,
                },
                recovery,
            );
            assert_eq!(selected.path, expected);
            assert_eq!(selected.source, SelectionSource::Environment);
        }
    }

    #[test]
    fn preferences_override_policy_and_crash_recovery() {
        let recovery = CrashRecovery::default().record_incomplete(RendererPath::ForceShm);
        for (preference, expected) in [
            (RendererPreference::Gpu, RendererPath::Default),
            (RendererPreference::Compatibility, RendererPath::ForceShm),
            (RendererPreference::Safe, RendererPath::DisableDmabuf),
        ] {
            let selected = select_renderer(
                context(DriverClass::VirtioVirgl),
                preference,
                RendererOverrides::default(),
                recovery,
            );
            assert_eq!(selected.path, expected);
            assert_eq!(selected.source, SelectionSource::Preference);
        }
        let selected = select_renderer(
            context(DriverClass::VirtioVirgl),
            RendererPreference::Automatic,
            RendererOverrides::default(),
            CrashRecovery::default(),
        );
        assert_eq!(selected.path, RendererPath::DisableDmabuf);
        assert_eq!(selected.source, SelectionSource::Policy);
    }

    #[test]
    fn crash_fallback_steps_down_is_bounded_and_success_resets_it() {
        let mut recovery = CrashRecovery::default();
        let target = context(DriverClass::Unknown);
        let select = |recovery| {
            select_renderer(
                target,
                RendererPreference::Automatic,
                RendererOverrides::default(),
                recovery,
            )
        };
        assert_eq!(select(recovery).path, RendererPath::Default);
        recovery = recovery.record_incomplete(RendererPath::Default);
        assert_eq!(select(recovery).path, RendererPath::ForceShm);
        assert_eq!(select(recovery).source, SelectionSource::CrashRecovery);
        recovery = recovery.record_incomplete(RendererPath::ForceShm);
        assert_eq!(select(recovery).path, RendererPath::DisableDmabuf);
        for _ in 0..300 {
            recovery = recovery.record_incomplete(RendererPath::DisableDmabuf);
        }
        assert_eq!(recovery.fallback_steps, MAX_CRASH_FALLBACK_STEPS);
        assert_eq!(select(recovery).path, RendererPath::DisableDmabuf);
        assert_eq!(select(recovery.reset()).path, RendererPath::Default);
        assert_eq!(select(recovery.reset()).source, SelectionSource::Policy);
        let malformed = CrashRecovery {
            fallback_steps: 255,
            previous_incomplete_path: None,
            held_path: None,
        };
        assert_eq!(
            select(malformed).crash_recovery.fallback_steps,
            MAX_CRASH_FALLBACK_STEPS
        );
    }

    #[test]
    fn a_held_path_outranks_policy_and_a_single_step_down() {
        let target = context(DriverClass::Unknown);
        let select = |recovery| {
            select_renderer(
                target,
                RendererPreference::Automatic,
                RendererOverrides::default(),
                recovery,
            )
        };
        let held = CrashRecovery {
            held_path: Some(RendererPath::ForceShm),
            ..CrashRecovery::default()
        };
        let selected = select(held);
        assert_eq!(selected.path, RendererPath::ForceShm);
        assert_eq!(selected.source, SelectionSource::RepeatedFailure);
        // A further failure this launch steps below the held path as before.
        let deeper = held.record_incomplete(RendererPath::ForceShm);
        assert_eq!(deeper.held_path, Some(RendererPath::ForceShm));
        let selected = select(deeper);
        assert_eq!(selected.path, RendererPath::DisableDmabuf);
        assert_eq!(selected.source, SelectionSource::CrashRecovery);
        // A hold no stricter than policy is not reported as one.
        let virgl = select_renderer(
            context(DriverClass::VirtioVirgl),
            RendererPreference::Automatic,
            RendererOverrides::default(),
            held,
        );
        assert_eq!(virgl.path, RendererPath::DisableDmabuf);
        assert_eq!(virgl.source, SelectionSource::Policy);
        // Explicit choices and environment overrides still win.
        let chosen = select_renderer(
            target,
            RendererPreference::Gpu,
            RendererOverrides::default(),
            held,
        );
        assert_eq!(chosen.path, RendererPath::Default);
    }

    #[test]
    fn previous_failed_transport_cannot_upgrade_when_policy_changes() {
        let recovery = CrashRecovery::default().record_incomplete(RendererPath::ForceShm);
        let selected = select_renderer(
            context(DriverClass::Unknown),
            RendererPreference::Automatic,
            RendererOverrides::default(),
            recovery,
        );
        assert_eq!(selected.path, RendererPath::DisableDmabuf);
        let recovery = CrashRecovery::default().record_incomplete(RendererPath::DisableDmabuf);
        let selected = select_renderer(
            context(DriverClass::Unknown),
            RendererPreference::Automatic,
            RendererOverrides::default(),
            recovery,
        );
        assert_eq!(selected.path, RendererPath::DisableDmabuf);
    }

    #[test]
    fn settings_contract_has_strict_preference_values() {
        assert_eq!(
            serde_json::to_string(&RendererPreference::Automatic).unwrap(),
            "\"automatic\""
        );
        assert_eq!(
            serde_json::to_string(&RendererPreference::Gpu).unwrap(),
            "\"gpu\""
        );
        for invalid in ["\"Auto\"", "\"software\"", "0", "null", "{}"] {
            assert!(serde_json::from_str::<RendererPreference>(invalid).is_err());
        }
        assert!(serde_json::from_str::<CrashRecovery>(
            r#"{"fallback_steps":0,"previous_incomplete_path":null,"extra":true}"#
        )
        .is_err());
    }
}
