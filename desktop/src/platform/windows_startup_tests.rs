//! Native first-erase regression, independent of web content and app profiles.
use tauri::utils::config::Color;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, FillRect, GetDC, GetPixel,
    GetStockObject, ReleaseDC, SelectObject, HBITMAP, HBRUSH, HDC, HGDIOBJ, WHITE_BRUSH,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, SendMessageW, WM_ERASEBKGND};

struct TestSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl TestSurface {
    fn new(width: i32, height: i32) -> Self {
        // SAFETY: the fixture owns both new GDI objects, restores the selected
        // bitmap before deletion, and immediately releases the borrowed DC.
        unsafe {
            let screen = GetDC(None);
            assert!(!screen.0.is_null());
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            ReleaseDC(None, screen);
            assert!(!dc.0.is_null() && !bitmap.0.is_null());
            let previous = SelectObject(dc, bitmap.into());
            Self {
                dc,
                bitmap,
                previous,
            }
        }
    }

    fn erase_with(&self, hwnd: HWND) -> [u32; 3] {
        let mut rect = RECT::default();
        // SAFETY: the HWND belongs to this test's hidden Tauri window on the
        // current UI thread; the memory DC and rectangle are fixture-owned.
        unsafe {
            GetClientRect(hwnd, &mut rect).unwrap();
            assert!(rect.right > 8 && rect.bottom > 8);
            assert_ne!(
                FillRect(self.dc, &rect, HBRUSH(GetStockObject(WHITE_BRUSH).0)),
                0
            );
            SendMessageW(
                hwnd,
                WM_ERASEBKGND,
                Some(WPARAM(self.dc.0 as usize)),
                Some(LPARAM(0)),
            );
            [
                GetPixel(self.dc, 2, 2).0,
                GetPixel(self.dc, rect.right / 2, rect.bottom / 2).0,
                GetPixel(self.dc, rect.right - 3, rect.bottom - 3).0,
            ]
        }
    }
}

impl Drop for TestSurface {
    fn drop(&mut self) {
        // SAFETY: release only this fixture's objects after restoring selection.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

#[test]
#[ignore = "creates owned hidden native windows; run explicitly on Windows"]
fn transparent_startup_parent_erases_first_paint_without_waiting_for_redraw() {
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = "app.zephium.native-startup-test".into();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    // No WebView, plugin, user profile, focus change, timer or event-loop wait.
    // Both windows use the same native builder/erase implementation as chrome.
    let legacy = tauri::WindowBuilder::new(app.handle(), "legacy")
        .visible(false)
        .transparent(true)
        .inner_size(240.0, 160.0)
        .build()
        .unwrap();
    let fixed = tauri::WindowBuilder::new(app.handle(), "fixed")
        .visible(false)
        .transparent(true)
        .inner_size(240.0, 160.0)
        .background_color(Color(0, 0, 0, 0))
        .build()
        .unwrap();
    let surface = TestSurface::new(1024, 1024);
    let legacy_pixels = surface.erase_with(HWND(legacy.hwnd().unwrap().0));
    let fixed_pixels = surface.erase_with(HWND(fixed.hwnd().unwrap().0));
    legacy.destroy().unwrap();
    fixed.destroy().unwrap();
    app.cleanup_before_exit();
    // White remains untouched with the old configuration; explicit zero RGB
    // erases it immediately. Visible Acrylic composition is a separate QA check.
    assert_eq!(legacy_pixels, [0x00ff_ffff; 3]);
    assert_eq!(fixed_pixels, [0; 3]);
}

#[test]
fn a_runtime_without_a_required_interface_is_too_old_not_broken() {
    use crate::startup_alert::{StartupFailure, StartupProblem};
    use windows::Win32::Foundation::{E_ACCESSDENIED, E_NOINTERFACE};

    let missing: StartupFailure = windows::core::Error::new(
        E_NOINTERFACE,
        "this WebView2 runtime does not provide Settings3",
    )
    .into();
    assert_eq!(missing.problem(), StartupProblem::WebViewOutdated);
    assert!(missing.detail().contains("Settings3"));
    let denied: StartupFailure = windows::core::Error::new(E_ACCESSDENIED, "denied").into();
    assert_eq!(denied.problem(), StartupProblem::Other);
    let on_runtime = missing.on_runtime("120.0.2210.91");
    assert!(on_runtime.detail().contains("120.0.2210.91"));
}
