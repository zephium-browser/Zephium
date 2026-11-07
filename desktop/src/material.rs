//! Native material is an installed, per-window capability, never a UA guess.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    #[default]
    None,
    Vibrancy,
    LiquidGlass,
    Acrylic,
    Mica,
}

/// The person's choice of material. The system still decides what it can
/// install: a choice it cannot honour falls back down the same ladder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    /// The system's best glass.
    #[default]
    Auto,
    LiquidGlass,
    Vibrancy,
    Acrylic,
    Mica,
    None,
}

impl Preference {
    pub fn parse(value: &str) -> Self {
        match value {
            "liquid_glass" => Self::LiquidGlass,
            "vibrancy" => Self::Vibrancy,
            "acrylic" => Self::Acrylic,
            "mica" => Self::Mica,
            "none" => Self::None,
            _ => Self::Auto,
        }
    }
}

static CHOSEN: std::sync::Mutex<Option<Preference>> = std::sync::Mutex::new(None);

/// A change arrives as a projection, possibly ahead of the store's write.
pub fn choose(value: &str) {
    *CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Preference::parse(value));
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn preference() -> Preference {
    if let Some(chosen) = *CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) {
        return chosen;
    }
    crate::app_setting("ui.material")
        .map(|value| Preference::parse(&value))
        .unwrap_or_default()
}

/// The stored theme as native paints it. Each projected change replaces the
/// stored value it shadows, as `CHOSEN` does for the material.
struct Theme {
    id: Option<String>,
    custom: Option<String>,
    level: Option<u8>,
    contrast: Option<bool>,
    tint: Option<u8>,
}

static THEME: std::sync::Mutex<Theme> = std::sync::Mutex::new(Theme {
    id: None,
    custom: None,
    level: None,
    contrast: None,
    tint: None,
});

/// Takes a projected preference. True when it changes what native paints.
pub fn theme_changed(key: &str, value: &str) -> bool {
    let mut theme = THEME.lock().unwrap_or_else(|e| e.into_inner());
    match key {
        "ui.theme" => theme.id = Some(value.to_owned()),
        "ui.theme-custom" => theme.custom = Some(value.to_owned()),
        "ui.theme-contrast" => theme.level = value.parse().ok(),
        "ui.contrast" => theme.contrast = Some(value == "true"),
        "ui.window-tint" => theme.tint = Some(value.parse().unwrap_or(0)),
        _ => return false,
    }
    true
}

/// The theme's chrome rung, which is the window's ground when it is solid,
/// or nothing while the theme is the graphite default.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn themed_ground(light: bool) -> Option<[u8; 3]> {
    let theme = THEME.lock().unwrap_or_else(|e| e.into_inner());
    // A themed space paints its own ground while it is active.
    let space = crate::spaces::theme();
    let id = space
        .as_ref()
        .map(|(id, _)| id.clone())
        .or_else(|| theme.id.clone())
        .or_else(|| crate::app_setting("ui.theme"))
        .unwrap_or_else(|| zephium_core::theme::DEFAULT.into());
    let custom = match space {
        Some((_, custom)) => custom,
        None => theme
            .custom
            .clone()
            .or_else(|| crate::app_setting("ui.theme-custom")),
    };
    let level = theme
        .level
        .or_else(|| crate::app_setting("ui.theme-contrast").and_then(|value| value.parse().ok()))
        .unwrap_or(zephium_core::theme::DEFAULT_CONTRAST);
    let contrast = theme
        .contrast
        .unwrap_or_else(|| crate::app_setting("ui.contrast").as_deref() == Some("true"));
    (!zephium_core::theme::is_default(&id, custom.as_deref(), level, contrast))
        .then(|| zephium_core::theme::chrome(&id, custom.as_deref(), level, contrast, light))
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn ground(light: bool) -> [u8; 3] {
    themed_ground(light).unwrap_or(if light { [230, 230, 233] } else { [31, 31, 34] })
}

/// The ground laid over the material, as RGBA, or nothing at the untinted
/// default. Full tint stops short of opaque so the material still reads.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn tint(light: bool) -> Option<(u8, u8, u8, u8)> {
    let amount = {
        let theme = THEME.lock().unwrap_or_else(|e| e.into_inner());
        theme.tint.unwrap_or_else(|| {
            crate::app_setting("ui.window-tint")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        })
    };
    (amount > 0).then(|| {
        let [r, g, b] = ground(light);
        (r, g, b, tint_alpha(amount))
    })
}

fn tint_alpha(amount: u8) -> u8 {
    (f64::from(amount.min(100)) * 0.85 * 2.55).round() as u8
}

#[cfg(any(target_os = "macos", test))]
fn select_material(
    reduce_transparency: bool,
    preference: Preference,
    glass: impl FnOnce() -> bool,
    vibrancy: impl FnOnce() -> bool,
) -> Material {
    if reduce_transparency || preference == Preference::None {
        Material::None
    } else if preference != Preference::Vibrancy && glass() {
        Material::LiquidGlass
    } else if vibrancy() {
        Material::Vibrancy
    } else {
        Material::None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{select_material, Material};
    use objc2::{rc::Retained, runtime::ProtocolObject, MainThreadMarker};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBox, NSBoxType, NSColor, NSView, NSWindow, NSWorkspace,
        NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
    };
    use objc2_foundation::{NSNotificationCenter, NSObjectProtocol};
    use std::{
        cell::RefCell,
        collections::HashMap,
        sync::{LazyLock, Mutex},
    };
    use tauri::{Manager, WebviewWindow};
    use window_vibrancy::{
        apply_liquid_glass, apply_vibrancy, clear_liquid_glass, clear_vibrancy, LiquidGlassOptions,
        NSGlassEffectViewStyle, NSVisualEffectMaterial,
    };

    static INSTALLED: LazyLock<Mutex<HashMap<String, Material>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    struct Observer {
        center: Retained<NSNotificationCenter>,
        token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }
    impl Drop for Observer {
        fn drop(&mut self) {
            unsafe {
                self.center.removeObserver((*self.token).as_ref());
            }
        }
    }
    thread_local! { static OBSERVERS: RefCell<HashMap<String, Observer>> = RefCell::new(HashMap::new()); }
    /// Solid paints the window itself. The chrome covers only the sidebar, so
    /// with no material the gutters around the page showed the desktop. The
    /// colour is the theme's chrome rung, so the sidebar and its surround are
    /// one sheet, and it is read from the store before the window is shown.
    fn set_ground(window: &WebviewWindow, material: Material) {
        let Ok(pointer) = window.ns_window() else {
            return;
        };
        // SAFETY: Tao's live NSWindow, borrowed on the main thread for this call.
        let ns_window = unsafe { &*(pointer as *const NSWindow) };
        let color = if material == Material::None {
            let [r, g, b] = super::ground(crate::appearance_is_light(window));
            srgb((r, g, b, 255))
        } else {
            NSColor::clearColor()
        };
        ns_window.setBackgroundColor(Some(&color));
    }

    fn srgb((r, g, b, a): (u8, u8, u8, u8)) -> Retained<NSColor> {
        let channel = |value: u8| f64::from(value) / 255.0;
        NSColor::colorWithSRGBRed_green_blue_alpha(channel(r), channel(g), channel(b), channel(a))
    }

    /// Vibrancy has no tint of its own: the ground is laid inside the effect
    /// view, above its material and below everything the window holds.
    fn tint_vibrancy(window: &WebviewWindow, tint: (u8, u8, u8, u8)) {
        /// `window_vibrancy`'s tag for the effect view it installs.
        const EFFECT_VIEW_TAG: isize = 91_376_254;
        let Ok(pointer) = window.ns_view() else {
            return;
        };
        // SAFETY: Tao's live content view, borrowed on the main thread.
        let content = unsafe { &*(pointer as *const NSView) };
        let Some(effect) = content.viewWithTag(EFFECT_VIEW_TAG) else {
            return;
        };
        let Some(main) = MainThreadMarker::new() else {
            return;
        };
        let wash = NSBox::initWithFrame(main.alloc(), effect.bounds());
        wash.setBoxType(NSBoxType::Custom);
        wash.setBorderWidth(0.0);
        wash.setCornerRadius(12.0);
        wash.setFillColor(&srgb(tint));
        wash.setTransparent(false);
        wash.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        effect.addSubview(&wash);
    }

    pub fn current(label: &str) -> Material {
        INSTALLED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(label)
            .copied()
            .unwrap_or_default()
    }

    pub fn install(window: &WebviewWindow, panel: bool) -> Material {
        // Setup and observer callbacks execute on AppKit's main thread.
        if MainThreadMarker::new().is_none() {
            return Material::None;
        }
        let workspace = NSWorkspace::sharedWorkspace();
        let preference = super::preference();
        let reduce_transparency = workspace.accessibilityDisplayShouldReduceTransparency();
        let material = if panel {
            // The launcher is two shapes rather than one window-sized sheet;
            // it installs its own material behind its content.
            crate::panel::install_shapes(
                window,
                reduce_transparency || preference == super::Preference::None,
                !crate::appearance_is_light(window),
            )
        } else {
            // Clear before reapplying: the upstream glass API inserts a new view.
            let _ = clear_liquid_glass(window);
            let _ = clear_vibrancy(window);
            // The system's materials as it draws them, untinted unless the
            // person asked for the theme's ground over them.
            let tint = super::tint(crate::appearance_is_light(window));
            select_material(
                reduce_transparency,
                preference,
                || {
                    let options = LiquidGlassOptions::new(NSGlassEffectViewStyle::Regular)
                        .radius(12.0)
                        .opaque(false);
                    let options = match tint {
                        Some(tint) => options.tint_color(tint),
                        None => options,
                    };
                    apply_liquid_glass(window, options).is_ok()
                },
                || {
                    let applied =
                        apply_vibrancy(window, NSVisualEffectMaterial::Sidebar, None, Some(12.0))
                            .is_ok();
                    if let (true, Some(tint)) = (applied, tint) {
                        tint_vibrancy(window, tint);
                    }
                    applied
                },
            )
        };
        if !panel {
            set_ground(window, material);
        }
        // No content_view(): preserve WKWebView siblings, parent bounds,
        // native z-order and the security-sensitive chrome rectangle.
        INSTALLED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(window.label().to_owned(), material);
        publish(window, material);
        OBSERVERS.with(|observers| {
            if observers.borrow().contains_key(window.label()) {
                return;
            }
            let app = window.app_handle().clone();
            let label = window.label().to_owned();
            let block = block2::RcBlock::new(
                move |_: std::ptr::NonNull<objc2_foundation::NSNotification>| {
                    let app_for_main = app.clone();
                    let label = label.clone();
                    let _ = app.run_on_main_thread(move || {
                        if let Some(window) = app_for_main.get_webview_window(&label) {
                            install(&window, panel);
                        }
                    });
                },
            );
            let center = workspace.notificationCenter();
            // SAFETY: block captures only Send handles/strings; callback work
            // is dispatched to the main thread. Token removal is main-thread.
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                    None,
                    None,
                    &block,
                )
            };
            observers
                .borrow_mut()
                .insert(window.label().to_owned(), Observer { center, token });
            let app = window.app_handle().clone();
            let label = window.label().to_owned();
            window.on_window_event(move |event| {
                // The material is installed for one appearance; reinstalling
                // keeps the launcher's shapes and the ground in step.
                if matches!(event, tauri::WindowEvent::ThemeChanged(_)) {
                    let app_for_main = app.clone();
                    let label = label.clone();
                    let _ = app.run_on_main_thread(move || {
                        if let Some(window) = app_for_main.get_webview_window(&label) {
                            install(&window, panel);
                        }
                    });
                    return;
                }
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    let label = label.clone();
                    let _ = app.run_on_main_thread(move || {
                        OBSERVERS.with(|observers| {
                            observers.borrow_mut().remove(&label);
                        });
                        INSTALLED
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&label);
                    });
                }
            });
        });
        material
    }

    pub fn publish(window: &WebviewWindow, material: Material) {
        // Serialization is a closed enum, never page or model input.
        if let Ok(value) = serde_json::to_string(&material) {
            let _ = window.eval(format!("window.dispatchEvent(new CustomEvent('zephium:ui-command',{{detail:'material.'+{value}}}))"));
        }
    }
}
#[cfg(target_os = "macos")]
pub use macos::{current, install};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_projected_theme_shadows_the_store_and_tints_only_when_asked() {
        assert!(theme_changed("ui.theme", "zephium"));
        assert!(theme_changed("ui.contrast", "false"));
        assert!(theme_changed("ui.window-tint", "0"));
        assert!(theme_changed("ui.theme-contrast", "50"));
        assert!(!theme_changed("ui.accent", "sky"));
        assert_eq!(ground(false), [31, 31, 34]);
        assert_eq!(tint(false), None);
        assert!(theme_changed("ui.theme", "nord"));
        assert!(theme_changed("ui.window-tint", "100"));
        let [r, g, b] = ground(false);
        assert_eq!(tint(false), Some((r, g, b, 217)));
        assert_eq!(tint_alpha(40), 87);
        assert!(theme_changed("ui.theme", "zephium"));
        assert!(theme_changed("ui.window-tint", "0"));
    }

    #[test]
    fn accessibility_avoids_effect_installation() {
        assert_eq!(
            select_material(
                true,
                Preference::Auto,
                || panic!("glass must not run"),
                || panic!("vibrancy must not run")
            ),
            Material::None
        );
    }
    #[test]
    fn material_fallback_reports_only_installed_effects() {
        assert_eq!(
            select_material(
                false,
                Preference::Auto,
                || true,
                || panic!("fallback must not run")
            ),
            Material::LiquidGlass
        );
        assert_eq!(
            select_material(false, Preference::Auto, || false, || true),
            Material::Vibrancy
        );
        assert_eq!(
            select_material(false, Preference::Auto, || false, || false),
            Material::None
        );
    }
    #[test]
    fn the_chosen_material_is_installed_or_falls_back() {
        assert_eq!(
            select_material(
                false,
                Preference::Vibrancy,
                || panic!("glass is not chosen"),
                || true
            ),
            Material::Vibrancy
        );
        assert_eq!(
            select_material(
                false,
                Preference::None,
                || panic!("no material is chosen"),
                || panic!("no material is chosen")
            ),
            Material::None
        );
        assert_eq!(
            select_material(false, Preference::LiquidGlass, || false, || true),
            Material::Vibrancy
        );
    }
}
