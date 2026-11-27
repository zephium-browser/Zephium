//! Assigns Windows taskbar identity and icons to top-level windows.

use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::sync::{LazyLock, Mutex};

use tauri::WebviewWindow;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, PROPERTYKEY};
use windows::Win32::System::Com::CoTaskMemAlloc;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Variant::{VT_EMPTY, VT_LPWSTR};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, LoadImageW, SendMessageW, SetWindowPos, GWL_EXSTYLE, HICON, ICON_BIG, ICON_SMALL,
    ICON_SMALL2, IMAGE_ICON, LR_LOADFROMFILE, LR_SHARED, SM_CXICON, SM_CXSMICON, SWP_FRAMECHANGED,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_SETICON, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// Non-shared icons loaded from mini-app files remain live until their window closes.
static OWNED_ICONS: LazyLock<Mutex<HashMap<String, Vec<isize>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Assigns an optional window identity and icon, then opts it into the taskbar.
pub fn set_taskbar_icon(
    window: &WebviewWindow,
    app_user_model_id: Option<&str>,
    icon_path: Option<&std::path::Path>,
) {
    let Ok(native_window) = window.hwnd() else {
        return;
    };
    let hwnd = HWND(native_window.0);
    if let Some(app_user_model_id) = app_user_model_id {
        if let Err(error) = set_window_app_user_model_id(hwnd, app_user_model_id) {
            crate::write_diagnostic(format_args!(
                "mini_apps: taskbar identity could not be assigned: {error}"
            ));
        }
        let label = window.label().to_owned();
        let hwnd_value = hwnd.0 as isize;
        window.on_window_event(move |event| match event {
            tauri::WindowEvent::CloseRequested { .. } => {
                let hwnd = HWND(hwnd_value as *mut _);
                if let Err(error) = clear_window_app_user_model_id(hwnd) {
                    crate::write_diagnostic(format_args!(
                        "mini_apps: taskbar identity could not be cleared: {error}"
                    ));
                }
            }
            tauri::WindowEvent::Destroyed => destroy_owned_for_label(&label),
            _ => {}
        });
    }

    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let small_size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(1);
    let large_size = unsafe { GetSystemMetricsForDpi(SM_CXICON, dpi) }.max(1);
    let icons = [
        (ICON_BIG, large_size),
        (ICON_SMALL, small_size),
        (ICON_SMALL2, small_size),
    ]
    .into_iter()
    .filter_map(|(kind, size)| {
        let icon = icon_path.and_then(|path| load_file_icon(path, size));
        let icon = match icon {
            Some(icon) => Some((icon, true)),
            None => load_application_icon(size).map(|icon| (icon, false)),
        }?;
        Some((kind, icon.0, icon.1))
    })
    .collect::<Vec<_>>();

    // SAFETY: `hwnd` is this live Tauri top-level window. Preserve its other
    // extended styles while opting into taskbar presence and removing the
    // tool-window style, which explicitly suppresses taskbar buttons.
    unsafe {
        let style = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style | WS_EX_APPWINDOW.0 as isize) & !(WS_EX_TOOLWINDOW.0 as isize);
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
        if !icons.is_empty() {
            for (icon_type, icon, _) in &icons {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(windows::Win32::Foundation::WPARAM(*icon_type as usize)),
                    Some(windows::Win32::Foundation::LPARAM(icon.0 as isize)),
                );
            }
        }
    }

    retain_until_window_closes(
        window,
        icons
            .into_iter()
            .filter_map(|(_, icon, owned)| owned.then_some(icon))
            .collect(),
    );
}

/// Gives each installed mini-app a stable identity distinct from Zephium.
pub fn windows_app_user_model_id(id: &str) -> String {
    format!("app.zephium.miniapp.{id}")
}

/// Sets the Shell's per-window application identity before the window is shown.
fn set_window_app_user_model_id(hwnd: HWND, app_user_model_id: &str) -> windows::core::Result<()> {
    // SAFETY: `hwnd` is the live top-level window returned by Tauri.
    let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(hwnd)? };
    set_property_store_app_user_model_id(&store, app_user_model_id)?;
    // Windows applies this property immediately. Commit is harmless for a
    // window store and required for some shortcut property stores.
    unsafe { store.Commit() }
}

/// Clears a mini-app window's Shell identity before its HWND is destroyed.
fn clear_window_app_user_model_id(hwnd: HWND) -> windows::core::Result<()> {
    const APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };
    // VT_EMPTY removes the per-window override so Windows can release its
    // taskbar grouping association before the native handle goes away.
    let value = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_EMPTY,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0::default(),
            }),
        },
    };
    // SAFETY: `hwnd` is the live top-level window for its CloseRequested event.
    let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(hwnd)? };
    // SAFETY: the property key and empty PROPVARIANT match the Shell contract.
    unsafe { store.SetValue(&APP_USER_MODEL_ID, &value)? };
    // SAFETY: this commits only the property update above.
    unsafe { store.Commit() }
}

/// Applies the same AppUserModelID to taskbar windows and their `.lnk` files.
pub fn set_property_store_app_user_model_id(
    store: &IPropertyStore,
    app_user_model_id: &str,
) -> windows::core::Result<()> {
    const APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };
    let value = string_propvariant(app_user_model_id)?;
    // SAFETY: this is the SDK-defined System.AppUserModel.ID key, and `value`
    // remains alive for the synchronous COM call.
    unsafe { store.SetValue(&APP_USER_MODEL_ID, &value) }
}

fn string_propvariant(value: &str) -> windows::core::Result<PROPVARIANT> {
    let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    // PROPVARIANT's VT_LPWSTR requires CoTaskMem storage; PropVariantClear
    // takes ownership of this buffer when the returned value is dropped.
    let buffer = unsafe { CoTaskMemAlloc(bytes) }.cast::<u16>();
    if buffer.is_null() {
        return Err(windows::core::Error::new(
            windows::Win32::Foundation::E_OUTOFMEMORY,
            "could not allocate taskbar identity string",
        ));
    }
    // SAFETY: `buffer` has `bytes` writable bytes from CoTaskMemAlloc and
    // `wide` contains exactly that many initialized UTF-16 code units.
    unsafe { buffer.copy_from_nonoverlapping(wide.as_ptr(), wide.len()) };
    Ok(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: windows::core::PWSTR(buffer),
                },
            }),
        },
    })
}

fn load_file_icon(path: &std::path::Path, size: i32) -> Option<HICON> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `path` is NUL-terminated and remains alive for this synchronous
    // call. Without LR_SHARED, the returned HICON is owned by this process.
    unsafe {
        LoadImageW(
            None,
            PCWSTR(path.as_ptr()),
            IMAGE_ICON,
            size,
            size,
            LR_LOADFROMFILE,
        )
    }
    .ok()
    .map(|handle| HICON(handle.0))
}

fn load_application_icon(size: i32) -> Option<HICON> {
    // `tauri-build` embeds the configured icon as resource 32512 (IDI_ICON).
    // SAFETY: the executable module remains loaded for the process lifetime.
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        LoadImageW(
            Some(HINSTANCE(module.0)),
            PCWSTR(32512usize as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_SHARED,
        )
        .ok()
        .map(|handle| HICON(handle.0))
    }
}

fn retain_until_window_closes(window: &WebviewWindow, icons: Vec<HICON>) {
    let label = window.label().to_owned();
    let previous = OWNED_ICONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            label.clone(),
            icons.into_iter().map(|icon| icon.0 as isize).collect(),
        );
    if let Some(previous) = previous {
        for icon in previous {
            destroy_icon(icon);
        }
    }
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            destroy_owned_for_label(&label);
        }
    });
}

fn destroy_owned_for_label(label: &str) {
    let icon = OWNED_ICONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(label);
    if let Some(icons) = icon {
        for icon in icons {
            destroy_icon(icon);
        }
    }
}

fn destroy_icon(icon: isize) {
    // SAFETY: the handle came from LoadImageW without LR_SHARED and is removed
    // from the map before destruction, so it is released exactly once.
    let _ = unsafe { DestroyIcon(HICON(icon as *mut _)) };
}

#[cfg(test)]
mod tests {
    use super::windows_app_user_model_id;

    #[test]
    fn mini_app_taskbar_identity_is_stable_and_distinct() {
        let first = windows_app_user_model_id("0123456789abcdef0123456789abcdef");
        let same_app = windows_app_user_model_id("0123456789abcdef0123456789abcdef");
        let other_app = windows_app_user_model_id("fedcba9876543210fedcba9876543210");

        assert_eq!(first, same_app);
        assert_ne!(first, other_app);
        assert!(first.starts_with("app.zephium.miniapp."));
    }
}
