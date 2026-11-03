//! Checks the real Wry response-policy delegate in an isolated, hidden WebKit view.
#[cfg(target_os = "macos")]
mod native {
    use objc2::{define_class, msg_send, rc::Retained, ClassType, DefinedClass};
    use objc2_foundation::{MainThreadMarker, NSHTTPURLResponse, NSString, NSURLResponse, NSURL};
    use objc2_web_kit::{WKNavigationResponse, WKNavigationResponsePolicy};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    struct ResponseIvars {
        native: RefCell<Option<Retained<NSURLResponse>>>,
        mime_reads: Cell<usize>,
        can_show: bool,
        main_frame: bool,
    }

    define_class!(
        #[unsafe(super(WKNavigationResponse))]
        #[ivars = ResponseIvars]
        #[name = "ZephiumNavigationResponseGuardFixture"]
        struct Response;
        impl Response {
            #[unsafe(method_id(response))]
            fn native_response(&self) -> Option<Retained<NSURLResponse>> {
                self.ivars().native.borrow_mut().take()
            }
            #[unsafe(method(canShowMIMEType))]
            fn can_show_mime_type(&self) -> bool {
                let ivars = self.ivars();
                ivars.mime_reads.set(ivars.mime_reads.get() + 1);
                ivars.can_show
            }
            #[unsafe(method(isForMainFrame))]
            fn is_for_main_frame(&self) -> bool { self.ivars().main_frame }
        }
    );

    pub fn run() {
        use objc2::{rc::autoreleasepool, AllocAnyThread};
        use objc2_app_kit::{
            NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow,
            NSWindowStyleMask,
        };
        use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};
        use raw_window_handle::{
            AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
        };
        use std::ptr::NonNull;
        use wry::{DownloadPolicy, WebViewBuilder, WebViewExtMacOS};

        struct Host(NonNull<std::ffi::c_void>);
        impl HasWindowHandle for Host {
            fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
                // SAFETY: the retained content view outlives the web views.
                Ok(unsafe {
                    WindowHandle::borrow_raw(RawWindowHandle::AppKit(AppKitWindowHandle::new(
                        self.0,
                    )))
                })
            }
        }
        let mtm = MainThreadMarker::new().expect("main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        autoreleasepool(|_| {
            // SAFETY: the main-thread allocation uses NSWindow's designated initializer.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    NSRect::new(NSPoint::new(-30000.0, -30000.0), NSSize::new(400.0, 300.0)),
                    NSWindowStyleMask::Titled,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            // SAFETY: Retained owns this window; closing must not release it separately.
            unsafe { window.setReleasedWhenClosed(false) };
            let content = window.contentView().expect("content view");
            let host = Host(NonNull::new(Retained::as_ptr(&content) as *mut _).unwrap());
            let url =
                NSURL::URLWithString(&NSString::from_str("https://example.invalid/file")).unwrap();
            // SAFETY: NSURLResponse's initializer receives live URL/string arguments and NSInteger length.
            let normal: Retained<NSURLResponse> = unsafe {
                msg_send![NSURLResponse::alloc(), initWithURL: &*url, MIMEType: &*NSString::from_str("text/html"),
                    expectedContentLength: 0_isize, textEncodingName: &*NSString::from_str("utf-8")]
            };
            let oversized_url = NSURL::URLWithString(&NSString::from_str(&format!(
                "https://example.invalid/{}",
                "x".repeat(8 * 1024)
            )))
            .unwrap();
            // SAFETY: the same NSURLResponse initializer receives a live oversized URL, not a raw buffer.
            let oversized: Retained<NSURLResponse> = unsafe {
                msg_send![NSURLResponse::alloc(), initWithURL: &*oversized_url, MIMEType: &*NSString::from_str("text/html"),
                    expectedContentLength: 0_isize, textEncodingName: &*NSString::from_str("utf-8")]
            };
            // SAFETY: NSDictionary's factory receives live NSString key/value objects matching its type.
            let headers: Retained<NSDictionary<NSString, NSString>> = unsafe {
                msg_send![NSDictionary::<NSString, NSString>::class(), dictionaryWithObject: &*NSString::from_str("attachment; filename=file.html"),
                    forKey: &*NSString::from_str("Content-Disposition")]
            };
            let attachment = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
                NSHTTPURLResponse::alloc(),
                &url,
                200,
                Some(&NSString::from_str("HTTP/1.1")),
                Some(&headers),
            )
            .unwrap()
            .into_super();
            let views = [false, true].map(|download| {
                WebViewBuilder::new()
                    .with_incognito(true)
                    .with_download_policy(if download {
                        DownloadPolicy::UseHandlers
                    } else {
                        DownloadPolicy::DenyWithoutMetadata
                    })
                    .with_download_started_handler(|_, _| {
                        panic!("policy regression must not start a download")
                    })
                    .build_as_child(&host)
                    .expect("isolated webview")
            });
            use WKNavigationResponsePolicy as Policy;
            let cases = [
                (
                    "nil with downloads enabled",
                    true,
                    None,
                    true,
                    true,
                    Policy::Cancel,
                ),
                (
                    "valid main frame uses first response",
                    false,
                    Some(normal.clone()),
                    true,
                    true,
                    Policy::Allow,
                ),
                (
                    "unsupported MIME denied",
                    false,
                    Some(normal.clone()),
                    false,
                    true,
                    Policy::Cancel,
                ),
                (
                    "unsupported MIME downloads",
                    true,
                    Some(normal.clone()),
                    false,
                    true,
                    Policy::Download,
                ),
                (
                    "attachment denied",
                    false,
                    Some(attachment.clone()),
                    true,
                    true,
                    Policy::Cancel,
                ),
                (
                    "attachment downloads",
                    true,
                    Some(attachment),
                    true,
                    true,
                    Policy::Download,
                ),
                (
                    "oversized main-frame URL",
                    false,
                    Some(oversized.clone()),
                    true,
                    true,
                    Policy::Cancel,
                ),
                (
                    "oversized subframe URL",
                    false,
                    Some(oversized.clone()),
                    true,
                    false,
                    Policy::Allow,
                ),
                (
                    "download precedes URL validation",
                    true,
                    Some(oversized),
                    false,
                    true,
                    Policy::Download,
                ),
            ];
            let count = cases.len();
            for (name, download, native, can_show, main_frame, expected) in cases {
                let page = views[usize::from(download)].webview();
                // SAFETY: this live main-thread Wry view retains its installed delegate.
                let delegate =
                    unsafe { page.navigationDelegate() }.expect("Wry navigation delegate");
                let nil = native.is_none();
                let response = mtm.alloc::<Response>().set_ivars(ResponseIvars {
                    native: RefCell::new(native),
                    mime_reads: Cell::new(0),
                    can_show,
                    main_frame,
                });
                // SAFETY: superclass init preserves the allocated fixture; private-backed dealloc is avoided below.
                let response: Retained<Response> = unsafe { msg_send![super(response), init] };
                // shortcut: retain nine fixtures until process exit; public init lacks WebKit's private backing.
                // Use real native responses if this check ever runs in a long-lived process.
                // SAFETY: ownership transfers to a nonnull allocation kept live until process exit.
                let response = unsafe { &*Retained::into_raw(response) };
                let decisions = Rc::new(RefCell::new(Vec::new()));
                let recorded = decisions.clone();
                let handler = block2::RcBlock::new(move |policy: WKNavigationResponsePolicy| {
                    recorded.borrow_mut().push(policy)
                });
                // SAFETY: the actual Wry delegate receives live native objects and its real decision block.
                let _: () = unsafe {
                    msg_send![&*delegate, webView: &*page,
                        decidePolicyForNavigationResponse: response, decisionHandler: &*handler]
                };
                assert_eq!(*decisions.borrow(), vec![expected], "{name}");
                if nil {
                    assert_eq!(response.ivars().mime_reads.get(), 0, "{name}");
                }
                println!("ok: {name}");
            }
            println!("ok: {count} native response-policy cases");
        });
    }
}

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS-only native regression");
}
