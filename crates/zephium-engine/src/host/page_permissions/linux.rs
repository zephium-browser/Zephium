use super::*;
use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::sync::atomic::AtomicU64;
use webkit2gtk::{PermissionRequestExt, UserMediaPermissionRequestExt, WebViewExt};
use wry::WebViewExtUnix;

type Source = (
    ProfileId,
    Rc<std::cell::Cell<ItemId>>,
    EventPermit,
    NavigationEpochTracker,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
);

#[derive(Default)]
pub(in crate::host) struct LinuxRequests {
    pending: RefCell<Option<(u64, webkit2gtk::PermissionRequest, bool)>>,
}
impl LinuxRequests {
    pub(in crate::host) fn admit(
        &self,
        page: &webkit2gtk::WebView,
        native: &webkit2gtk::PermissionRequest,
        source: Source,
    ) -> bool {
        let (profile, item, permit, navigation, presence, presentation) = source;
        if !presentation.load(Ordering::Acquire)
            || self.pending.borrow().is_some()
            || !page.is_mapped()
            || !page
                .toplevel()
                .and_then(|w| w.downcast::<gtk::Window>().ok())
                .is_some_and(|w| w.is_active())
        {
            return false;
        }
        let kind =
            if let Some(media) = native.downcast_ref::<webkit2gtk::UserMediaPermissionRequest>() {
                use glib::translate::ToGlibPtr;
                if unsafe {
                    webkit2gtk::ffi::webkit_user_media_permission_is_for_display_device(
                        media.to_glib_none().0,
                    )
                } != 0
                {
                    if permit.active_token().is_some()
                        && navigation.current_committed().is_some()
                        && portal_available()
                    {
                        native.allow();
                    } else {
                        native.deny();
                    }
                    return true;
                }
                match (media.is_for_video_device(), media.is_for_audio_device()) {
                    (true, true) => WryRequestKind::CameraAndMicrophone,
                    (true, false) => WryRequestKind::Single(wry::PermissionKind::Camera),
                    (false, true) => WryRequestKind::Single(wry::PermissionKind::Microphone),
                    _ => return false,
                }
            } else if native.is::<webkit2gtk::GeolocationPermissionRequest>() {
                WryRequestKind::Single(wry::PermissionKind::Geolocation)
            } else {
                return false;
            };
        let Some((_, url)) = navigation.committed_snapshot() else {
            return false;
        };
        let Ok(url) = url::Url::parse(&url) else {
            return false;
        };
        let Some(host) = url.host_str() else {
            return false;
        };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let Ok(id) =
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        else {
            return false;
        };
        let Some(mut request) =
            core_request_from_components(id, url.scheme(), host, url.port(), kind)
        else {
            return false;
        };
        let available = matches!(
            kind,
            WryRequestKind::Single(wry::PermissionKind::Geolocation)
        ) || capture_available(page);
        request.blocked_by_system = !available;
        *self.pending.borrow_mut() = Some((id, native.clone(), available));
        if admit_core_request(profile, item, permit, navigation, presence, request)
            != WryDisposition::Defer
        {
            self.resolve(id, WryResponse::Deny);
        }
        true
    }
    pub(in crate::host) fn resolve(&self, id: u64, response: WryResponse) -> bool {
        let mut pending = self.pending.borrow_mut();
        if pending
            .as_ref()
            .is_none_or(|(current, _, _)| *current != id)
        {
            return false;
        }
        let (_, native, available) = pending.take().unwrap();
        drop(pending);
        if available && response == WryResponse::Allow {
            native.allow();
        } else {
            native.deny();
        }
        true
    }
}
impl Drop for LinuxRequests {
    fn drop(&mut self) {
        if let Some((_, native, _)) = self.pending.get_mut().take() {
            native.deny();
        }
    }
}

pub(in crate::host) struct CaptureObserver {
    page: glib::WeakRef<webkit2gtk::WebView>,
    signals: Vec<glib::SignalHandlerId>,
}
impl CaptureObserver {
    pub(in crate::host) fn install(
        view: &wry::WebView,
        changed: impl Fn(zephium_core::ports::engine::MediaCaptureState) + 'static,
    ) -> Self {
        let page = view.webview();
        if let Some(settings) = webkit2gtk::WebViewExt::settings(&page) {
            webkit2gtk::SettingsExt::set_enable_media_stream(&settings, true);
        }
        let changed = Rc::new(changed);
        let camera = changed.clone();
        let camera = page.connect_camera_capture_state_notify(move |page| camera(sample(page)));
        let microphone =
            page.connect_microphone_capture_state_notify(move |page| changed(sample(page)));
        Self {
            page: page.downgrade(),
            signals: vec![camera, microphone],
        }
    }
}
impl Drop for CaptureObserver {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            for signal in self.signals.drain(..) {
                page.disconnect(signal);
            }
        }
    }
}
fn sample(page: &webkit2gtk::WebView) -> zephium_core::ports::engine::MediaCaptureState {
    use zephium_core::ports::engine::{CaptureDeviceState, MediaCaptureState};
    let device = |state| match state {
        webkit2gtk::MediaCaptureState::Active => CaptureDeviceState::Active,
        webkit2gtk::MediaCaptureState::Muted => CaptureDeviceState::Muted,
        _ => CaptureDeviceState::None,
    };
    MediaCaptureState {
        camera: device(page.camera_capture_state()),
        microphone: device(page.microphone_capture_state()),
    }
}
pub(super) fn stop(view: &wry::WebView) {
    let page = view.webview();
    page.set_camera_capture_state(webkit2gtk::MediaCaptureState::None);
    page.set_microphone_capture_state(webkit2gtk::MediaCaptureState::None);
}

fn portal_available() -> bool {
    use gtk::gio;
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&("org.freedesktop.portal.Desktop",).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        250,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|v| v.get::<(bool,)>())
    .is_some_and(|v| v.0)
}
fn capture_available(page: &webkit2gtk::WebView) -> bool {
    #[cfg(test)]
    {
        use webkit2gtk::SettingsExt;
        if webkit2gtk::WebViewExt::settings(page).is_some_and(|s| s.enables_mock_capture_devices())
        {
            return true;
        }
    }
    let _ = page;
    use std::os::unix::fs::FileTypeExt;
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .and_then(|path| std::fs::symlink_metadata(path.join("pipewire-0")).ok())
        .is_some_and(|m| m.file_type().is_socket())
        && portal_available()
}
