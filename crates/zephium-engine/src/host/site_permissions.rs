//! The decisions a person saved through WebView2's own permission prompt,
//! read and changed through the human profile that holds them.

use zephium_core::ids::ProfileId;
#[cfg(target_os = "windows")]
use zephium_core::permissions::SitePermission;
use zephium_core::permissions::{
    PageOrigin, RememberedPagePermission, SitePermissionKind, SitePermissionSetDone,
    SitePermissionsDone,
};

#[cfg(target_os = "windows")]
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ, COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
    COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES,
    COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS,
    COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS,
    COREWEBVIEW2_PERMISSION_STATE, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    COREWEBVIEW2_PERMISSION_STATE_DEFAULT, COREWEBVIEW2_PERMISSION_STATE_DENY,
};

use super::EngineHost;

#[cfg(target_os = "windows")]
const KINDS: [(SitePermissionKind, COREWEBVIEW2_PERMISSION_KIND); 9] = [
    (
        SitePermissionKind::Camera,
        COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    ),
    (
        SitePermissionKind::Microphone,
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    ),
    (
        SitePermissionKind::Geolocation,
        COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
    ),
    (
        SitePermissionKind::Notifications,
        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
    ),
    (
        SitePermissionKind::ClipboardRead,
        COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
    ),
    (
        SitePermissionKind::Midi,
        COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES,
    ),
    (
        SitePermissionKind::Sensors,
        COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS,
    ),
    (
        SitePermissionKind::LocalFonts,
        COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS,
    ),
    (
        SitePermissionKind::AutomaticDownloads,
        COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS,
    ),
];

#[cfg(target_os = "windows")]
fn native_kind(kind: SitePermissionKind) -> Option<COREWEBVIEW2_PERMISSION_KIND> {
    KINDS
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map(|(_, native)| *native)
}

#[cfg(target_os = "windows")]
fn site_kind(native: COREWEBVIEW2_PERMISSION_KIND) -> Option<SitePermissionKind> {
    KINDS
        .iter()
        .find(|(_, candidate)| *candidate == native)
        .map(|(kind, _)| *kind)
}

#[cfg(target_os = "windows")]
fn native_state(decision: Option<RememberedPagePermission>) -> COREWEBVIEW2_PERMISSION_STATE {
    match decision {
        Some(RememberedPagePermission::Allow) => COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        Some(RememberedPagePermission::Deny) => COREWEBVIEW2_PERMISSION_STATE_DENY,
        None => COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
    }
}

#[cfg(target_os = "windows")]
fn decision(state: COREWEBVIEW2_PERMISSION_STATE) -> Option<RememberedPagePermission> {
    match state {
        COREWEBVIEW2_PERMISSION_STATE_ALLOW => Some(RememberedPagePermission::Allow),
        COREWEBVIEW2_PERMISSION_STATE_DENY => Some(RememberedPagePermission::Deny),
        _ => None,
    }
}

/// WebView2 reports origins with a trailing slash; only a canonical tuple
/// origin Zephium itself could have prompted for is listed.
#[cfg(target_os = "windows")]
fn listed(
    origin: &str,
    kind: COREWEBVIEW2_PERMISSION_KIND,
    state: COREWEBVIEW2_PERMISSION_STATE,
) -> Option<SitePermission> {
    let url = url::Url::parse(origin).ok()?;
    Some(SitePermission {
        scheme: None,
        origin: PageOrigin::from_url(&url).ok()?,
        kind: site_kind(kind)?,
        decision: decision(state)?,
    })
}

impl EngineHost {
    /// The WebView2 profile holding `profile`'s saved decisions, admitted as
    /// a data call is, and whether a hidden page was opened to reach it.
    #[cfg(target_os = "windows")]
    fn human_profile(
        &mut self,
        profile: ProfileId,
    ) -> Option<(
        webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Profile4,
        bool,
    )> {
        use windows::core::Interface;
        use wry::WebViewExtWindows;
        let partition = zephium_core::ports::engine::Partition::Persistent(profile);
        super::browsing_data::admitted_profile(
            partition,
            &mut self.profile_persistence_classes,
            &self.erasure_tombstones,
        )
        .ok()?;
        self.reach_durable_profile(partition, |view| {
            crate::platform::imp::extensions::profile(&view.webview())
                .ok()?
                .cast()
                .ok()
        })
    }

    /// Closes a page opened only for one call once that call has answered.
    #[cfg(target_os = "windows")]
    fn after_answer<T: 'static>(
        profile: ProfileId,
        temporary: bool,
        done: Box<dyn FnOnce(T) + Send>,
    ) -> Box<dyn FnOnce(T) + Send> {
        if !temporary {
            return done;
        }
        Box::new(move |answer| {
            done(answer);
            super::lifecycle::release_temporary_spare(profile);
        })
    }

    pub(crate) fn site_permissions(&mut self, profile: ProfileId, done: SitePermissionsDone) {
        #[cfg(target_os = "windows")]
        {
            use std::cell::Cell;
            use std::rc::Rc;
            let Some((native, temporary)) = self.human_profile(profile) else {
                return done(None);
            };
            let done = Self::after_answer(profile, temporary, done);
            let slot = Rc::new(Cell::new(Some(done)));
            let callback = slot.clone();
            let handler = webview2_com::GetNonDefaultPermissionSettingsCompletedHandler::create(
                Box::new(move |status, settings| {
                    let Some(done) = callback.take() else {
                        return Ok(());
                    };
                    let read = || -> windows::core::Result<Vec<SitePermission>> {
                        status?;
                        let Some(settings) = settings else {
                            return Ok(Vec::new());
                        };
                        let mut count = 0;
                        // SAFETY: the collection is live for this callback.
                        unsafe { settings.Count(&mut count)? };
                        let mut entries = Vec::new();
                        // One past the cap, so the listing knows it is cut short.
                        let limit =
                            count.min(zephium_core::permissions::MAX_SITE_PERMISSIONS as u32 + 1);
                        for index in 0..limit {
                            // SAFETY: index is below the collection's Count.
                            let setting = unsafe { settings.GetValueAtIndex(index)? };
                            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                            let mut state = COREWEBVIEW2_PERMISSION_STATE::default();
                            let mut origin = windows::core::PWSTR::null();
                            // SAFETY: outputs are initialized and the string
                            // is freed by take_pwstr.
                            unsafe {
                                setting.PermissionKind(&mut kind)?;
                                setting.PermissionState(&mut state)?;
                                setting.PermissionOrigin(&mut origin)?;
                            }
                            let origin = webview2_com::take_pwstr(origin);
                            entries.extend(listed(&origin, kind, state));
                        }
                        Ok(entries)
                    };
                    done(read().ok());
                    Ok(())
                }),
            );
            // SAFETY: a live human profile on its owning apartment.
            if unsafe { native.GetNonDefaultPermissionSettings(&handler) }.is_err() {
                if let Some(done) = slot.take() {
                    done(None);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = profile;
            done(None);
        }
    }

    pub(crate) fn set_site_permission(
        &mut self,
        profile: ProfileId,
        origin: PageOrigin,
        kind: SitePermissionKind,
        decision: Option<RememberedPagePermission>,
        done: SitePermissionSetDone,
    ) {
        #[cfg(target_os = "windows")]
        {
            use std::cell::Cell;
            use std::rc::Rc;
            let Some(kind) = native_kind(kind) else {
                return done(false);
            };
            let Some((native, temporary)) = self.human_profile(profile) else {
                return done(false);
            };
            let done = Self::after_answer(profile, temporary, done);
            let slot = Rc::new(Cell::new(Some(done)));
            let callback = slot.clone();
            let handler =
                webview2_com::SetPermissionStateCompletedHandler::create(Box::new(move |status| {
                    if let Some(done) = callback.take() {
                        done(status.is_ok());
                    }
                    Ok(())
                }));
            let origin = windows::core::HSTRING::from(origin.as_str());
            // SAFETY: a live human profile on its owning apartment; the
            // origin string outlives the call.
            if unsafe { native.SetPermissionState(kind, &origin, native_state(decision), &handler) }
                .is_err()
            {
                if let Some(done) = slot.take() {
                    done(false);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (profile, origin, kind, decision);
            done(false);
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn every_listed_kind_maps_both_ways_and_others_do_not() {
        for (kind, native) in KINDS {
            assert_eq!(native_kind(kind), Some(native));
            assert_eq!(site_kind(native), Some(kind));
        }
        assert_eq!(native_kind(SitePermissionKind::Popups), None);
        assert_eq!(
            site_kind(webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE),
            None
        );
    }

    #[test]
    fn states_map_to_decisions_and_default_forgets() {
        for value in [
            Some(RememberedPagePermission::Allow),
            Some(RememberedPagePermission::Deny),
        ] {
            assert_eq!(decision(native_state(value)), value);
        }
        assert_eq!(native_state(None), COREWEBVIEW2_PERMISSION_STATE_DEFAULT);
        assert_eq!(decision(COREWEBVIEW2_PERMISSION_STATE_DEFAULT), None);
    }

    #[test]
    fn listed_origins_are_canonical_and_default_states_are_left_out() {
        let entry = listed(
            "https://Meet.Example/",
            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        )
        .unwrap();
        assert_eq!(entry.origin.as_str(), "https://meet.example");
        assert_eq!(entry.kind, SitePermissionKind::Camera);
        assert_eq!(entry.decision, RememberedPagePermission::Allow);
        assert!(listed(
            "https://meet.example/",
            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT
        )
        .is_none());
        assert!(listed(
            "file:///C:/",
            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_STATE_DENY
        )
        .is_none());
    }
}
