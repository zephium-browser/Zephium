use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use gtk::prelude::*;
use zephium_core::linux_graphics::{
    classify_driver, is_software_renderer, DriverEvidence, SessionType,
};
pub use zephium_ipc::linux_graphics::*;

const MAX_DEVICES: usize = 16;
const MAX_DRM_ENTRIES: usize = 256;
const MAX_MONITORS: i32 = 16;
const MAX_STRING_BYTES: usize = 256;
const MAX_FILE_BYTES: usize = 512;

#[link(name = "epoxy")]
unsafe extern "C" {
    static epoxy_glGetString: unsafe extern "C" fn(u32) -> *const u8;
}

#[link(name = "dl")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

pub fn collect(probe: Option<ProbeData>) -> Detection {
    let probe = probe.unwrap_or_else(inconclusive_probe);
    let kernel = kernel_graphics_at(Path::new("/sys/class/drm"), Path::new("/sys/module"));
    let kernel_drivers: Vec<_> = kernel
        .devices
        .iter()
        .filter_map(|device| device.driver.as_deref())
        .collect();
    let gl = probe.gl.as_ref();
    let driver_class = classify_driver(DriverEvidence {
        vendor: gl.and_then(|gl| gl.vendor.as_deref()).unwrap_or_default(),
        renderer: gl.and_then(|gl| gl.renderer.as_deref()).unwrap_or_default(),
        version: gl.and_then(|gl| gl.version.as_deref()).unwrap_or_default(),
        kernel_drivers: &kernel_drivers,
    });
    // These accessors return library constants without initializing either toolkit.
    let webkit_version = unsafe {
        format!(
            "{}.{}.{}",
            webkit2gtk::ffi::webkit_get_major_version(),
            webkit2gtk::ffi::webkit_get_minor_version(),
            webkit2gtk::ffi::webkit_get_micro_version(),
        )
    };
    Detection {
        kernel,
        gl: probe.gl,
        session: probe.session,
        gtk_version: format!(
            "{}.{}.{}",
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
        ),
        webkit_version,
        monitors: probe.monitors,
        monitors_truncated: probe.monitors_truncated,
        compositor: probe.compositor,
        video_decode: video_decode(),
        probe_outcome: probe.outcome,
        driver_class,
    }
}

fn inconclusive_probe() -> ProbeData {
    ProbeData {
        outcome: ProbeOutcome::Inconclusive,
        session: SessionType::Unknown,
        gl: None,
        monitors: Vec::new(),
        monitors_truncated: false,
        compositor: None,
    }
}

pub fn probe() -> ProbeData {
    let mut result = inconclusive_probe();
    if gtk::init().is_err() {
        return result;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return result;
    };
    result.session = match display.type_().name() {
        "GdkWaylandDisplay" => SessionType::Wayland,
        "GdkX11Display" => SessionType::X11,
        _ => SessionType::Unknown,
    };
    let primary = display.primary_monitor();
    let monitor_count = display.n_monitors();
    result.monitors_truncated = monitor_count > MAX_MONITORS;
    for index in 0..monitor_count.clamp(0, MAX_MONITORS) {
        let Some(monitor) = display.monitor(index) else {
            continue;
        };
        let geometry = monitor.geometry();
        result.monitors.push(MonitorInfo {
            index: index as u32,
            x: geometry.x(),
            y: geometry.y(),
            width: geometry.width(),
            height: geometry.height(),
            integer_scale: monitor.scale_factor(),
            // GTK3 does not expose the compositor's fractional output scale.
            fractional_scale: None,
            refresh_rate_millihertz: monitor.refresh_rate(),
            primary: primary.as_ref() == Some(&monitor),
        });
    }
    result.compositor = compositor(&display, result.session);
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_accept_focus(false);
    window.set_focus_on_map(false);
    window.set_default_size(1, 1);
    window.realize();
    if window.is_visible() || window.is_mapped() {
        return result;
    }
    let Some(native) = window.window() else {
        return result;
    };
    let Ok(context) = native.create_gl_context() else {
        result.outcome = ProbeOutcome::Unavailable;
        return result;
    };
    if context.realize().is_err()
        || (std::env::var_os("GDK_GL").is_some_and(|value| value == "gles") && !context.uses_es())
    {
        result.outcome = ProbeOutcome::Unavailable;
        return result;
    }
    context.make_current();
    if gtk::gdk::GLContext::current().as_ref() != Some(&context) {
        result.outcome = ProbeOutcome::Unavailable;
        return result;
    }
    let privacy = Privacy::from_environment();
    // Epoxy dispatches for the current GDK context, including GLES contexts.
    let (vendor, renderer, version) = unsafe {
        (
            bounded_c_string(epoxy_glGetString(0x1f00)),
            bounded_c_string(epoxy_glGetString(0x1f01)),
            bounded_c_string(epoxy_glGetString(0x1f02)),
        )
    };
    gtk::gdk::GLContext::clear_current();
    result.gl = Some(gl_info(
        vendor.as_deref(),
        renderer.as_deref(),
        version.as_deref(),
        context.uses_es(),
        &privacy,
    ));
    result.outcome = ProbeOutcome::Supported;
    result
}

fn compositor(display: &gtk::gdk::Display, session: SessionType) -> Option<String> {
    use gtk::glib::translate::ToGlibPtr;

    if session != SessionType::X11 {
        return None;
    }
    let screen = display.default_screen();
    if !screen.is_composited() {
        return None;
    }
    // An optional backend symbol keeps Wayland-only GTK builds usable.
    let symbol = unsafe {
        dlsym(
            std::ptr::null_mut(),
            c"gdk_x11_screen_get_window_manager_name".as_ptr(),
        )
    };
    if symbol.is_null() {
        return None;
    }
    type GetName = unsafe extern "C" fn(*mut gtk::gdk::ffi::GdkScreen) -> *const c_char;
    let get_name: GetName = unsafe { std::mem::transmute(symbol) };
    let name = unsafe { bounded_c_string(get_name(screen.to_glib_none().0).cast()) }?;
    match name.as_str() {
        "GNOME Shell" | "Mutter" => Some("Mutter".to_owned()),
        "KWin" => Some("KWin".to_owned()),
        "Xfwm4" => Some("Xfwm4".to_owned()),
        _ => None,
    }
}

fn kernel_graphics_at(drm: &Path, modules: &Path) -> KernelGraphics {
    let Ok(entries) = fs::read_dir(drm) else {
        return KernelGraphics {
            status: KernelStatus::Unavailable,
            devices: Vec::new(),
            truncated: false,
        };
    };
    let mut devices = BTreeMap::<PathBuf, KernelDevice>::new();
    let mut partial = false;
    let mut truncated = false;
    for (index, entry) in entries.enumerate() {
        if index == MAX_DRM_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_node(&name, "card") && !is_node(&name, "renderD") {
            continue;
        }
        let device_path = entry.path().join("device");
        let Ok(identity) = device_path.canonicalize() else {
            partial = true;
            continue;
        };
        if !devices.contains_key(&identity) && devices.len() == MAX_DEVICES {
            truncated = true;
            continue;
        }
        let device = devices.entry(identity).or_insert_with(|| {
            let driver = kernel_driver(&device_path);
            let driver_version = driver.as_deref().and_then(|driver| {
                read_small(&modules.join(driver).join("version"))
                    .and_then(|version| numeric_version(version.trim()))
            });
            KernelDevice {
                driver,
                driver_version,
                vendor_id: read_small(&device_path.join("vendor")).and_then(|id| pci_id(&id)),
                device_id: read_small(&device_path.join("device")).and_then(|id| pci_id(&id)),
                card_nodes: Vec::new(),
                render_nodes: Vec::new(),
            }
        });
        let nodes = if name.starts_with("card") {
            &mut device.card_nodes
        } else {
            &mut device.render_nodes
        };
        if nodes.len() < MAX_DEVICES {
            nodes.push(name);
        } else {
            truncated = true;
        }
    }
    let mut devices: Vec<_> = devices.into_values().collect();
    for device in &mut devices {
        device.card_nodes.sort();
        device.render_nodes.sort();
    }
    let status = if partial || truncated {
        KernelStatus::Partial
    } else if devices.is_empty() {
        KernelStatus::NoGpu
    } else {
        KernelStatus::Available
    };
    KernelGraphics {
        status,
        devices,
        truncated,
    }
}

fn kernel_driver(device: &Path) -> Option<String> {
    let direct = driver_name(&device.join("driver"));
    if direct
        .as_deref()
        .is_some_and(|name| name.starts_with("virtio"))
    {
        if let Ok(entries) = fs::read_dir(device) {
            for entry in entries.take(32).flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| is_node(name, "virtio"))
                {
                    if let Some(driver) = driver_name(&entry.path().join("driver")) {
                        return Some(driver);
                    }
                }
            }
        }
    }
    direct
}

fn driver_name(path: &Path) -> Option<String> {
    let link = fs::read_link(path).ok()?;
    let name = link.file_name()?.to_str()?;
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(name.to_owned())
}

fn is_node(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty() && suffix.len() <= 5 && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn pci_id(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix("0x")?;
    (value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("0x{}", value.to_ascii_lowercase()))
}

fn read_small(path: &Path) -> Option<String> {
    let mut value = String::new();
    File::open(path)
        .ok()?
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_string(&mut value)
        .ok()?;
    (value.len() <= MAX_FILE_BYTES).then_some(value)
}

fn video_decode() -> VideoDecodeInfo {
    let directories = [
        "/usr/lib64/gstreamer-1.0",
        "/usr/lib/gstreamer-1.0",
        "/usr/lib/x86_64-linux-gnu/gstreamer-1.0",
        "/usr/lib/aarch64-linux-gnu/gstreamer-1.0",
        "/usr/lib/arm-linux-gnueabihf/gstreamer-1.0",
        "/usr/local/lib/gstreamer-1.0",
        "/usr/local/lib64/gstreamer-1.0",
    ];
    let paths: Vec<_> = directories.iter().map(Path::new).collect();
    VideoDecodeInfo {
        va_plugin: plugin_presence(&paths, "libgstva.so"),
        vaapi_plugin: plugin_presence(&paths, "libgstvaapi.so"),
        capabilities_queried: false,
    }
}

fn plugin_presence(directories: &[&Path], filename: &str) -> PluginPresence {
    let mut searched = false;
    let mut unavailable = false;
    for directory in directories {
        match fs::metadata(directory) {
            Ok(metadata) if metadata.is_dir() => searched = true,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                unavailable = true;
                continue;
            }
        }
        match fs::metadata(directory.join(filename)) {
            Ok(metadata) if metadata.is_file() => return PluginPresence::PresentInSystemDirectory,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => unavailable = true,
        }
    }
    if searched && !unavailable {
        PluginPresence::NotFoundInSystemDirectories
    } else {
        PluginPresence::Unknown
    }
}

fn gl_info(
    vendor: Option<&str>,
    renderer: Option<&str>,
    version: Option<&str>,
    gles: bool,
    privacy: &Privacy,
) -> GlInfo {
    let vendor = vendor.and_then(|value| privacy.sanitize(value));
    let renderer = renderer.and_then(|value| privacy.sanitize(value));
    let version = version.and_then(|value| privacy.sanitize(value));
    let renderer_lower = renderer.as_deref().unwrap_or_default().to_ascii_lowercase();
    GlInfo {
        mesa_version: version
            .as_deref()
            .and_then(|value| version_after(value, "Mesa ")),
        nvidia_version: version
            .as_deref()
            .and_then(|value| version_after(value, "NVIDIA ")),
        zink: renderer_lower.contains("zink"),
        software: is_software_renderer(&renderer_lower),
        vendor,
        renderer,
        version,
        api: if gles { GlApi::Gles } else { GlApi::Desktop },
    }
}

fn version_after(value: &str, marker: &str) -> Option<String> {
    numeric_version(value.split_once(marker)?.1.split_whitespace().next()?)
}

fn numeric_version(value: &str) -> Option<String> {
    let numeric: String = value
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let components: Vec<_> = numeric.split('.').collect();
    (numeric.len() <= 32
        && (2..=4).contains(&components.len())
        && components
            .iter()
            .all(|part| !part.is_empty() && part.parse::<u32>().is_ok()))
    .then_some(numeric)
}

struct Privacy {
    identities: Vec<String>,
}

impl Privacy {
    fn from_environment() -> Self {
        let mut identities: Vec<_> = ["USER", "LOGNAME", "HOSTNAME"]
            .iter()
            .filter_map(std::env::var_os)
            .filter_map(|value| value.into_string().ok())
            .collect();
        if let Some(home) = std::env::var_os("HOME") {
            if let Some(name) = Path::new(&home).file_name().and_then(|name| name.to_str()) {
                identities.push(name.to_owned());
            }
        }
        if let Some(hostname) = read_small(Path::new("/etc/hostname")) {
            identities.push(hostname.trim().to_owned());
        }
        identities.retain(|identity| !identity.is_empty() && identity.len() <= MAX_STRING_BYTES);
        Self { identities }
    }

    fn sanitize(&self, value: &str) -> Option<String> {
        let normalized: String = value
            .chars()
            .filter(|character| character.is_ascii_graphic() || character.is_ascii_whitespace())
            .collect();
        let value = normalized.as_str();
        let lower = value.to_ascii_lowercase();
        if ["serial", "hostname", "username"]
            .iter()
            .any(|label| lower.contains(label))
        {
            return Some("[redacted]".to_owned());
        }
        let mut output = String::new();
        for word in value.split_whitespace() {
            if !output.is_empty() {
                if output.len() >= MAX_STRING_BYTES {
                    break;
                }
                output.push(' ');
            }
            let trimmed = word.trim_matches(|character: char| !character.is_ascii_alphanumeric());
            let word_lower = word.to_ascii_lowercase();
            let private = self
                .identities
                .iter()
                .any(|identity| word_lower.contains(&identity.to_ascii_lowercase()))
                || word.contains('@')
                || word.contains('\\')
                || (word.contains('/') && !permitted_graphics_slash_token(word))
                || word.contains('~')
                || hardware_identifier(trimmed);
            let word = if private { "[redacted]" } else { word };
            for character in word
                .chars()
                .filter(|character| character.is_ascii_graphic())
            {
                if output.len() == MAX_STRING_BYTES {
                    break;
                }
                output.push(character);
            }
            if output.len() >= MAX_STRING_BYTES {
                break;
            }
        }
        let output = output.trim();
        (!output.is_empty()).then(|| output.to_owned())
    }
}

fn hardware_identifier(value: &str) -> bool {
    if value
        .split(|character: char| !character.is_ascii_hexdigit())
        .any(|part| part.len() >= 12)
    {
        return true;
    }
    for length in [17, 23, 36] {
        for candidate in value.as_bytes().windows(length) {
            let uuid = length == 36;
            let separator = candidate[if uuid { 8 } else { 2 }];
            if !matches!(separator, b':' | b'-') || (uuid && separator != b'-') {
                continue;
            }
            if candidate.iter().enumerate().all(|(index, byte)| {
                let separator_index = if uuid {
                    matches!(index, 8 | 13 | 18 | 23)
                } else {
                    index % 3 == 2
                };
                if separator_index {
                    *byte == separator
                } else {
                    byte.is_ascii_hexdigit()
                }
            }) {
                return true;
            }
        }
    }
    false
}

fn permitted_graphics_slash_token(value: &str) -> bool {
    matches!(value, "PCIe/SSE2" | "Mesa/X.org")
        || value.strip_suffix("/PCIe/SSE2").is_some_and(|model| {
            !model.is_empty()
                && model.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'(' | b')' | b'.')
                })
        })
}

unsafe fn bounded_c_string(pointer: *const u8) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut value = Vec::new();
    for index in 0..MAX_FILE_BYTES {
        // GL and GDK own these NUL-terminated strings for the current context/display.
        let byte = unsafe { *pointer.add(index) };
        if byte == 0 {
            return String::from_utf8(value).ok();
        }
        value.push(byte);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn privacy() -> Privacy {
        Privacy {
            identities: vec!["alice".to_owned(), "private-host".to_owned()],
        }
    }

    #[test]
    fn realistic_gl_strings_keep_driver_versions_and_renderer_identity() {
        let fixtures = [
            (
                "AMD",
                "AMD Radeon RX 6800 (radeonsi, navi21, LLVM 20.1.8, DRM 3.64)",
                "4.6 (Core Profile) Mesa 25.1.7",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "Intel",
                "Mesa Intel(R) Xe Graphics (TGL GT2)",
                "4.6 (Core Profile) Mesa 25.1.7",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "Intel",
                "Mesa Intel(R) HD Graphics 4000 (IVB GT2)",
                "4.2 (Core Profile) Mesa 25.1.7 (crocus)",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "NVIDIA Corporation",
                "NVIDIA GeForce RTX 4090/PCIe/SSE2",
                "4.6.0 NVIDIA 580.95.05",
                false,
                None,
                Some("580.95.05"),
            ),
            (
                "nouveau",
                "NV137",
                "4.3 (Core Profile) Mesa 25.1.7",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "Mesa",
                "zink Vulkan 1.4(NVIDIA GeForce RTX 3060 (NVK GA106))",
                "4.6 (Core Profile) Mesa 25.1.7",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "Mesa",
                "virgl (Apple M1 (Compat))",
                "OpenGL ES 3.2 Mesa 26.2.6",
                true,
                Some("26.2.6"),
                None,
            ),
            (
                "Mesa/X.org",
                "llvmpipe (LLVM 20.1.8, 128 bits)",
                "4.5 (Core Profile) Mesa 25.1.7",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "Mesa",
                "zink Vulkan 1.4(AMD Radeon RX 6800 (RADV NAVI21))",
                "4.6 (Core Profile) Mesa 25.1.7-devel (git-0123456)",
                false,
                Some("25.1.7"),
                None,
            ),
            (
                "Mesa",
                "Apple M2 (G14G B0)",
                "4.6 (Core Profile) Mesa 25.1.7 (Asahi)",
                false,
                Some("25.1.7"),
                None,
            ),
        ];
        for (vendor, renderer, version, gles, mesa, nvidia) in fixtures {
            let info = gl_info(
                Some(vendor),
                Some(renderer),
                Some(version),
                gles,
                &privacy(),
            );
            assert_eq!(info.vendor.as_deref(), Some(vendor));
            assert_eq!(info.renderer.as_deref(), Some(renderer));
            assert_eq!(info.mesa_version.as_deref(), mesa);
            assert_eq!(info.nvidia_version.as_deref(), nvidia);
            assert_eq!(info.api == GlApi::Gles, gles);
        }
    }

    #[test]
    fn gl_strings_redact_paths_identities_hardware_ids_and_labelled_values() {
        let privacy = privacy();
        let value = "Mesa /home/alice/driver /Users/bob/x file:///tmp/private renderer-on-private-host gpu@alice 01:23:45:67:89:ab 01234567-89ab-cdef-0123-456789abcdef 0123456789abcdef";
        let sanitized = privacy.sanitize(value).expect("redacted value");
        assert_eq!(sanitized, format!("Mesa {}", ["[redacted]"; 8].join(" ")));
        for value in [
            "Mesa serial ABC123",
            "Mesa hostname private-host",
            "username alice",
            "Mesa serial=AB1234",
        ] {
            assert_eq!(privacy.sanitize(value).as_deref(), Some("[redacted]"));
        }
        for value in [
            "renderer-on-pri\u{202e}vate-host",
            "user=al\u{202e}ice",
            "mac=01:23:45:67:89:ab",
            "gpu(01234567-89ab-cdef-0123-456789abcdef)",
        ] {
            assert_eq!(privacy.sanitize(value).as_deref(), Some("[redacted]"));
        }
        let info = gl_info(
            Some(value),
            Some("renderer-on-private-host"),
            Some("Mesa serial ABC123"),
            false,
            &privacy,
        );
        let serialized = serde_json::to_string(&info).expect("structured GL data");
        for private in [
            "alice",
            "bob",
            "private-host",
            "ABC123",
            "/home/",
            "/Users/",
            "01:23:45",
            "01234567-89ab",
        ] {
            assert!(!serialized.contains(private), "leaked {private}");
        }
    }

    #[test]
    fn strings_and_numeric_versions_are_bounded_and_malformed_safe() {
        for value in ["", "  ", "\u{202e}", "\0"] {
            assert!(privacy().sanitize(value).is_none());
        }
        let long = privacy()
            .sanitize(&"x ".repeat(1000))
            .expect("bounded text");
        assert!(long.len() <= MAX_STRING_BYTES);
        assert!(!long.chars().any(char::is_control));
        assert_eq!(numeric_version("580.95.05"), Some("580.95.05".to_owned()));
        assert_eq!(
            version_after("4.6 Mesa 26.2.6-devel (git-123)", "Mesa "),
            Some("26.2.6".to_owned())
        );
        for invalid in [
            "",
            "25",
            "25..1",
            "25.1.",
            "4294967296.1",
            "1.2.3.4.5",
            "/home/alice",
        ] {
            assert_eq!(numeric_version(invalid), None, "{invalid}");
        }
        assert_eq!(pci_id("0x1AF4\n"), Some("0x1af4".to_owned()));
        for invalid in ["1af4", "0x12345", "0xzzzz", "0x1234\nalice", "0x1"] {
            assert_eq!(pci_id(invalid), None);
        }
        unsafe {
            assert_eq!(bounded_c_string(std::ptr::null()), None);
            assert_eq!(
                bounded_c_string(c"Mesa".as_ptr().cast()),
                Some("Mesa".to_owned())
            );
            assert_eq!(bounded_c_string([0xff, 0].as_ptr()), None);
            assert_eq!(bounded_c_string([b'x'; MAX_FILE_BYTES].as_ptr()), None);
        }
    }

    fn fixture_device(
        root: &Path,
        name: &str,
        driver: &str,
        vendor: &str,
        device: &str,
    ) -> PathBuf {
        let path = root.join(name);
        fs::create_dir(&path).expect("device fixture");
        fs::write(path.join("vendor"), vendor).expect("vendor fixture");
        fs::write(path.join("device"), device).expect("device ID fixture");
        symlink(
            format!("/sys/bus/pci/drivers/{driver}"),
            path.join("driver"),
        )
        .expect("driver symlink fixture");
        path
    }

    fn fixture_node(drm: &Path, name: &str, device: &Path) {
        let node = drm.join(name);
        fs::create_dir(&node).expect("node fixture");
        symlink(device, node.join("device")).expect("device symlink fixture");
    }

    #[test]
    fn sysfs_groups_hybrid_gpus_and_discovers_virtio_kernel_driver() {
        let root = tempfile::tempdir().expect("sysfs fixture");
        let drm = root.path().join("drm");
        let modules = root.path().join("modules");
        fs::create_dir(&drm).expect("DRM fixture");
        fs::create_dir_all(modules.join("nvidia")).expect("module fixture");
        fs::write(modules.join("nvidia/version"), "580.95.05\n").expect("module version");
        let intel = fixture_device(root.path(), "gpu0", "i915", "0x8086\n", "0x9a49\n");
        let nvidia = fixture_device(root.path(), "gpu1", "nvidia", "0x10DE\n", "0x2684\n");
        let virtio = fixture_device(root.path(), "gpu2", "virtio-pci", "0x1af4\n", "0x1050\n");
        fs::create_dir(virtio.join("virtio1")).expect("virtio child");
        symlink(
            "/sys/bus/virtio/drivers/virtio_gpu",
            virtio.join("virtio1/driver"),
        )
        .expect("virtio driver");
        for (index, device) in [&intel, &nvidia, &virtio].into_iter().enumerate() {
            fixture_node(&drm, &format!("card{index}"), device);
            fixture_node(&drm, &format!("renderD{}", 128 + index), device);
        }
        fs::create_dir(drm.join("card0-DP-1")).expect("connector excluded");
        let result = kernel_graphics_at(&drm, &modules);
        assert_eq!(result.status, KernelStatus::Available);
        assert_eq!(result.devices.len(), 3);
        assert!(!result.truncated);
        let gpu = result
            .devices
            .iter()
            .find(|gpu| gpu.driver.as_deref() == Some("nvidia"))
            .expect("NVIDIA device");
        assert_eq!(gpu.vendor_id.as_deref(), Some("0x10de"));
        assert_eq!(gpu.driver_version.as_deref(), Some("580.95.05"));
        assert_eq!(gpu.card_nodes, ["card1"]);
        assert_eq!(gpu.render_nodes, ["renderD129"]);
        assert!(result
            .devices
            .iter()
            .any(|gpu| gpu.driver.as_deref() == Some("virtio_gpu")));
        let serialized = serde_json::to_string(&result).expect("kernel data");
        assert!(!serialized.contains(root.path().to_str().expect("fixture path")));
        assert!(!serialized.contains("/sys/"));
    }

    #[test]
    fn sysfs_distinguishes_no_gpu_unknown_partial_and_malformed_data() {
        let root = tempfile::tempdir().expect("sysfs fixture");
        let drm = root.path().join("drm");
        let modules = root.path().join("modules");
        assert_eq!(
            kernel_graphics_at(&drm, &modules).status,
            KernelStatus::Unavailable
        );
        fs::create_dir(&drm).expect("empty DRM fixture");
        assert_eq!(
            kernel_graphics_at(&drm, &modules).status,
            KernelStatus::NoGpu
        );
        fixture_node(&drm, "card0", &root.path().join("missing-device"));
        assert_eq!(
            kernel_graphics_at(&drm, &modules).status,
            KernelStatus::Partial
        );
        fs::remove_dir_all(drm.join("card0")).expect("remove broken node");
        let gpu = fixture_device(root.path(), "gpu", "amdgpu", "0x1234\nalice", "0xfffff");
        fixture_node(&drm, "renderD128", &gpu);
        fixture_node(&drm, "renderD129-private-host", &gpu);
        let result = kernel_graphics_at(&drm, &modules);
        assert_eq!(result.devices[0].vendor_id, None);
        assert_eq!(result.devices[0].device_id, None);
        assert_eq!(result.devices[0].render_nodes, ["renderD128"]);
        fs::write(gpu.join("vendor"), "x".repeat(MAX_FILE_BYTES + 1)).expect("oversize sysfs");
        assert_eq!(read_small(&gpu.join("vendor")), None);
    }

    #[test]
    fn sysfs_caps_devices_and_plugin_presence_does_not_claim_decoder_support() {
        let root = tempfile::tempdir().expect("sysfs fixture");
        let drm = root.path().join("drm");
        fs::create_dir(&drm).expect("DRM fixture");
        for index in 0..=MAX_DEVICES {
            let gpu = fixture_device(
                root.path(),
                &format!("gpu{index}"),
                "amdgpu",
                "0x1002",
                "0x1234",
            );
            fixture_node(&drm, &format!("renderD{}", 128 + index), &gpu);
        }
        let result = kernel_graphics_at(&drm, &root.path().join("modules"));
        assert_eq!(result.devices.len(), MAX_DEVICES);
        assert!(result.truncated);
        assert_eq!(result.status, KernelStatus::Partial);
        assert_eq!(
            plugin_presence(&[root.path()], "libgstva.so"),
            PluginPresence::NotFoundInSystemDirectories
        );
        fs::write(root.path().join("libgstva.so"), []).expect("plugin file fixture");
        assert_eq!(
            plugin_presence(&[root.path()], "libgstva.so"),
            PluginPresence::PresentInSystemDirectory
        );
        assert_eq!(
            plugin_presence(&[&root.path().join("missing")], "libgstva.so"),
            PluginPresence::Unknown
        );
    }
}
