//! Hidden extension pages: offscreen documents (`chrome.offscreen`), which
//! WebKit doesn't provide, and the page that clears an extension's origin
//! data on uninstall. Each runs in a view built from the extension's own
//! configuration, so it has the extension's origin and APIs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSError, NSObjectProtocol, NSRect, NSString, NSTimer, NSURLRequest, NSURL};
use objc2_web_kit::{
    WKContentWorld, WKNavigation, WKNavigationDelegate, WKUIDelegate, WKWebExtensionContext,
    WKWebView,
};

use crate::runtime::Shared;
use crate::LogLevel;

type Loaded = Box<dyn FnOnce(Result<(), String>)>;

struct Page {
    view: Retained<WKWebView>,
    delegate: Retained<LoadDelegate>,
}

impl Page {
    /// Opens `path` of the extension in a view outside any window; `loaded`
    /// runs once the page has finished loading or has failed to.
    fn open(context: &WKWebExtensionContext, path: &str, loaded: Loaded) -> Option<Self> {
        let url = match resolve(context, path) {
            Ok(url) => url,
            Err(message) => {
                loaded(Err(message));
                return None;
            }
        };
        let (Some(mtm), Some(configuration)) = (MainThreadMarker::new(), unsafe {
            context.webViewConfiguration()
        }) else {
            loaded(Err("The extension is not loaded.".into()));
            return None;
        };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(mtm.alloc(), NSRect::default(), &configuration)
        };
        let delegate = LoadDelegate::new(mtm, loaded);
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.loadRequest(&NSURLRequest::requestWithURL(&url));
        }
        Some(Self { view, delegate })
    }

    fn close(&self) {
        self.delegate.settle(Err("The page was closed.".into()));
        unsafe {
            self.view.stopLoading();
            self.view.setNavigationDelegate(None);
            self.view.setUIDelegate(None);
        }
    }
}

/// A profile's offscreen documents, one per extension.
#[derive(Default)]
pub(crate) struct Documents(RefCell<HashMap<String, Page>>);

type Gone = Box<dyn FnOnce(&WKWebView)>;

pub(crate) struct Ivars {
    loaded: RefCell<Option<Loaded>>,
    gone: RefCell<Option<Gone>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumHiddenPageLoadDelegate"]
    #[ivars = Ivars]
    pub(crate) struct LoadDelegate;

    unsafe impl NSObjectProtocol for LoadDelegate {}

    unsafe impl WKNavigationDelegate for LoadDelegate {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.settle(Ok(()));
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.failed(error);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            _view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.failed(error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn did_terminate(&self, view: &WKWebView) {
            self.settle(Err("The page's process ended.".into()));
            let gone = self.ivars().gone.borrow_mut().take();
            if let Some(gone) = gone {
                gone(view);
            }
        }
    }

    unsafe impl WKUIDelegate for LoadDelegate {
        // WebKit's default for an omitted method is its own prompt; extension
        // pages never receive camera, microphone or motion access.
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn deny_media_capture(
            &self,
            _view: &WKWebView,
            _origin: &objc2_web_kit::WKSecurityOrigin,
            _frame: &objc2_web_kit::WKFrameInfo,
            _capture_type: objc2_web_kit::WKMediaCaptureType,
            decision: &block2::DynBlock<dyn Fn(objc2_web_kit::WKPermissionDecision)>,
        ) {
            decision.call((objc2_web_kit::WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        fn deny_device_motion(
            &self,
            _view: &WKWebView,
            _origin: &objc2_web_kit::WKSecurityOrigin,
            _frame: &objc2_web_kit::WKFrameInfo,
            decision: &block2::DynBlock<dyn Fn(objc2_web_kit::WKPermissionDecision)>,
        ) {
            decision.call((objc2_web_kit::WKPermissionDecision::Deny,));
        }
    }
);

impl LoadDelegate {
    fn new(mtm: MainThreadMarker, loaded: Loaded) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            loaded: RefCell::new(Some(loaded)),
            gone: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn failed(&self, error: &NSError) {
        self.settle(Err(format!(
            "The page failed to load: {}",
            error.localizedDescription()
        )));
    }

    fn settle(&self, result: Result<(), String>) {
        let loaded = self.ivars().loaded.borrow_mut().take();
        if let Some(loaded) = loaded {
            loaded(result);
        }
    }
}

/// Creates the extension's offscreen document. As in Chrome, `done` runs once
/// the document has loaded and can receive messages.
pub(crate) fn create(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    path: &str,
    done: Loaded,
) {
    let extension = unsafe { context.uniqueIdentifier() }.to_string();
    if has(shared, &extension) {
        return done(Err(
            "Only a single offscreen document may be created.".into()
        ));
    }
    let owner = Rc::downgrade(shared);
    let failed = extension.clone();
    let loaded: Loaded = Box::new(move |result| {
        if let (Err(_), Some(shared)) = (&result, owner.upgrade()) {
            close(&shared, &failed);
        }
        done(result);
    });
    let Some(page) = Page::open(context, path, loaded) else {
        return;
    };
    // A crashed document would otherwise count as open forever, and every
    // later createDocument would fail.
    let owner = Rc::downgrade(shared);
    let crashed = extension.clone();
    *page.delegate.ivars().gone.borrow_mut() = Some(Box::new(move |view| {
        let Some(shared) = owner.upgrade() else {
            return;
        };
        let current = shared
            .offscreen
            .0
            .borrow()
            .get(&crashed)
            .is_some_and(|page| std::ptr::eq(&*page.view, view));
        if current {
            close(&shared, &crashed);
            shared.host().log(
                &crashed,
                LogLevel::Warning,
                "the offscreen document's process ended",
            );
        }
    }));
    shared.offscreen.0.borrow_mut().insert(extension, page);
}

pub(crate) fn close(shared: &Shared, extension: &str) -> bool {
    let page = shared.offscreen.0.borrow_mut().remove(extension);
    page.inspect(Page::close).is_some()
}

/// The open offscreen document's address.
pub(crate) fn url(shared: &Shared, extension: &str) -> Option<String> {
    let documents = shared.offscreen.0.borrow();
    let page = documents.get(extension)?;
    unsafe { page.view.URL() }
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
}

pub(crate) fn has(shared: &Shared, extension: &str) -> bool {
    shared.offscreen.0.borrow().contains_key(extension)
}

/// Clears the website data of the extension's origin (IndexedDB,
/// `localStorage`, Cache Storage) from a page of its own, since WebKit
/// doesn't list extension origins among a data store's records. `done` runs
/// once clearing has been requested; the page stays open a little longer so
/// deletions its other pages were blocking can finish after they close.
pub(crate) fn clear_origin(context: &WKWebExtensionContext, done: Box<dyn FnOnce()>) {
    const SCRIPT: &str = "\
        try { localStorage.clear(); } catch {}\n\
        try {\n\
          const names = (await indexedDB.databases()).map((database) => database.name);\n\
          await Promise.all(names.map((name) => new Promise((settle) => {\n\
            const request = indexedDB.deleteDatabase(name);\n\
            request.onsuccess = request.onerror = request.onblocked = settle;\n\
          })));\n\
        } catch {}\n\
        try { await Promise.all((await caches.keys()).map((key) => caches.delete(key))); } catch {}";
    let page: RefCell<Option<Page>> = RefCell::new(None);
    let page = Rc::new(page);
    let opened = page.clone();
    let loaded: Loaded = Box::new(move |result| {
        let Some(view) = opened.borrow().as_ref().map(|page| page.view.clone()) else {
            return done();
        };
        if result.is_err() {
            return done();
        }
        let done = RefCell::new(Some(done));
        let keep = opened.clone();
        let cleared = RcBlock::new(move |_value: *mut AnyObject, _error: *mut NSError| {
            if let Some(done) = done.borrow_mut().take() {
                done();
            }
            linger(keep.clone());
        });
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        unsafe {
            view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(SCRIPT),
                None,
                None,
                &WKContentWorld::pageWorld(mtm),
                Some(&cleared),
            )
        };
    });
    // The manifest is part of every package, so the page always exists.
    let opened = Page::open(context, "manifest.json", loaded);
    *page.borrow_mut() = opened;
}

fn linger(page: Rc<RefCell<Option<Page>>>) {
    let release = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        if let Some(page) = page.borrow_mut().take() {
            page.close();
        }
    });
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(30.0, false, &release) };
}

/// Resolves a path against the extension's own origin; hidden pages are
/// only ever the extension's own.
fn resolve(context: &WKWebExtensionContext, path: &str) -> Result<Retained<NSURL>, String> {
    let base = unsafe { context.baseURL() };
    let url = NSURL::URLWithString_relativeToURL(&NSString::from_str(path), Some(&base))
        .and_then(|url| url.absoluteURL())
        .ok_or("Invalid offscreen document URL.")?;
    // NSURL can expose Swift-backed NSString values whose Objective-C equality
    // signature differs from NSObject's. Compare their values in Rust instead.
    let same_origin = url.scheme().map(|value| value.to_string())
        == base.scheme().map(|value| value.to_string())
        && url.host().map(|value| value.to_string()) == base.host().map(|value| value.to_string());
    if !same_origin {
        return Err("The offscreen document must be one of the extension's own pages.".into());
    }
    Ok(url)
}

/// Main-thread origin qualification used only by the standalone lab.
#[cfg(feature = "lab")]
pub(crate) fn verify_origin(context: &WKWebExtensionContext) -> Result<(), String> {
    let own = resolve(context, "manifest.json")?;
    resolve(
        context,
        &own.absoluteString().ok_or("missing own URL")?.to_string(),
    )?;
    let base = unsafe { context.baseURL() };
    let scheme = base.scheme().ok_or("missing extension scheme")?.to_string();
    let host = base.host().ok_or("missing extension host")?.to_string();
    for path in [
        format!("https://{host}/manifest.json"),
        format!("{scheme}://different-origin.invalid/manifest.json"),
    ] {
        if resolve(context, &path).is_ok() {
            return Err(format!("offscreen origin admitted {path}"));
        }
    }
    Ok(())
}
