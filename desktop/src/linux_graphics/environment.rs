use zephium_ipc::linux_graphics::GraphicsEnvironmentOverride;

const VARIABLES: &[&str] = &[
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
