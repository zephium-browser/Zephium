use std::ffi::OsString;
use zephium_core::linux_graphics::{
    AtlasWorkaroundReason, RendererPath, RendererSelection, SelectionSource,
};
use zephium_ipc::linux_graphics::GraphicsEnvironmentOverride;

pub(super) const ATLAS_VARIABLE: &str = "WEBKIT_DISABLE_DMABUF_ATLAS";

const VARIABLES: &[&str] = &[
    ATLAS_VARIABLE,
    "WEBKIT_DISABLE_DMABUF_RENDERER",
    "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
    "WEBKIT_DISABLE_COMPOSITING_MODE",
    "WEBKIT_FORCE_COMPOSITING_MODE",
    "GDK_GL",
    "GDK_BACKEND",
    "GDK_SCALE",
    "GDK_DPI_SCALE",
    "LIBGL_ALWAYS_SOFTWARE",
    "LIBGL_ALWAYS_INDIRECT",
    "MESA_LOADER_DRIVER_OVERRIDE",
    "GALLIUM_DRIVER",
    "DRI_PRIME",
    "__NV_PRIME_RENDER_OFFLOAD",
    "__GLX_VENDOR_LIBRARY_NAME",
    "GBM_BACKEND",
    "EGL_PLATFORM",
    "VK_ICD_FILENAMES",
    "VK_DRIVER_FILES",
    "MESA_GL_VERSION_OVERRIDE",
    "MESA_GLES_VERSION_OVERRIDE",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AtlasStatus {
    Unchanged,
    Policy(AtlasWorkaroundReason),
    Environment { disabled: bool },
}

impl AtlasStatus {
    pub(super) fn description(self, report_only: bool) -> String {
        match self {
            Self::Unchanged => "Small-image atlas: WebKit default".into(),
            Self::Policy(reason) => format!(
                "Small-image workaround: {} DMA-BUF atlas textures to {} (WEBKIT_DISABLE_DMABUF_ATLAS=1); renderer path is selected separately",
                if report_only { "would disable" } else { "disabled" },
                reason.description(),
            ),
            Self::Environment { disabled } => format!(
                "Small-image atlas: {}; user-set WEBKIT_DISABLE_DMABUF_ATLAS preserved (only exactly 1 disables atlas textures)",
                if disabled { "DMA-BUF atlas textures disabled" } else { "DMA-BUF atlas textures not disabled" },
            ),
        }
    }
}

pub(super) struct StartupEnvironment {
    names: Vec<&'static str>,
    pub atlas: AtlasStatus,
}

impl StartupEnvironment {
    pub(super) fn plan(
        selection: &RendererSelection,
        workaround: Option<AtlasWorkaroundReason>,
        mut read: impl FnMut(&str) -> Option<OsString>,
    ) -> Self {
        let mut names = Vec::new();
        if selection.source != SelectionSource::Environment {
            let renderer = match selection.path {
                RendererPath::Default => None,
                RendererPath::ForceShm => Some("WEBKIT_DMABUF_RENDERER_FORCE_SHM"),
                RendererPath::DisableDmabuf => Some("WEBKIT_DISABLE_DMABUF_RENDERER"),
            };
            if let Some(name) = renderer.filter(|name| read(name).is_none()) {
                names.push(name);
            }
        }
        let atlas = if let Some(value) = read(ATLAS_VARIABLE) {
            AtlasStatus::Environment {
                disabled: value == "1",
            }
        } else if let Some(reason) = workaround {
            names.push(ATLAS_VARIABLE);
            AtlasStatus::Policy(reason)
        } else {
            AtlasStatus::Unchanged
        };
        Self { names, atlas }
    }

    /// Called only before toolkit/runtime startup; injectable for tests without global mutation.
    pub(super) fn apply(&self, mut set: impl FnMut(&str, &str)) -> Vec<&'static str> {
        for name in &self.names {
            set(name, "1");
        }
        self.names.clone()
    }
}

pub(super) fn capture() -> Vec<GraphicsEnvironmentOverride> {
    VARIABLES
        .iter()
        .filter_map(|name| {
            std::env::var_os(name).map(|value| GraphicsEnvironmentOverride {
                name: (*name).into(),
                value: summarize(name, &value.to_string_lossy()),
            })
        })
        .collect()
}

fn summarize(name: &str, value: &str) -> String {
    if value.len() > 256 {
        return "<set; value redacted>".into();
    }
    if value.is_empty() {
        return "<empty>".into();
    }
    let allowed = match name {
        "GDK_GL" => value.split(',').all(|part| {
            [
                "gles",
                "always",
                "disable",
                "software",
                "texture-rectangle",
                "legacy",
            ]
            .contains(&part)
        }),
        "GDK_BACKEND" => value
            .split(',')
            .all(|part| ["wayland", "x11", "broadway", "*"].contains(&part)),
        "GDK_SCALE" | "GDK_DPI_SCALE" => {
            value.len() <= 8
                && value
                    .parse::<f64>()
                    .is_ok_and(|scale| scale.is_finite() && (0.1..=16.0).contains(&scale))
        }
        "MESA_LOADER_DRIVER_OVERRIDE" | "GALLIUM_DRIVER" | "GBM_BACKEND" => [
            "radeonsi",
            "iris",
            "crocus",
            "i915",
            "nouveau",
            "nvk",
            "zink",
            "virgl",
            "virtio_gpu",
            "llvmpipe",
            "softpipe",
            "swrast",
            "asahi",
            "nvidia-drm",
        ]
        .contains(&value),
        "__GLX_VENDOR_LIBRARY_NAME" => ["nvidia", "mesa"].contains(&value),
        "EGL_PLATFORM" => ["wayland", "x11", "surfaceless", "drm"].contains(&value),
        "VK_ICD_FILENAMES" | "VK_DRIVER_FILES" => false,
        _ => ["0", "1", "true", "false"].contains(&value),
    };
    if allowed {
        value.into()
    } else {
        "<set; value redacted>".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStringExt;
    use zephium_core::linux_graphics::{
        atlas_workaround_policy, select_renderer, CrashRecovery, DriverClass, GraphicsVersion,
        PolicyContext, RendererOverrides, RendererPreference, SessionType,
    };

    fn context() -> PolicyContext {
        PolicyContext {
            driver_class: DriverClass::VirtioVirgl,
            session: SessionType::Wayland,
            webkit_version: Some(GraphicsVersion::new(2, 53, 2)),
            mesa_version: None,
            nvidia_version: None,
        }
    }

    fn apply(
        context: PolicyContext,
        preference: RendererPreference,
        recovery: CrashRecovery,
        variables: &mut BTreeMap<String, OsString>,
    ) -> (StartupEnvironment, Vec<&'static str>) {
        let disable = variables
            .get("WEBKIT_DISABLE_DMABUF_RENDERER")
            .map(|v| v.to_string_lossy());
        let shm = variables
            .get("WEBKIT_DMABUF_RENDERER_FORCE_SHM")
            .map(|v| v.to_string_lossy());
        let selection = select_renderer(
            context,
            preference,
            RendererOverrides {
                disable_dmabuf: disable.as_deref(),
                force_shm: shm.as_deref(),
            },
            recovery,
        );
        let plan = StartupEnvironment::plan(&selection, atlas_workaround_policy(context), |name| {
            variables.get(name).cloned()
        });
        let applied = plan.apply(|name, value| {
            variables.insert(name.into(), value.into());
        });
        (plan, applied)
    }

    #[test]
    fn atlas_is_independent_of_preferences_and_recovery() {
        for (preference, renderer) in [
            (RendererPreference::Automatic, None),
            (RendererPreference::Gpu, None),
            (
                RendererPreference::Compatibility,
                Some("WEBKIT_DMABUF_RENDERER_FORCE_SHM"),
            ),
            (
                RendererPreference::Safe,
                Some("WEBKIT_DISABLE_DMABUF_RENDERER"),
            ),
        ] {
            let mut variables = BTreeMap::new();
            let (plan, applied) = apply(
                context(),
                preference,
                CrashRecovery::default(),
                &mut variables,
            );
            assert_eq!(
                plan.atlas,
                AtlasStatus::Policy(AtlasWorkaroundReason::VirtioMissingImages)
            );
            assert_eq!(variables.get(ATLAS_VARIABLE), Some(&OsString::from("1")));
            assert_eq!(applied.len(), 1 + usize::from(renderer.is_some()));
            assert_eq!(variables.len(), applied.len());
            if let Some(renderer) = renderer {
                assert_eq!(variables.get(renderer), Some(&OsString::from("1")));
            }
        }
        let mut variables = BTreeMap::new();
        let (_, applied) = apply(
            context(),
            RendererPreference::Automatic,
            CrashRecovery::default().record_incomplete(RendererPath::Default),
            &mut variables,
        );
        assert_eq!(
            applied,
            ["WEBKIT_DMABUF_RENDERER_FORCE_SHM", ATLAS_VARIABLE]
        );
    }

    #[test]
    fn user_atlas_values_are_preserved_byte_for_byte_and_never_removed_on_restart() {
        for value in [
            "1".into(),
            "0".into(),
            "".into(),
            "true".into(),
            "01".into(),
            "1\n".into(),
            OsString::from_vec(vec![0xff]),
        ] {
            let mut variables = BTreeMap::from([(ATLAS_VARIABLE.into(), value.clone())]);
            let (plan, applied) = apply(
                context(),
                RendererPreference::Compatibility,
                CrashRecovery::default(),
                &mut variables,
            );
            assert_eq!(variables.get(ATLAS_VARIABLE), Some(&value));
            assert_eq!(
                plan.atlas,
                AtlasStatus::Environment {
                    disabled: value == "1"
                }
            );
            assert_eq!(applied, ["WEBKIT_DMABUF_RENDERER_FORCE_SHM"]);
        }
    }

    #[test]
    fn renderer_environment_overrides_do_not_suppress_atlas_workaround() {
        for (name, value) in [
            ("WEBKIT_DISABLE_DMABUF_RENDERER", "0"),
            ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
            ("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1"),
            ("WEBKIT_DMABUF_RENDERER_FORCE_SHM", ""),
        ] {
            let mut variables = BTreeMap::from([(name.into(), value.into())]);
            let (_, applied) = apply(
                context(),
                RendererPreference::Safe,
                CrashRecovery::default(),
                &mut variables,
            );
            assert_eq!(applied, [ATLAS_VARIABLE]);
            assert_eq!(variables.get(name), Some(&OsString::from(value)));
            assert_eq!(variables.len(), 2);
        }
    }

    #[test]
    fn unaffected_or_unknown_versions_and_other_gpus_get_no_environment_switch() {
        let mut old = context();
        old.webkit_version = Some(GraphicsVersion::new(2, 53, 1));
        let mut unknown = context();
        unknown.webkit_version = None;
        let mut other = context();
        other.driver_class = DriverClass::MesaIris;
        for context in [old, unknown, other] {
            let mut variables = BTreeMap::new();
            let (plan, applied) = apply(
                context,
                RendererPreference::Automatic,
                CrashRecovery::default(),
                &mut variables,
            );
            assert!(variables.is_empty());
            assert!(applied.is_empty());
            assert_eq!(plan.atlas, AtlasStatus::Unchanged);
        }
    }

    #[test]
    fn diagnostics_distinguish_active_planned_and_overridden_atlas_workarounds() {
        let atlas = AtlasStatus::Policy(AtlasWorkaroundReason::VirtioMissingImages);
        assert!(atlas
            .description(false)
            .contains("disabled DMA-BUF atlas textures to prevent missing small images"));
        assert!(atlas.description(true).contains("would disable"));
        assert!(AtlasStatus::Environment { disabled: false }
            .description(false)
            .contains("not disabled"));
        assert!(AtlasStatus::Environment { disabled: true }
            .description(false)
            .contains("user-set"));
    }

    #[test]
    fn overrides_report_only_allowlisted_values() {
        for secret in [
            "/home/person/driver.so",
            "alice@private-host",
            "aa:bb:cc:dd:ee:ff",
            "serial-12345",
            "1\nusername=alice",
        ] {
            for name in VARIABLES {
                assert_eq!(summarize(name, secret), "<set; value redacted>");
            }
        }
        assert_eq!(summarize("GDK_GL", "gles"), "gles");
        assert_eq!(summarize("GDK_BACKEND", "wayland,x11"), "wayland,x11");
        assert_eq!(summarize("GDK_SCALE", "2"), "2");
        assert_eq!(summarize("GDK_DPI_SCALE", "NaN"), "<set; value redacted>");
        assert_eq!(summarize("WEBKIT_DISABLE_DMABUF_RENDERER", "0"), "0");
        assert_eq!(summarize("WEBKIT_DMABUF_RENDERER_FORCE_SHM", ""), "<empty>");
        assert_eq!(
            summarize("GDK_GL", &"gles,".repeat(100)),
            "<set; value redacted>"
        );
    }
}
