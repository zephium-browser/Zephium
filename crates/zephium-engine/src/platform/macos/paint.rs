//! A bounded visual handoff, independent of navigation/security admission.
//! The native cover owns no page pixels and never changes website styles.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use std::panic::AssertUnwindSafe;

use dispatch2::MainThreadBound;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSImageCompressionFactor};
use objc2_core_graphics::CGImage;
use objc2_foundation::{MainThreadMarker, NSData, NSDictionary, NSError, NSNumber, NSString};
use objc2_web_kit::{WKContentWorld, WKSnapshotConfiguration};
use wry::WebViewExtMacOS;
use zephium_core::ids::ItemId;

use super::{ContentPolicyTimeout, ContentStage};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
// Animation frames fire on a blank canvas; the cover holds until the page
// has painted content (or the bound passes) so no empty white frame shows.
const MAX_COVER_TIME: Duration = Duration::from_millis(600);
const FIRST_FRAME: &str = "await new Promise(resolve => { const settle = () => requestAnimationFrame(() => resolve()); try { if (performance.getEntriesByName('first-contentful-paint').length) return settle(); const observer = new PerformanceObserver(list => { if (list.getEntriesByName('first-contentful-paint').length) { observer.disconnect(); settle(); } }); observer.observe({type: 'paint', buffered: true}); } catch (_) { requestAnimationFrame(settle); } }); return true;";
// A restored page reloads from cache under its last frame. Hold that frame
// until the document has parsed and painted twice, within a short bound.
const MAX_RESTORE_COVER_TIME: Duration = Duration::from_millis(1500);
const RESTORED_FRAME: &str = "if (document.readyState === 'loading') await new Promise(resolve => document.addEventListener('DOMContentLoaded', resolve, {once: true})); await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))); return true;";
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;

/// The last frame of a tab about to be discarded, JPEG-compressed at half
/// resolution. Volatile: never written to disk or IPC, and painted only over
/// the same tab while its restored history reloads.
pub(crate) struct PageSnapshot(Retained<NSData>);

impl PageSnapshot {
    pub(crate) fn bytes(&self) -> usize {
        self.0.len()
    }

    /// Returns false when no completion will be delivered.
    pub(crate) fn capture(view: &wry::WebView, done: impl FnOnce(Option<Self>) + 'static) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let page = view.webview();
        let bounds = page.bounds();
        if !(bounds.size.width >= 2.0 && bounds.size.height >= 2.0) {
            return false;
        }
        // SAFETY: main-thread WebKit object creation and configuration.
        let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
        let width = NSNumber::new_f64((bounds.size.width / 2.0).round());
        unsafe {
            configuration.setRect(bounds);
            configuration.setSnapshotWidth(Some(&width));
            configuration.setAfterScreenUpdates(false);
        }
        let pending = RefCell::new(Some(done));
        let callback = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(done) = pending.borrow_mut().take() else {
                return;
            };
            let snapshot = (!image.is_null() && error.is_null())
                .then(|| {
                    objc2::exception::catch(AssertUnwindSafe(|| {
                        // SAFETY: WebKit keeps a non-null image valid for the callback.
                        encode(unsafe { &*image })
                    }))
                    .ok()
                    .flatten()
                })
                .flatten();
            done(snapshot);
        });
        // SAFETY: WebKit copies the block and replies once on the main thread.
        objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            page.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &callback);
        }))
        .is_ok()
    }

    fn image(&self) -> Option<Retained<CGImage>> {
        let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &self.0)?;
        rep.CGImage()
    }
}

fn encode(image: &NSImage) -> Option<PageSnapshot> {
    // SAFETY: AppKit accepts a null proposed rectangle; nothing is retained.
    let cg =
        unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    let quality = NSNumber::new_f64(0.6);
    // SAFETY: the key is an immutable AppKit constant.
    let properties = NSDictionary::from_slices(
        &[unsafe { NSImageCompressionFactor }],
        &[&*quality as &AnyObject],
    );
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &properties)
    }?;
    (!data.is_empty() && data.len() <= MAX_SNAPSHOT_BYTES).then_some(PageSnapshot(data))
}

struct State {
    id: ItemId,
    token: u64,
    stages: Vec<Weak<ContentStage>>,
    finished: Cell<bool>,
    timeout: RefCell<Option<ContentPolicyTimeout>>,
}

impl State {
    fn finish(&self) {
        if self.finished.replace(true) {
            return;
        }
        self.timeout.borrow_mut().take();
        for stage in &self.stages {
            if let Some(stage) = stage.load() {
                stage.uncover(self.id, self.token);
            }
        }
    }
}

/// Lives with the exact native view. Closing/replacing it cancels the handoff.
pub(crate) struct PaintCover(Rc<State>);

impl PaintCover {
    pub(crate) fn begin(
        id: ItemId,
        view: &wry::WebView,
        stages: impl Iterator<Item = Retained<ContentStage>>,
        snapshot: Option<PageSnapshot>,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let token = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()?;
        let image = snapshot.as_ref().and_then(PageSnapshot::image);
        let stages: Vec<_> = stages
            .filter(|stage| stage.cover(id, token, image.as_deref()))
            .map(|stage| Weak::from_retained(&stage))
            .collect();
        if stages.is_empty() {
            return None;
        }
        let state = Rc::new(State {
            id,
            token,
            stages,
            finished: Cell::new(false),
            timeout: RefCell::new(None),
        });
        let timeout_state = MainThreadBound::new(state.clone(), mtm);
        let (limit, script) = if image.is_some() {
            (MAX_RESTORE_COVER_TIME, RESTORED_FRAME)
        } else {
            (MAX_COVER_TIME, FIRST_FRAME)
        };
        let Some(timeout) = super::schedule_presentation_timeout(limit, move || {
            if let Some(mtm) = MainThreadMarker::new() {
                timeout_state.get(mtm).finish();
            }
        }) else {
            state.finish();
            return None;
        };
        *state.timeout.borrow_mut() = Some(timeout);
        let completed = state.clone();
        let callback = block2::RcBlock::new(move |_: *mut AnyObject, _: *mut NSError| {
            // A renderer answer is only a hint to remove an inert cover. It
            // grants no visibility, IPC, navigation or native input authority.
            completed.finish();
        });
        let page = view.webview();
        // SAFETY: the view and world are retained on WebKit's main thread;
        // WebKit copies the completion block. The script returns a primitive,
        // touches no DOM, and its paint observer disconnects at first paint.
        // It runs in the page's own world: a named world would build a second
        // realm in every document for its lifetime, and a page that delays or
        // forges this hint only moves the bounded cover's removal.
        unsafe {
            let world = WKContentWorld::pageWorld(mtm);
            page.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(script),
                None,
                None,
                &world,
                Some(&callback),
            );
        }
        Some(Self(state))
    }
}

impl Drop for PaintCover {
    fn drop(&mut self) {
        self.0.finish();
    }
}
