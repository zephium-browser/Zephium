//! The Chrome Web Store's in-page "Add to Zephium" button on WebKit.
//!
//! Its script runs in a content world of its own: the page can neither reach
//! the button nor post to the handler, which exists only in that world.
//! WebKit names the world, frame and view of every message, and the handler
//! checks each before anything reaches the host.

use std::panic::AssertUnwindSafe;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass as _, MainThreadOnly};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView,
};
use zephium_webext::{store, ExtensionId};

const WORLD: &str = "zephium-store";
const HANDLER: &str = "zephiumStore";
const STORE_HOST: &str = "chromewebstore.google.com";
const MAX_BODY_UTF16: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StoreMessage {
    /// A new document asks for the button's state.
    Ready,
    /// A trusted click on the button, for the listing the view shows.
    Clicked(ExtensionId),
}

struct HandlerIvars {
    page: Weak<WKWebView>,
    world: Retained<WKContentWorld>,
    on_message: Box<dyn Fn(StoreMessage)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumStoreButtonHandler"]
    #[ivars = HandlerIvars]
    struct Handler;

    unsafe impl NSObjectProtocol for Handler {}

    unsafe impl WKScriptMessageHandler for Handler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let ivars = self.ivars();
            if let Some(message) = admit(ivars, message) {
                // Never unwind through WebKit's Objective-C callback frame.
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| (ivars.on_message)(message)));
            }
        }
    }
);

fn admit(ivars: &HandlerIvars, message: &WKScriptMessage) -> Option<StoreMessage> {
    let page = ivars.page.load()?;
    // SAFETY: WebKit delivers the message on the main thread and keeps it
    // alive for this call; every accessor returns retained objects.
    let (world, frame, view, body) = unsafe {
        (
            message.world(),
            message.frameInfo(),
            message.webView(),
            message.body(),
        )
    };
    if Retained::as_ptr(&world) != Retained::as_ptr(&ivars.world)
        || view.is_none_or(|view| Retained::as_ptr(&view) != Retained::as_ptr(&page))
    {
        return None;
    }
    // SAFETY: as above.
    let origin = unsafe { frame.securityOrigin() };
    let store_frame = unsafe {
        frame.isMainFrame()
            && origin.protocol().to_string() == "https"
            && origin.host().to_string() == STORE_HOST
            && origin.port() == 0
    };
    if !store_frame {
        return None;
    }
    let body = body.downcast::<NSString>().ok()?;
    if body.length() > MAX_BODY_UTF16 {
        return None;
    }
    let body = body.to_string();
    if body == "ready" {
        return Some(StoreMessage::Ready);
    }
    // SAFETY: a live view on the main thread.
    let address = unsafe { page.URL().and_then(|url| url.absoluteString()) }?.to_string();
    store::listing_id(&address)
        .filter(|id| id.as_str() == body)
        .map(StoreMessage::Clicked)
}

/// The world and handler of one view; dropping it removes the handler. WebKit
/// cannot remove a single script, so the script stays with the view.
pub(crate) struct StoreButton {
    controller: Retained<WKUserContentController>,
    world: Retained<WKContentWorld>,
    page: Weak<WKWebView>,
    _handler: Retained<Handler>,
}

impl StoreButton {
    /// Runs the store's page script in `page`'s own store world and routes
    /// what its button reports to `on_message`.
    pub(crate) fn install(
        page: &WKWebView,
        on_message: impl Fn(StoreMessage) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        // SAFETY: main thread proven above; the name is constant ASCII.
        let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(WORLD), mtm) };
        // SAFETY: main-thread WebKit calls on a live view.
        let controller = unsafe { page.configuration().userContentController() };
        let handler = Handler::alloc(mtm).set_ivars(HandlerIvars {
            page: Weak::from(page),
            world: world.clone(),
            on_message: Box::new(on_message),
        });
        // SAFETY: a freshly allocated instance with initialized ivars.
        let handler: Retained<Handler> = unsafe { msg_send![super(handler), init] };
        objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            controller.addScriptMessageHandler_contentWorld_name(
                ProtocolObject::from_ref(&*handler),
                &world,
                &NSString::from_str(HANDLER),
            );
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                WKUserScript::alloc(mtm),
                &NSString::from_str(store::PAGE_SCRIPT),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
                &world,
            );
            controller.addUserScript(&script);
        }))
        .ok()?;
        Some(Self {
            controller,
            world,
            page: Weak::from(page),
            _handler: handler,
        })
    }

    /// Shows `state`, a JSON object the script's `show` accepts, in the
    /// view's current document.
    pub(crate) fn show(&self, state: &str) {
        let Some(page) = self.page.load() else {
            return;
        };
        let script = format!("window.__zephium_store_v1__?.show({state})");
        // SAFETY: main-thread WebKit call on a live view and world.
        unsafe {
            page.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(&script),
                None,
                &self.world,
                None,
            );
        }
    }
}

impl Drop for StoreButton {
    fn drop(&mut self) {
        let _ = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            self.controller
                .removeScriptMessageHandlerForName_contentWorld(
                    &NSString::from_str(HANDLER),
                    &self.world,
                );
        }));
    }
}
