//! Native new-window adoption. Construct once with the original opener's native
//! request/configuration; shell owns the logical tab and never replays its URL.
use super::{construction::NativeViewPurpose, permits::EventPermit, EngineHost};
use crate::navigation_epoch::NavigationActivity;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use wry::{NewWindowFeatures, NewWindowResponse};
use zephium_core::geometry::Rect;
use zephium_core::ids::ItemId;
use zephium_core::ports::engine::EngineEvent;

impl EngineHost {
    #[cfg(target_os = "windows")]
    pub(super) fn finish_windows_native_tab(&mut self, id: ItemId, attached: bool) {
        let result = (|| {
            if !attached {
                return false;
            }
            let Some(profile) = self.partitions.get(&id).copied() else {
                return false;
            };
            let Some(policy) = self.applied_content_policy(profile.profile()) else {
                return false;
            };
            let Some(view) = self.views.get_mut(&id) else {
                return false;
            };
            let Ok(next) = crate::platform::imp::install_scoped_content_policy_on_view(
                &view.view,
                &policy,
                &view.site_scope.pause,
            ) else {
                return false;
            };
            let previous = view.content_policy_registration.replace(next);
            if previous.is_some_and(|previous| previous.retire().is_err()) {
                self.fail_content_policy_retirement();
                return false;
            }
            true
        })();
        if !result {
            let token = self
                .views
                .get(&id)
                .and_then(|view| view.event_permit.active_token());
            self.close(id);
            if let Some(token) = token {
                self.sink
                    .emit_for(token, EngineEvent::ViewCreationFailed { id });
            }
        }
    }

    pub(crate) fn finish_native_tab_adoption(
        &mut self,
        id: ItemId,
        token: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        if !self
            .views
            .get(&id)
            .is_some_and(|view| view.event_permit.matches_token(token))
        {
            return;
        }
        self.finish_new_view_insertion(id, token);
        let facts = self.views.get(&id).and_then(|view| {
            view.navigation.current_committed().map(|epoch| {
                (
                    view.event_permit.clone(),
                    view.navigation.clone(),
                    epoch,
                    view.title_ready == Some(epoch),
                )
            })
        });
        if let Some((permit, navigation, epoch, finished)) = facts {
            if self.emit_navigation_observation(id, &permit, &navigation, epoch) {
                if finished {
                    self.complete_title_attribution(id, &permit, &navigation, epoch);
                }
                self.emit_navigation_ready(id, &permit, &navigation, epoch);
            }
        }
    }

    pub(super) fn open_native_tab(
        &mut self,
        source: ItemId,
        permit: &EventPermit,
        activity: NavigationActivity,
        url: &str,
        features: NewWindowFeatures,
    ) -> NewWindowResponse {
        if !features.user_initiated || !permit.allows_target(url) {
            return NewWindowResponse::Deny;
        }
        let Some(view) = self.views.get(&source) else {
            return NewWindowResponse::Deny;
        };
        if !view.event_permit.same_generation(permit)
            || !view.navigation.matches_activity(activity)
            || !view.presentation_permit.load(Ordering::Acquire)
            || !view.download_surface_intent.load(Ordering::Acquire)
        {
            return NewWindowResponse::Deny;
        }
        if permit.active_token().is_none() {
            return NewWindowResponse::Deny;
        }
        let Some(partition) = self.partitions.get(&source).copied() else {
            return NewWindowResponse::Deny;
        };
        if self.erasure_tombstones.contains(&partition.profile()) {
            return NewWindowResponse::Deny;
        }
        #[cfg(target_os = "macos")]
        {
            use objc2::rc::Retained;
            use wry::WebViewExtMacOS;
            let observed = view.view.webview();
            if Retained::as_ptr(&observed).cast::<()>()
                != Retained::as_ptr(&features.opener.webview).cast::<()>()
                || !observed
                    .window()
                    .is_some_and(|window| window.isVisible() && window.isKeyWindow())
            {
                return NewWindowResponse::Deny;
            }
        }
        #[cfg(target_os = "windows")]
        {
            use windows::core::Interface;
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{
                GetAncestor, GetForegroundWindow, GA_ROOT,
            };
            use wry::WebViewExtWindows;
            if view.view.webview().as_raw() != features.opener.webview.as_raw() {
                return NewWindowResponse::Deny;
            }
            let mut parent = HWND::default();
            if unsafe { view.view.controller().ParentWindow(&mut parent) }.is_err()
                || unsafe { GetAncestor(parent, GA_ROOT) != GetForegroundWindow() }
            {
                return NewWindowResponse::Deny;
            }
        }
        self.adopt_native_tab(source, permit, activity, url, features, || true)
    }

    /// Both callers authenticate their native opener first. Preserve WebView2's
    /// original request (including OAuth state); never replay just its URL.
    pub(super) fn adopt_native_tab(
        &mut self,
        source: ItemId,
        permit: &EventPermit,
        activity: NavigationActivity,
        url: &str,
        features: NewWindowFeatures,
        opener_current: impl Fn() -> bool,
    ) -> NewWindowResponse {
        let Some(token) = permit.active_token() else {
            return NewWindowResponse::Deny;
        };
        let Some(partition) = self.partitions.get(&source).copied() else {
            return NewWindowResponse::Deny;
        };
        let child = ItemId::generate();
        let Some(active) =
            self.native_open_authority
                .reserve(source, &token, child, partition.profile())
        else {
            return NewWindowResponse::Deny;
        };
        let adoption = self.native_open_authority.adoption(child, active.clone());
        let foreground = features.foreground;
        let built = self.build_view(
            Rc::new(Cell::new(child)),
            partition,
            url,
            Rect::default(),
            EventPermit::bound(&active),
            NativeViewPurpose::NativeTab(features.opener),
        );
        let Some(view) = built else {
            self.native_open_authority.release_failed(child, &active);
            return NewWindowResponse::Deny;
        };
        if !opener_current()
            || !token.load(Ordering::Acquire)
            || !active.load(Ordering::Acquire)
            || !self.views.get(&source).is_some_and(|view| {
                view.event_permit.same_generation(permit)
                    && view.navigation.matches_activity(activity)
            })
        {
            drop(view);
            self.native_open_authority.release_failed(child, &active);
            return NewWindowResponse::Deny;
        }
        #[cfg(target_os = "macos")]
        let native = {
            use objc2::rc::Retained;
            use wry::WebViewExtMacOS;
            let value = view.view.webview();
            Retained::clone(&value).into_super()
        };
        #[cfg(target_os = "windows")]
        let native = {
            use wry::WebViewExtWindows;
            view.view.webview()
        };
        #[cfg(target_os = "windows")]
        let guarded_controller = {
            use wry::WebViewExtWindows;
            view.view.controller()
        };
        #[cfg(target_os = "windows")]
        let guarded_permit = view.event_permit.clone();
        #[cfg(target_os = "windows")]
        let guarded_failure = self.native_terminal_failure.clone();
        #[cfg(target_os = "macos")]
        crate::platform::imp::set_background_suspension(&view.view, false);
        self.partitions.insert(child, partition);
        self.views.insert(child, view);
        permit.emit(
            &self.sink,
            EngineEvent::NativeTabOpened {
                id: source,
                child,
                foreground,
                adoption,
            },
        );
        #[cfg(target_os = "macos")]
        {
            NewWindowResponse::Create { webview: native }
        }
        #[cfg(target_os = "windows")]
        {
            NewWindowResponse::CreateGuarded {
                webview: native,
                attached: Box::new(move |success| {
                    if !super::dispatch::finish_windows_native_tab(child, success) {
                        // Never complete a new-window deferral with an
                        // unattested controller when host ownership reenters.
                        guarded_permit.revoke();
                        unsafe {
                            let _ = guarded_controller.Close();
                        }
                        guarded_failure("native popup post-attachment ownership was unavailable");
                    }
                }),
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn filter_windows_context_menu(
    permit: &EventPermit,
    navigation: &crate::navigation_epoch::NavigationEpochTracker,
    presented: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    args:&webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2ContextMenuRequestedEventArgs,
) -> bool {
    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::HWND,
            System::Com::CoTaskMemFree,
            UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT},
        },
    };
    let Some(activity) = navigation.activity_snapshot() else {
        return false;
    };
    let mut parent = HWND::default();
    if permit.active_token().is_none()
        || !presented.load(Ordering::Acquire)
        || unsafe { controller.ParentWindow(&mut parent) }.is_err()
        || unsafe { GetAncestor(parent, GA_ROOT) != GetForegroundWindow() }
    {
        return false;
    }
    let Ok(items) = (unsafe { args.MenuItems() }) else {
        return false;
    };
    let mut count = 0;
    if unsafe { items.Count(&mut count) }.is_err() || count > 64 {
        return false;
    }
    for index in (0..count).rev() {
        let Ok(item) = (unsafe { items.GetValueAtIndex(index) }) else {
            return false;
        };
        let mut name = PWSTR::null();
        if unsafe { item.Name(&mut name) }.is_err() {
            unsafe { CoTaskMemFree(Some(name.0.cast())) };
            return false;
        }
        let text = if name.is_null() {
            None
        } else {
            (0..=128)
                .find(|offset| unsafe { *name.0.add(*offset) } == 0)
                .and_then(|len| {
                    String::from_utf16(unsafe { std::slice::from_raw_parts(name.0, len) }).ok()
                })
        };
        unsafe { CoTaskMemFree(Some(name.0.cast())) };
        if !text.as_deref().is_some_and(windows_menu_item_allowed)
            && unsafe { items.RemoveValueAtIndex(index) }.is_err()
        {
            return false;
        }
    }
    permit.active_token().is_some()
        && navigation.matches_activity(activity)
        && presented.load(Ordering::Acquire)
        && unsafe { GetAncestor(parent, GA_ROOT) == GetForegroundWindow() }
}
#[cfg(any(target_os = "windows", test))]
fn windows_menu_item_allowed(name: &str) -> bool {
    matches!(
        name,
        "back"
            | "forward"
            | "reload"
            | "undo"
            | "redo"
            | "cut"
            | "copy"
            | "paste"
            | "pasteAndMatchStyle"
            | "selectAll"
            | "copyLinkLocation"
            | "copyImage"
            | "copyImageLocation"
            | "saveImageAs"
            | "saveLinkAs"
            | "saveVideoAs"
            | "saveAudioAs"
            | "openLinkInNewWindow"
            | "openLinkInNewTab"
            | "openImageInNewWindow"
            | "openImageInNewTab"
            | "inspectElement"
            // WebView2 owns the installed extension's submenu and dispatch.
            // Keep it intact after the same foreground/document checks above.
            | "extension"
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_windows_context_menu_keeps_file_actions_but_denies_unbrokered_surfaces() {
        assert!(windows_menu_item_allowed("saveImageAs"));
        assert!(windows_menu_item_allowed("copyLinkLocation"));
        assert!(windows_menu_item_allowed("extension"));
        assert!(windows_menu_item_allowed("inspectElement"));
        for name in [
            "print",
            "savePageAs",
            "share",
            "unknownFutureCommand",
            "custom",
        ] {
            assert!(!windows_menu_item_allowed(name));
        }
    }
}
