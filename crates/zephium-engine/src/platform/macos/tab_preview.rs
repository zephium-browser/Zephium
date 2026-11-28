use objc2_app_kit::NSImage;
use objc2_core_foundation::CFString;
use objc2_core_graphics::{CGDataConsumer, CGDataConsumerCallbacks, CGImage};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSError, NSNumber};
use objc2_web_kit::WKSnapshotConfiguration;
use std::{cell::RefCell, ffi::c_void, panic::AssertUnwindSafe, ptr::NonNull};
use wry::WebViewExtMacOS;

const LIMIT: usize = 80 * 1024;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C-unwind" {
    fn CGImageDestinationCreateWithDataConsumer(
        consumer: Option<&CGDataConsumer>,
        kind: Option<&CFString>,
        count: usize,
        options: *const c_void,
    ) -> *mut c_void;
    fn CGImageDestinationAddImage(
        destination: *mut c_void,
        image: Option<&CGImage>,
        properties: *const c_void,
    );
    fn CGImageDestinationFinalize(destination: *mut c_void) -> bool;
    static kCGImageDestinationLossyCompressionQuality: &'static CFString;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C-unwind" {
    fn CFRelease(value: *mut c_void);
}

struct Destination(NonNull<c_void>);
impl Drop for Destination {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

unsafe extern "C-unwind" fn write(
    info: *mut c_void,
    buffer: NonNull<c_void>,
    count: usize,
) -> usize {
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        // ImageIO invokes this synchronously while encode owns the sink.
        let sink = unsafe { &*(info.cast::<RefCell<Vec<u8>>>()) };
        let mut sink = sink.borrow_mut();
        if count > LIMIT.saturating_sub(sink.len()) {
            return 0;
        }
        let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), count) };
        sink.extend_from_slice(bytes);
        count
    }))
    .unwrap_or(0)
}

fn encode(cg: &CGImage) -> Option<Vec<u8>> {
    if CGImage::width(Some(cg)) == 0
        || CGImage::height(Some(cg)) == 0
        || CGImage::width(Some(cg)).max(CGImage::height(Some(cg))) > 480
    {
        return None;
    }
    let sink = RefCell::new(Vec::new());
    let callbacks = CGDataConsumerCallbacks {
        putBytes: Some(write),
        releaseConsumer: None,
    };
    let consumer =
        unsafe { CGDataConsumer::new(std::ptr::from_ref(&sink).cast_mut().cast(), &callbacks) }?;
    let kind = CFString::from_static_str("public.jpeg");
    let destination = Destination(NonNull::new(unsafe {
        CGImageDestinationCreateWithDataConsumer(Some(&consumer), Some(&kind), 1, std::ptr::null())
    })?);
    let quality = NSNumber::new_f64(0.8);
    let key = unsafe {
        &*(std::ptr::from_ref(kCGImageDestinationLossyCompressionQuality)
            .cast::<objc2_foundation::NSString>())
    };
    let properties = NSDictionary::from_slices(&[key], &[&*quality]);
    unsafe {
        CGImageDestinationAddImage(
            destination.0.as_ptr(),
            Some(cg),
            std::ptr::from_ref(&*properties).cast(),
        );
    }
    let finalized = unsafe { CGImageDestinationFinalize(destination.0.as_ptr()) };
    drop(destination);
    drop(consumer);
    let bytes = sink.into_inner();
    (finalized && !bytes.is_empty()).then_some(bytes)
}

pub(crate) fn capture(
    view: &wry::WebView,
    _full: bool,
    done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(None);
        return;
    };
    let page = view.webview();
    if page.window().is_none() || page.isHiddenOrHasHiddenAncestor() {
        done(None);
        return;
    }
    let bounds = page.bounds();
    let scale = page
        .window()
        .map(|window| window.backingScaleFactor())
        .unwrap_or(1.0);
    if ![bounds.size.width, bounds.size.height, scale]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0)
    {
        done(None);
        return;
    }
    let fit = (480.0 / (bounds.size.width.max(bounds.size.height) * scale)).min(1.0);
    let pixels = (bounds.size.width * scale * fit).floor();
    if pixels < 1.0 {
        done(None);
        return;
    }
    let width = NSNumber::new_f64(pixels / scale);
    let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
    unsafe {
        configuration.setRect(bounds);
        configuration.setSnapshotWidth(Some(&width));
        configuration.setAfterScreenUpdates(false);
    }
    let pending = std::rc::Rc::new(RefCell::new(Some(done)));
    let completed = pending.clone();
    let callback = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(done) = completed.borrow_mut().take() else {
            return;
        };
        let cg = if image.is_null() || !error.is_null() {
            None
        } else {
            objc2::exception::catch(AssertUnwindSafe(|| unsafe {
                (&*image).CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
            }))
            .ok()
            .flatten()
        };
        dispatch2::DispatchQueue::global_queue(dispatch2::GlobalQueueIdentifier::QualityOfService(
            dispatch2::DispatchQoS::Utility,
        ))
        .exec_async(move || {
            let bytes =
                std::panic::catch_unwind(AssertUnwindSafe(|| cg.as_deref().and_then(encode)))
                    .ok()
                    .flatten();
            done(bytes);
        });
    });
    if objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        page.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &callback);
    }))
    .is_err()
    {
        if let Some(done) = pending.borrow_mut().take() {
            done(None);
        }
    }
}
