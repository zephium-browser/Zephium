//! Protection QA only: a PNG of one of the app's own windows, as the window
//! server composites it. An app may capture its own windows without the
//! screen-recording permission, so no prompt appears.

use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWindow};
#[allow(deprecated)]
use objc2_core_graphics::CGWindowListCreateImage;
use objc2_core_graphics::{CGRectNull, CGWindowImageOption, CGWindowListOption};
use objc2_foundation::NSDictionary;

/// # Safety
/// `ns_window` must be a live `NSWindow` of this app, used on the main thread.
#[allow(deprecated)]
pub unsafe fn capture_window_png(ns_window: *mut std::ffi::c_void) -> Result<Vec<u8>, String> {
    // SAFETY: the caller passes a live NSWindow on the main thread.
    let window = unsafe { &*(ns_window as *const NSWindow) };
    let number = u32::try_from(window.windowNumber()).map_err(|_| "no window number")?;
    // SAFETY: an immutable framework constant.
    let image = CGWindowListCreateImage(
        unsafe { CGRectNull },
        CGWindowListOption::OptionIncludingWindow,
        number,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
    )
    .ok_or("the window server gave no image")?;
    let rep =
        NSBitmapImageRep::initWithCGImage(<NSBitmapImageRep as objc2::AnyThread>::alloc(), &image);
    // SAFETY: PNG takes no properties; the dictionary is empty.
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("no PNG")?;
    Ok(png.to_vec())
}
