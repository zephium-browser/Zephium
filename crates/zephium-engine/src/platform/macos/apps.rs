//! Hands an application link the person allowed to the app macOS has for it.
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

fn native(url: &str) -> Option<objc2::rc::Retained<NSURL>> {
    zephium_core::navigation::external_app_link(url)?;
    NSURL::URLWithString(&NSString::from_str(url))
}

/// The app that would open the link, by the name Finder shows for it.
pub(crate) fn external_app_name(url: &str) -> Option<String> {
    let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&*native(url)?)?;
    let name = app
        .URLByDeletingPathExtension()?
        .lastPathComponent()?
        .to_string();
    (!name.is_empty()).then_some(name)
}

pub(crate) fn open_external_app(url: &str) -> bool {
    native(url).is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url))
}
