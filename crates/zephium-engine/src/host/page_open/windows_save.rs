//! One resource save from an admitted native menu, bound to its document.
use crate::navigation_epoch::{NavigationActivity, NavigationEpochTracker};
use std::cell::Cell;
use webview2_com::Microsoft::Web::WebView2::Win32::*;

#[derive(Clone, Copy)]
enum Pending {
    Menu(NavigationActivity),
    Download(NavigationActivity),
}

#[derive(Default)]
pub(in crate::host) struct ResourceSave(Cell<Option<Pending>>);
impl ResourceSave {
    pub(in crate::host) fn menu(
        &self,
        navigation: &NavigationEpochTracker,
        args: &ICoreWebView2ContextMenuRequestedEventArgs,
        admitted: bool,
    ) {
        self.0.set(None);
        if !admitted {
            return;
        }
        let Some(activity) = navigation.activity_snapshot() else {
            return;
        };
        let Ok(target) = (unsafe { args.ContextMenuTarget() }) else {
            return;
        };
        let mut kind = COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND::default();
        let mut link = windows::core::BOOL::default();
        if unsafe {
            target
                .Kind(&mut kind)
                .and_then(|_| target.HasLinkUri(&mut link))
        }
        .is_err()
        {
            return;
        }
        // SaveAsUIShowing reports the document MIME even for Save Image As.
        // Use the runtime's native menu target, never that MIME or page script.
        if link.as_bool()
            || matches!(
                kind,
                COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_IMAGE
                    | COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_VIDEO
                    | COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_AUDIO
            )
        {
            self.0.set(Some(Pending::Menu(activity)));
        }
    }

    pub(in crate::host) fn authorize(
        &self,
        navigation: &NavigationEpochTracker,
        foreground: bool,
    ) -> bool {
        let Some(Pending::Menu(activity)) = self.0.take() else {
            return false;
        };
        if !foreground || !navigation.matches_activity(activity) {
            return false;
        }
        self.0.set(Some(Pending::Download(activity)));
        true
    }

    pub(in crate::host) fn take_download(&self, navigation: &NavigationEpochTracker) -> bool {
        matches!(self.0.take(), Some(Pending::Download(activity)) if navigation.matches_activity(activity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_authority_is_consumed_and_never_follows_a_new_document() {
        let navigation = NavigationEpochTracker::new();
        navigation.begin("https://fixture.example/image").unwrap();
        let activity = navigation.activity_snapshot().unwrap();
        let save = ResourceSave::default();
        assert!(!save.authorize(&navigation, true));
        save.0.set(Some(Pending::Menu(activity)));
        assert!(!save.authorize(&navigation, false));
        assert!(!save.authorize(&navigation, true));
        save.0.set(Some(Pending::Menu(activity)));
        assert!(save.authorize(&navigation, true));
        assert!(save.take_download(&navigation));
        assert!(!save.take_download(&navigation));
        save.0.set(Some(Pending::Menu(activity)));
        navigation
            .begin("https://fixture.example/replaced")
            .unwrap();
        assert!(!save.authorize(&navigation, true));
        save.0.set(Some(Pending::Download(activity)));
        assert!(!save.take_download(&navigation));
    }
}
