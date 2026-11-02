//! What a person decided for one site, as Site settings lists and changes it.
//! macOS keeps camera, microphone and location in the profile's page-permission
//! catalog; Windows keeps every engine kind in the WebView2 profile. Pop-ups
//! are the browser's own on both.

use super::{PageOrigin, PagePermissionKind, RememberedPagePermission};

/// Most saved decisions one listing returns.
pub const MAX_SITE_PERMISSIONS: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SitePermissionKind {
    Camera,
    Microphone,
    Geolocation,
    Notifications,
    ClipboardRead,
    Midi,
    Sensors,
    LocalFonts,
    AutomaticDownloads,
    Popups,
}

impl SitePermissionKind {
    /// The catalog kind macOS remembers this as, when it remembers it.
    pub const fn page(self) -> Option<PagePermissionKind> {
        match self {
            Self::Camera => Some(PagePermissionKind::Camera),
            Self::Microphone => Some(PagePermissionKind::Microphone),
            Self::Geolocation => Some(PagePermissionKind::Geolocation),
            _ => None,
        }
    }

    pub const fn from_page(kind: PagePermissionKind) -> Option<Self> {
        match kind {
            PagePermissionKind::Camera => Some(Self::Camera),
            PagePermissionKind::Microphone => Some(Self::Microphone),
            PagePermissionKind::Geolocation => Some(Self::Geolocation),
            PagePermissionKind::Notifications | PagePermissionKind::ClipboardRead => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SitePermission {
    pub origin: PageOrigin,
    pub kind: SitePermissionKind,
    pub decision: RememberedPagePermission,
}

/// One listing, or None when its source cannot be read right now.
pub type SitePermissionsDone = Box<dyn FnOnce(Option<Vec<SitePermission>>) + Send>;
/// Whether a change reached its source.
pub type SitePermissionSetDone = Box<dyn FnOnce(bool) + Send>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_macos_remembers_maps_to_the_catalog() {
        for kind in [
            SitePermissionKind::Camera,
            SitePermissionKind::Microphone,
            SitePermissionKind::Geolocation,
        ] {
            let page = kind.page().unwrap();
            assert_eq!(SitePermissionKind::from_page(page), Some(kind));
        }
        for kind in [
            SitePermissionKind::Notifications,
            SitePermissionKind::ClipboardRead,
            SitePermissionKind::Midi,
            SitePermissionKind::Sensors,
            SitePermissionKind::LocalFonts,
            SitePermissionKind::AutomaticDownloads,
            SitePermissionKind::Popups,
        ] {
            assert_eq!(kind.page(), None);
        }
    }
}
